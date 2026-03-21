//! Hardware sensor collection: reads hwmon sysfs, procfs, DRM sysfs, and nvidia-smi.
//!
//! `HwCollector` owns the persistent state (min/max/history). Call `update()` each
//! display-refresh tick; it merges new readings into `self.data` in-place so history
//! is preserved across samples.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::Instant;

pub const HISTORY_LEN: usize = 120;

// ── Sensor ────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct Sensor {
    pub label: &'static str,
    pub value: f32,
    pub unit: &'static str,
    pub min: f32,
    pub max: f32,
    avg_sum: f64,
    avg_count: u64,
    pub history: VecDeque<f32>,
}

impl Default for Sensor {
    fn default() -> Self {
        Self {
            label: "",
            value: 0.0,
            unit: "",
            min: f32::MAX,
            max: f32::MIN,
            avg_sum: 0.0,
            avg_count: 0,
            history: VecDeque::with_capacity(HISTORY_LEN + 1),
        }
    }
}

impl Sensor {
    pub fn new(label: &'static str, unit: &'static str) -> Self {
        Self { label, unit, ..Default::default() }
    }

    pub fn push(&mut self, value: f32) {
        self.value = value;
        if self.avg_count == 0 {
            self.min = value;
            self.max = value;
        } else {
            if value < self.min { self.min = value; }
            if value > self.max { self.max = value; }
        }
        self.avg_sum += value as f64;
        self.avg_count += 1;
        self.history.push_back(value);
        while self.history.len() > HISTORY_LEN {
            self.history.pop_front();
        }
    }

    pub fn avg(&self) -> f32 {
        if self.avg_count == 0 { 0.0 } else { (self.avg_sum / self.avg_count as f64) as f32 }
    }

    pub fn min_display(&self) -> f32 {
        if self.avg_count == 0 { 0.0 } else { self.min }
    }

    pub fn max_display(&self) -> f32 {
        if self.avg_count == 0 { 0.0 } else { self.max }
    }
}

// ── SensorGroup ───────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct SensorGroup {
    /// High-level section: "CPU", "GPU", "Memory", "Storage", "Network", "System"
    pub category: &'static str,
    /// Human-readable device / component name shown as sub-header
    pub name: String,
    pub sensors: Vec<Sensor>,
}

// ── HwMonitorData ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct HwMonitorData {
    pub groups: Vec<SensorGroup>,
}

// ── HwCollector ───────────────────────────────────────────────────────────────

pub struct HwCollector {
    pub data: HwMonitorData,
    /// device_name → [sectors_read, sectors_written]
    prev_disk: HashMap<String, [u64; 2]>,
    /// iface_name → [rx_bytes, tx_bytes]
    prev_net: HashMap<String, [u64; 2]>,
    prev_time: Instant,
}

impl HwCollector {
    pub fn new() -> Self {
        let mut c = Self {
            data: HwMonitorData::default(),
            prev_disk: HashMap::new(),
            prev_net: HashMap::new(),
            prev_time: Instant::now(),
        };
        c.prev_disk = read_disk_raw();
        c.prev_net  = read_net_raw();
        c
    }

    pub fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.prev_time).as_secs_f32().max(0.1);

        let new_disk = read_disk_raw();
        let new_net  = read_net_raw();

        let readings = collect_all(&self.prev_disk, &new_disk, &self.prev_net, &new_net, dt);

        self.prev_disk = new_disk;
        self.prev_net  = new_net;
        self.prev_time = now;

        self.merge(readings);
    }

    /// Merge new readings into `self.data`, preserving existing sensor history.
    fn merge(&mut self, readings: Vec<(&'static str, String, Vec<Reading>)>) {
        for (category, group_name, sensors) in readings {
            let g_idx = match self.data.groups.iter().position(|g| g.name == group_name) {
                Some(i) => i,
                None => {
                    self.data.groups.push(SensorGroup {
                        category,
                        name: group_name,
                        sensors: Vec::new(),
                    });
                    self.data.groups.len() - 1
                }
            };
            for (label, unit, value) in sensors {
                let group = &mut self.data.groups[g_idx];
                match group.sensors.iter_mut().find(|s| s.label == label) {
                    Some(s) => s.push(value),
                    None => {
                        let mut s = Sensor::new(label, unit);
                        s.push(value);
                        group.sensors.push(s);
                    }
                }
            }
        }
    }
}

// ── Reading type ──────────────────────────────────────────────────────────────

/// (label, unit, value) — all strings are &'static to avoid allocation per tick.
type Reading = (&'static str, &'static str, f32);

/// (category, group_name, readings)
type GroupReading = (&'static str, String, Vec<Reading>);

// ── Top-level collector ───────────────────────────────────────────────────────

fn collect_all(
    prev_disk: &HashMap<String, [u64; 2]>,
    new_disk:  &HashMap<String, [u64; 2]>,
    prev_net:  &HashMap<String, [u64; 2]>,
    new_net:   &HashMap<String, [u64; 2]>,
    dt: f32,
) -> Vec<GroupReading> {
    let mut out: Vec<GroupReading> = Vec::new();

    // CPU: hwmon temps + frequencies + load
    out.extend(collect_hwmon_cpu());
    if let Some(g) = collect_cpu_freqs()  { out.push(g); }
    if let Some(g) = collect_load_avg()   { out.push(g); }

    // GPU: prefer nvidia-smi; fall back to amdgpu hwmon
    let gpu = collect_nvidia_smi();
    if !gpu.is_empty() {
        out.extend(gpu);
    } else {
        out.extend(collect_hwmon_category("GPU"));
    }

    // Memory: SPD temps + /proc/meminfo
    out.extend(collect_hwmon_memory());
    if let Some(g) = collect_meminfo() { out.push(g); }

    // Storage: NVMe hwmon + disk I/O
    out.extend(collect_hwmon_storage());
    out.extend(collect_disk_io(prev_disk, new_disk, dt));

    // Network: NIC hwmon + interface I/O
    out.extend(collect_hwmon_network());
    out.extend(collect_net_io(prev_net, new_net, dt));

    out
}

// ── hwmon: per-category collectors ───────────────────────────────────────────

fn collect_hwmon_cpu() -> Vec<GroupReading> {
    collect_hwmon_where(|hw_name| matches!(hw_name, "k10temp" | "zenpower" | "coretemp"), |_path, hw_name| {
        let label = match hw_name {
            "k10temp"  => "AMD CPU [k10temp]",
            "zenpower" => "AMD CPU [zenpower]",
            _          => "Intel CPU [coretemp]",
        };
        ("CPU", label.to_string())
    })
}

fn collect_hwmon_memory() -> Vec<GroupReading> {
    collect_hwmon_where(|hw_name| matches!(hw_name, "spd5118" | "ee1004"), |path, _hw_name| {
        let slot = dimm_slot_name(path);
        ("Memory", slot)
    })
}

fn collect_hwmon_storage() -> Vec<GroupReading> {
    collect_hwmon_where(|hw_name| hw_name == "nvme", |path, _hw_name| {
        let model = read_trimmed(&path.join("device/model"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "NVMe".into());
        ("Storage", model)
    })
}

fn collect_hwmon_network() -> Vec<GroupReading> {
    collect_hwmon_where(
        |hw_name| {
            hw_name.starts_with("r8") || hw_name.starts_with("atlantic")
                || hw_name.starts_with("igb") || hw_name.starts_with("ixgbe")
                || hw_name.starts_with("e1000")
        },
        |path, hw_name| {
            // Try to derive network interface name from device symlink
            let iface = nic_interface_name(path).unwrap_or_else(|| hw_name.to_string());
            ("Network", format!("NIC [{iface}]"))
        },
    )
}

/// Collect hwmon groups for any non-handled driver (amdgpu etc.)
fn collect_hwmon_category(target_category: &'static str) -> Vec<GroupReading> {
    collect_hwmon_where(
        |hw_name| matches!(hw_name, "amdgpu" | "nvidia"),
        |_path, hw_name| (target_category, format!("{hw_name} GPU")),
    )
}

// ── Generic hwmon scanner ─────────────────────────────────────────────────────

fn collect_hwmon_where<F, G>(filter: F, namer: G) -> Vec<GroupReading>
where
    F: Fn(&str) -> bool,
    G: Fn(&Path, &str) -> (&'static str, String),
{
    let mut groups: Vec<GroupReading> = Vec::new();

    let hwmon_dir = Path::new("/sys/class/hwmon");
    let mut entries: Vec<_> = match std::fs::read_dir(hwmon_dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return groups,
    };
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let hw_name = match read_trimmed(&path.join("name")) {
            Some(n) => n,
            None => continue,
        };
        if !filter(&hw_name) { continue; }

        let mut sensors: Vec<Reading> = Vec::new();

        // Temperature
        for i in 1..=24u32 {
            let input = path.join(format!("temp{i}_input"));
            if !input.exists() { continue; }
            if let Some(val) = read_millidegrees(&input) {
                let lbl_file = path.join(format!("temp{i}_label"));
                let lbl = read_trimmed(&lbl_file).unwrap_or_else(|| format!("Temp {i}"));
                sensors.push((intern(lbl), "°C", val));
            }
        }

        // Fan
        for i in 1..=8u32 {
            let input = path.join(format!("fan{i}_input"));
            if !input.exists() { continue; }
            if let Some(rpm) = read_u64(&input) {
                let lbl_file = path.join(format!("fan{i}_label"));
                let lbl = read_trimmed(&lbl_file).unwrap_or_else(|| format!("Fan {i}"));
                sensors.push((intern(lbl), "RPM", rpm as f32));
            }
        }

        // Power (µW → W)
        for i in 1..=8u32 {
            let input = path.join(format!("power{i}_input"));
            if !input.exists() { continue; }
            if let Some(uw) = read_u64(&input) {
                let lbl_file = path.join(format!("power{i}_label"));
                let lbl = read_trimmed(&lbl_file).unwrap_or_else(|| format!("Power {i}"));
                sensors.push((intern(lbl), "W", uw as f32 / 1_000_000.0));
            }
        }

        // Voltage (mV → V)
        for i in 0..=16u32 {
            let input = path.join(format!("in{i}_input"));
            if !input.exists() { continue; }
            if let Some(mv) = read_u64(&input) {
                let lbl_file = path.join(format!("in{i}_label"));
                let lbl = read_trimmed(&lbl_file).unwrap_or_else(|| format!("In {i}"));
                sensors.push((intern(lbl), "V", mv as f32 / 1000.0));
            }
        }

        // Frequency (Hz → MHz; amdgpu etc.)
        for i in 1..=4u32 {
            let input = path.join(format!("freq{i}_input"));
            if !input.exists() { continue; }
            if let Some(hz) = read_u64(&input) {
                let lbl_file = path.join(format!("freq{i}_label"));
                let lbl = read_trimmed(&lbl_file).unwrap_or_else(|| format!("Freq {i}"));
                sensors.push((intern(lbl), "MHz", hz as f32 / 1_000_000.0));
            }
        }

        if !sensors.is_empty() {
            let (cat, name) = namer(&path, &hw_name);
            groups.push((cat, name, sensors));
        }
    }

    groups
}

// ── NVIDIA GPU via nvidia-smi ─────────────────────────────────────────────────

fn collect_nvidia_smi() -> Vec<GroupReading> {
    let output = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,temperature.gpu,power.draw,\
             clocks.current.graphics,clocks.current.memory,\
             utilization.gpu,utilization.memory,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };

    let text = String::from_utf8_lossy(&output.stdout);
    let mut groups = Vec::new();

    for line in text.lines() {
        let p: Vec<&str> = line.splitn(9, ", ").collect();
        if p.len() < 9 { continue; }

        let gpu_name   = p[0].trim();
        let temp:  f32 = p[1].trim().parse().unwrap_or(0.0);
        let power: f32 = p[2].trim().parse().unwrap_or(0.0);
        let g_clk: f32 = p[3].trim().parse().unwrap_or(0.0);
        let m_clk: f32 = p[4].trim().parse().unwrap_or(0.0);
        let g_util: f32 = p[5].trim().parse().unwrap_or(0.0);
        let m_util: f32 = p[6].trim().parse().unwrap_or(0.0);
        // nvidia-smi reports memory in MiB
        let m_used:  f32 = p[7].trim().parse::<f32>().unwrap_or(0.0) / 1024.0;
        let m_total: f32 = p[8].trim().parse::<f32>().unwrap_or(0.0) / 1024.0;

        let sensors: Vec<Reading> = vec![
            ("Temperature",  "°C",  temp),
            ("GPU Load",     "%",   g_util),
            ("Memory Usage", "%",   m_util),
            ("Power Draw",   "W",   power),
            ("GPU Clock",    "MHz", g_clk),
            ("Memory Clock", "MHz", m_clk),
            ("VRAM Used",    "GiB", m_used),
            ("VRAM Total",   "GiB", m_total),
        ];

        groups.push(("GPU", intern(gpu_name).to_string(), sensors));
    }

    groups
}

// ── CPU frequencies ───────────────────────────────────────────────────────────

fn collect_cpu_freqs() -> Option<GroupReading> {
    let cpu_dir = Path::new("/sys/devices/system/cpu");
    let mut entries: Vec<_> = std::fs::read_dir(cpu_dir)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.starts_with("cpu") && s[3..].parse::<u32>().is_ok()
        })
        .collect();
    entries.sort_by_key(|e| {
        e.file_name().to_string_lossy()[3..].parse::<u32>().unwrap_or(0)
    });

    let mut sensors: Vec<Reading> = Vec::new();
    for entry in entries {
        let num: u32 = entry.file_name().to_string_lossy()[3..].parse().unwrap_or(0);
        let freq_path = entry.path().join("cpufreq/scaling_cur_freq");
        if let Some(khz) = read_u64(&freq_path) {
            sensors.push((intern(format!("CPU {num}")), "MHz", khz as f32 / 1000.0));
        }
    }

    if sensors.is_empty() { None } else { Some(("CPU", "CPU Frequencies".into(), sensors)) }
}

// ── Load average ──────────────────────────────────────────────────────────────

fn collect_load_avg() -> Option<GroupReading> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    let mut parts = text.split_whitespace();
    let l1:  f32 = parts.next()?.parse().ok()?;
    let l5:  f32 = parts.next()?.parse().ok()?;
    let l15: f32 = parts.next()?.parse().ok()?;

    Some(("CPU", "Load Average".into(), vec![
        ("1 min",  "", l1),
        ("5 min",  "", l5),
        ("15 min", "", l15),
    ]))
}

// ── /proc/meminfo ─────────────────────────────────────────────────────────────

fn collect_meminfo() -> Option<GroupReading> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut map: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        let mut p = line.split_whitespace();
        if let (Some(k), Some(v)) = (p.next(), p.next()) {
            if let Ok(n) = v.parse::<u64>() {
                map.insert(k.trim_end_matches(':'), n);
            }
        }
    }

    let gib = |k: &str| map.get(k).copied().unwrap_or(0) as f32 / (1024.0 * 1024.0);

    let total     = gib("MemTotal");
    let available = gib("MemAvailable");
    let used      = total - available;
    let buffers   = gib("Buffers");
    let cached    = gib("Cached");
    let swap_total = gib("SwapTotal");
    let swap_used  = swap_total - gib("SwapFree");

    let mut sensors: Vec<Reading> = vec![
        ("Total",     "GiB", total),
        ("Used",      "GiB", used),
        ("Available", "GiB", available),
        ("Buffers",   "GiB", buffers),
        ("Cached",    "GiB", cached),
    ];
    if swap_total > 0.0 {
        sensors.push(("Swap Total", "GiB", swap_total));
        sensors.push(("Swap Used",  "GiB", swap_used));
    }

    Some(("Memory", "RAM".into(), sensors))
}

// ── Disk I/O ──────────────────────────────────────────────────────────────────

fn read_disk_raw() -> HashMap<String, [u64; 2]> {
    let mut map = HashMap::new();
    let text = match std::fs::read_to_string("/proc/diskstats") {
        Ok(t) => t, Err(_) => return map,
    };
    for line in text.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() < 14 { continue; }
        let dev = p[2].to_string();
        // Skip plain partition entries (sdXN, nvmeXnXpX, etc.) — keep whole disks
        let is_partition = dev.chars().last().map(|c| c.is_ascii_digit()).unwrap_or(false)
            && !dev.starts_with("nvme") && !dev.starts_with("mmcblk");
        if is_partition { continue; }
        let r: u64 = p[5].parse().unwrap_or(0);  // sectors read
        let w: u64 = p[9].parse().unwrap_or(0);  // sectors written
        map.insert(dev, [r, w]);
    }
    map
}

fn collect_disk_io(
    prev: &HashMap<String, [u64; 2]>,
    new:  &HashMap<String, [u64; 2]>,
    dt: f32,
) -> Vec<GroupReading> {
    let mut groups = Vec::new();
    let sector = 512.0_f32;

    let mut devs: Vec<&String> = new.keys().collect();
    devs.sort();

    for dev in devs {
        let n = &new[dev];
        let p = prev.get(dev).copied().unwrap_or(*n);

        let read_mb  = n[0].saturating_sub(p[0]) as f32 * sector / (dt * 1_048_576.0);
        let write_mb = n[1].saturating_sub(p[1]) as f32 * sector / (dt * 1_048_576.0);

        // Try to find a human-readable model for this block device
        let label = block_device_model(dev)
            .map(|m| format!("{dev} — {m}"))
            .unwrap_or_else(|| dev.clone());

        groups.push(("Storage", format!("I/O [{label}]"), vec![
            ("Read",  "MB/s", read_mb),
            ("Write", "MB/s", write_mb),
        ]));
    }
    groups
}

fn block_device_model(dev: &str) -> Option<String> {
    // /sys/block/<dev>/device/model  (NVMe)
    let path = Path::new("/sys/block").join(dev).join("device/model");
    read_trimmed(&path).map(|s| s.trim().to_string())
}

// ── Network I/O ───────────────────────────────────────────────────────────────

fn read_net_raw() -> HashMap<String, [u64; 2]> {
    let mut map = HashMap::new();
    let text = match std::fs::read_to_string("/proc/net/dev") {
        Ok(t) => t, Err(_) => return map,
    };
    for line in text.lines().skip(2) {
        let line = line.trim();
        let colon = match line.find(':') { Some(c) => c, None => continue };
        let iface = line[..colon].trim().to_string();
        if iface == "lo" { continue; }
        let fields: Vec<&str> = line[colon + 1..].split_whitespace().collect();
        if fields.len() < 9 { continue; }
        let rx: u64 = fields[0].parse().unwrap_or(0);
        let tx: u64 = fields[8].parse().unwrap_or(0);
        map.insert(iface, [rx, tx]);
    }
    map
}

fn collect_net_io(
    prev: &HashMap<String, [u64; 2]>,
    new:  &HashMap<String, [u64; 2]>,
    dt: f32,
) -> Vec<GroupReading> {
    let mut groups = Vec::new();
    let mut ifaces: Vec<&String> = new.keys().collect();
    ifaces.sort();

    for iface in ifaces {
        let n = &new[iface];
        let p = prev.get(iface).copied().unwrap_or(*n);

        let rx = n[0].saturating_sub(p[0]) as f32 / (dt * 1_048_576.0);
        let tx = n[1].saturating_sub(p[1]) as f32 / (dt * 1_048_576.0);

        groups.push(("Network", format!("I/O [{iface}]"), vec![
            ("Receive",  "MB/s", rx),
            ("Transmit", "MB/s", tx),
        ]));
    }
    groups
}

// ── sysfs helpers ─────────────────────────────────────────────────────────────

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

fn read_millidegrees(path: &Path) -> Option<f32> {
    read_u64(path).map(|v| v as f32 / 1000.0)
}

/// Determine NIC interface name from hwmon device symlink.
/// e.g., hwmon device → PCI device → net/ethX
fn nic_interface_name(hwmon_path: &Path) -> Option<String> {
    let dev = std::fs::canonicalize(hwmon_path.join("device")).ok()?;
    // /sys/devices/.../net/<iface>
    let net = dev.join("net");
    std::fs::read_dir(&net).ok()?
        .flatten()
        .next()
        .map(|e| e.file_name().to_string_lossy().to_string())
}

/// Derive DDR5 slot label from spd5118 i2c device address.
fn dimm_slot_name(hwmon_path: &Path) -> String {
    // Device path target is something like: .../i2c-7/7-0051
    // Address 0x51 → slot 1, 0x50 → slot 0, etc.
    let canon = std::fs::canonicalize(hwmon_path.join("device")).ok();
    if let Some(d) = canon {
        let name = d.file_name().unwrap_or_default().to_string_lossy().to_string();
        // name = "7-0051"  (bus-address)
        if let Some(addr_str) = name.split('-').last() {
            if let Ok(addr) = u64::from_str_radix(addr_str, 16) {
                let slot = addr & 0x0f;
                return format!("DDR5 Slot {slot}");
            }
        }
    }
    "DDR5".into()
}

/// Intern dynamic strings as &'static str via a leak-once cache.
/// The set of unique sensor labels from sysfs is small and bounded.
fn intern(s: impl Into<String>) -> &'static str {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let s = s.into();
    let mut guard = CACHE.lock().unwrap();
    let set = guard.get_or_insert_with(HashSet::new);
    if let Some(&existing) = set.get(s.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    set.insert(leaked);
    leaked
}
