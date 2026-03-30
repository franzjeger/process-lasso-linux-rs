//! Monitor daemon thread: process scanning, rule enforcement, ProBalance.
//!
//! Mirrors Python monitor.py MonitorThread:
//!   - 0.1s base tick
//!   - Every 0.5s (rule_enforce_interval_ms): enforce all rules on running processes
//!   - Every 1.0s: ProBalance tick
//!   - Every 2.0s (display_refresh_interval_ms): update AppState snapshot
//!   - New PIDs: apply matching rules or default affinity
//!   - Gaming Mode: nice -1 via helper for rule-matched processes
//!   - Manual affinity override: 30s suppression after user sets affinity

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;

use crate::config::Config;
use crate::cpu_park;
use crate::hw_monitor::{HwCollector, HwMonitorData};
use crate::probalance::{ProBalance, ProcSnapshot};
use crate::rules::RuleEngine;
use crate::utils;

// ── Commands from GUI → daemon ────────────────────────────────────────────────

#[derive(Debug)]
pub enum DaemonCmd {
    UpdateConfig(Config),
    SetGamingMode { active: bool, elevate_nice: bool, park: bool },
    SetManualOverride { pid: u32, duration_secs: f64 },
    ResetAffinities,
    ReapplyDefaults,
}

// ── Shared state (GUI reads this) ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub mem_rss: u64,   // bytes
    pub nice: i32,
    pub affinity: String,
    pub ionice: String,
    pub disk_read_bps: u64,   // bytes/s
    pub disk_write_bps: u64,  // bytes/s
    /// Reference-counted so GUI snapshot clones are O(1) for this field.
    pub cmdline: std::sync::Arc<String>,
}

impl Default for ProcInfo {
    fn default() -> Self {
        Self {
            pid: 0,
            ppid: 0,
            name: String::new(),
            cpu_percent: 0.0,
            mem_rss: 0,
            nice: 0,
            affinity: String::new(),
            ionice: String::new(),
            disk_read_bps: 0,
            disk_write_bps: 0,
            cmdline: std::sync::Arc::new(String::new()),
        }
    }
}

#[derive(Debug, Default)]
pub struct AppState {
    pub snapshot: Vec<ProcInfo>,
    /// Per-CPU utilisation % (indexed by cpu number, parked CPUs = 0.0)
    pub cpu_percents: Vec<f32>,
    /// Monotonic counter incremented each time cpu_percents is updated by the daemon.
    /// GUI tracks this to avoid pushing duplicate samples to the history widget.
    pub cpu_generation: u64,
    /// Rolling average CPU history (120 samples)
    pub cpu_history: std::collections::VecDeque<f32>,
    /// Throttled PID set from ProBalance
    pub throttled_pids: HashSet<u32>,
    /// Detailed throttle info for ProBalance tab live view
    pub throttle_infos: Vec<crate::probalance::ThrottleInfo>,
    /// Log lines ring buffer (max 2000)
    pub log_lines: std::collections::VecDeque<String>,
    /// Current config (read by GUI for settings display)
    pub config: Config,
    /// Is Gaming Mode currently active?
    pub gaming_active: bool,
    /// Hardware sensor data (updated every display_refresh_interval)
    pub hw_monitor: HwMonitorData,
    /// System-wide average CPU % (used by tray tooltip)
    pub cpu_avg: f32,
    /// Per-PID CPU usage history (last 30 samples)
    pub proc_cpu_history: HashMap<u32, std::collections::VecDeque<f32>>,
    /// CPU model string from /proc/cpuinfo
    pub cpu_model: String,
    /// PIDs manually suspended via SIGSTOP from the GUI
    pub suspended_pids: std::collections::HashSet<u32>,
}

pub fn read_cpu_model() -> String {
    std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .and_then(|l| l.splitn(2, ':').nth(1))
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "Unknown CPU".to_string())
}

impl AppState {
    pub fn append_log(&mut self, msg: String) {
        let ts = chrono_ts();
        self.log_lines.push_back(format!("[{ts}] {msg}"));
        while self.log_lines.len() > 2000 {
            self.log_lines.pop_front();
        }
    }
}

fn chrono_ts() -> String {
    // Simple HH:MM:SS without pulling in chrono (use std only)
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

// ── Daemon thread ─────────────────────────────────────────────────────────────

pub fn spawn(
    state: Arc<Mutex<AppState>>,
    cmd_rx: Receiver<DaemonCmd>,
    initial_config: Config,
    rule_engine: Arc<Mutex<RuleEngine>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        run_loop(state, cmd_rx, initial_config, rule_engine);
    })
}

fn run_loop(
    state: Arc<Mutex<AppState>>,
    cmd_rx: Receiver<DaemonCmd>,
    initial_config: Config,
    rule_engine: Arc<Mutex<RuleEngine>>,
) {
    let mut config = initial_config;

    // Build closures that push log messages into shared state
    let state_log = state.clone();
    let log_cb = move |msg: String| {
        if let Ok(mut s) = state_log.lock() {
            s.append_log(msg);
        }
    };

    let mut probalance = ProBalance::new(config.probalance.clone());
    let mut hw_collector = HwCollector::new();
    let log_cb2 = log_cb.clone();
    probalance.set_log_callback(move |m| log_cb2(m));

    {
        let log_cb3 = log_cb.clone();
        if let Ok(mut re) = rule_engine.lock() {
            re.set_log_callback(move |m| log_cb3(m));
        }
    }

    // Startup log entry so users can see the log is working
    log_cb(format!(
        "Argus-Lasso started — ProBalance: {}  |  Display refresh: {}ms  |  Rule enforce: {}ms",
        if config.probalance.enabled { "on" } else { "off" },
        config.monitor.display_refresh_interval_ms,
        config.monitor.rule_enforce_interval_ms,
    ));

    let mut known_pids: HashSet<u32> = HashSet::new();
    let mut first_snapshot = true;
    // Track previously throttled PIDs for change-based notifications
    let mut prev_throttled: HashSet<u32> = HashSet::new();
    // pid → original affinity set before we changed it; pruned every snapshot cycle
    let mut original_affinities: HashMap<u32, HashSet<u32>> = HashMap::new();
    // pid → expiry Instant (suppress rule re-enforcement after manual change)
    let mut manual_overrides: HashMap<u32, Instant> = HashMap::new();
    // Gaming Mode nice tracking: pid → original nice before we elevated
    let mut gaming_mode = false;
    let mut gaming_elevate_nice = false;
    let mut gaming_niced: HashMap<u32, i32> = HashMap::new();
    // Disk I/O tracking: pid → (read_bytes, write_bytes) at last sample
    let mut prev_io: HashMap<u32, (u64, u64)> = HashMap::new();
    // HW alert cooldown: sensor_label → last alert time
    let mut last_alert_times: HashMap<String, Instant> = HashMap::new();

    let tick = Duration::from_millis(500);
    let mut last_enforce = Instant::now();
    let mut last_pb = Instant::now();
    let mut last_snapshot = Instant::now();
    let mut last_pb_tick = Instant::now();

    // CPU percentage tracking: previous jiffies per process for delta
    let mut prev_cpu_times: HashMap<u32, u64> = HashMap::new();
    let mut prev_sys_total: u64 = 0;
    // Cached snapshot — rebuilt only on enforce/display cadence
    let mut raw_snapshot: Vec<ProcInfo> = Vec::new();

    loop {
        // ── Drain commands from GUI ─────────────────────────────────────────
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                DaemonCmd::UpdateConfig(cfg) => {
                    probalance.update_config(cfg.probalance.clone());
                    config = cfg.clone();
                    log_cb(format!(
                        "Config updated — ProBalance: {}  |  Notifications: {}",
                        if config.probalance.enabled { "on" } else { "off" },
                        if config.ui.notifications_enabled { "on" } else { "off" },
                    ));
                    if let Ok(mut s) = state.lock() {
                        s.config = cfg;
                    }
                }
                DaemonCmd::SetGamingMode { active, elevate_nice, park } => {
                    gaming_mode = active;
                    gaming_elevate_nice = elevate_nice;
                    if !active && !gaming_niced.is_empty() {
                        restore_gaming_nices(&mut gaming_niced, &log_cb);
                    }
                    if let Ok(mut s) = state.lock() {
                        s.gaming_active = active;
                    }
                    if park {
                        if active {
                            let topo = cpu_park::detect_topology();
                            if topo.has_asymmetry() && cpu_park::is_helper_installed() {
                                let to_park: HashSet<u32> = topo.non_preferred.iter().copied().collect();
                                log_cb(format!("[Gaming Mode] Parking CPUs {:?}…", {
                                    let mut v: Vec<_> = to_park.iter().copied().collect();
                                    v.sort_unstable();
                                    v
                                }));
                                if cpu_park::park_cpus(&to_park, |msg| log_cb(msg)) {
                                    log_cb("[Gaming Mode] ACTIVE — non-preferred CPUs offline.".into());
                                } else {
                                    log_cb("[Gaming Mode] Parking failed — check log.".into());
                                }
                            }
                        } else {
                            log_cb("[Gaming Mode] Unparking all CPUs…".into());
                            cpu_park::unpark_all(|msg| log_cb(msg));
                            log_cb("[Gaming Mode] Disabled — all CPUs online.".into());
                        }
                    }
                }
                DaemonCmd::SetManualOverride { pid, duration_secs } => {
                    manual_overrides.insert(
                        pid,
                        Instant::now() + Duration::from_secs_f64(duration_secs),
                    );
                }
                DaemonCmd::ResetAffinities => {
                    reset_all_affinities(&mut original_affinities, &log_cb);
                }
                DaemonCmd::ReapplyDefaults => {
                    reapply_defaults(&config, &rule_engine, &known_pids, &log_cb);
                }
            }
        }

        let now = Instant::now();
        let enforce_interval = Duration::from_millis(config.monitor.rule_enforce_interval_ms);
        let needs_snapshot = now.duration_since(last_enforce) >= enforce_interval
            || now.duration_since(last_snapshot) >= Duration::from_millis(config.monitor.display_refresh_interval_ms)
            || now.duration_since(last_pb) >= Duration::from_secs(1);

        // ── Collect process snapshot (only when needed) ─────────────────────
        if needs_snapshot {
            let (new_snapshot, new_cpu_times, sys_total) =
                collect_snapshot(&mut prev_cpu_times, prev_sys_total, &mut prev_io);
            prev_cpu_times = new_cpu_times;
            prev_sys_total = sys_total;
            raw_snapshot = new_snapshot;

            let current_pids: HashSet<u32> = raw_snapshot.iter().map(|p| p.pid).collect();

            // Prune dead PIDs from original_affinities to avoid unbounded growth
            original_affinities.retain(|pid, _| current_pids.contains(pid));

            // ── New PIDs: apply rules or default affinity ───────────────────
            let new_pids: HashSet<u32> = current_pids.difference(&known_pids).copied().collect();
            if !new_pids.is_empty() {
                for proc in raw_snapshot.iter().filter(|p| new_pids.contains(&p.pid)) {
                    apply_new_pid(
                        proc,
                        &config,
                        &rule_engine,
                        &mut original_affinities,
                        gaming_mode,
                        gaming_elevate_nice,
                        &mut gaming_niced,
                        &log_cb,
                    );
                }
            }
            if first_snapshot {
                log_cb(format!("Initial scan: {} processes found.", raw_snapshot.len()));
                first_snapshot = false;
            }
            known_pids = current_pids;
        }

        // ── Rule enforcement every enforce_interval ─────────────────────────
        if now.duration_since(last_enforce) >= enforce_interval {
            // Expire stale manual overrides
            manual_overrides.retain(|_, exp| *exp > now);
            if let Ok(re) = rule_engine.lock() {
                for proc in &raw_snapshot {
                    if manual_overrides.contains_key(&proc.pid) {
                        continue;
                    }
                    re.apply_to_process(proc.pid, &proc.name);
                }
            }
            last_enforce = now;
        }

        // ── ProBalance every 1s ────────────────────────────────────────────
        if now.duration_since(last_pb) >= Duration::from_secs(1) {
            let pb_tick = now.duration_since(last_pb_tick).as_secs_f32();
            last_pb_tick = now;
            let pb_snap: Vec<ProcSnapshot> = raw_snapshot
                .iter()
                .map(|p| ProcSnapshot {
                    pid: p.pid,
                    name: p.name.clone(),
                    cpu_percent: p.cpu_percent,
                    nice: p.nice,
                })
                .collect();
            probalance.tick(&pb_snap, pb_tick);

            // Fire desktop notifications for newly throttled / restored PIDs
            let cur_throttled = probalance.throttled_pids();
            if cur_throttled != prev_throttled && config.ui.notifications_enabled {
                // Build a name lookup from the current snapshot
                let name_map: HashMap<u32, &str> = raw_snapshot.iter()
                    .map(|p| (p.pid, p.name.as_str()))
                    .collect();

                // Newly throttled
                for &pid in cur_throttled.difference(&prev_throttled) {
                    let name = name_map.get(&pid).copied().unwrap_or("unknown");
                    let _ = notify_rust::Notification::new()
                        .summary("ProBalance")
                        .body(&format!("Throttled: {name} (PID {pid})"))
                        .timeout(notify_rust::Timeout::Milliseconds(3000))
                        .show();
                }
                // Restored
                for &pid in prev_throttled.difference(&cur_throttled) {
                    let name = name_map.get(&pid).copied().unwrap_or("unknown");
                    let _ = notify_rust::Notification::new()
                        .summary("ProBalance")
                        .body(&format!("Restored: {name} (PID {pid})"))
                        .timeout(notify_rust::Timeout::Milliseconds(3000))
                        .show();
                }
            }
            prev_throttled = cur_throttled;

            last_pb = now;
        }

        // ── Snapshot emit every display_refresh_interval ───────────────────
        let refresh = Duration::from_millis(config.monitor.display_refresh_interval_ms);
        if now.duration_since(last_snapshot) >= refresh {
            let throttled = probalance.throttled_pids();
            let pb_snap_for_infos: Vec<crate::probalance::ProcSnapshot> = raw_snapshot.iter()
                .map(|p| crate::probalance::ProcSnapshot {
                    pid: p.pid, name: p.name.clone(),
                    cpu_percent: p.cpu_percent, nice: p.nice,
                })
                .collect();
            let throttle_infos = probalance.throttle_infos(&pb_snap_for_infos);
            let cpu_percents = collect_cpu_percents();
            let avg = if cpu_percents.is_empty() {
                0.0
            } else {
                cpu_percents.iter().sum::<f32>() / cpu_percents.len() as f32
            };

            // Update hardware sensor readings
            hw_collector.update();

            // Check temperature alerts
            check_hw_alerts(
                &hw_collector.data,
                &config.hw_alerts,
                config.ui.notifications_enabled,
                &mut last_alert_times,
                &log_cb,
            );

            if let Ok(mut s) = state.lock() {
                s.snapshot = raw_snapshot.clone();
                s.cpu_percents = cpu_percents;
                s.cpu_generation = s.cpu_generation.wrapping_add(1);
                s.throttled_pids = throttled;
                s.throttle_infos = throttle_infos;
                s.cpu_avg = avg;
                s.cpu_history.push_back(avg);
                while s.cpu_history.len() > 120 {
                    s.cpu_history.pop_front();
                }
                s.hw_monitor = hw_collector.data.clone();
                // Update per-PID CPU history
                let current_pids: std::collections::HashSet<u32> = raw_snapshot.iter().map(|p| p.pid).collect();
                for p in &raw_snapshot {
                    let hist = s.proc_cpu_history.entry(p.pid).or_insert_with(|| std::collections::VecDeque::with_capacity(30));
                    hist.push_back(p.cpu_percent);
                    while hist.len() > 30 { hist.pop_front(); }
                }
                s.proc_cpu_history.retain(|pid, _| current_pids.contains(pid));
            }
            last_snapshot = now;
        }

        std::thread::sleep(tick);
    }
}

// ── Process collection ────────────────────────────────────────────────────────

fn collect_snapshot(
    prev_times: &mut HashMap<u32, u64>,
    prev_sys_total: u64,
    prev_io: &mut HashMap<u32, (u64, u64)>,
) -> (Vec<ProcInfo>, HashMap<u32, u64>, u64) {
    use procfs::process::all_processes;
    use procfs::WithCurrentSystemInfo;

    let mut new_times: HashMap<u32, u64> = HashMap::new();
    let mut new_io: HashMap<u32, (u64, u64)> = HashMap::new();
    let mut snapshot: Vec<ProcInfo> = Vec::new();

    // Read total system CPU jiffies for CPU% calculation
    let sys_total = read_sys_cpu_total();
    let sys_delta = sys_total.saturating_sub(prev_sys_total) as f32;

    let procs = match all_processes() {
        Ok(p) => p,
        Err(_) => return (snapshot, new_times, sys_total),
    };

    for proc_result in procs {
        let proc = match proc_result {
            Ok(p) => p,
            Err(_) => continue,
        };

        let pid = proc.pid() as u32;

        let stat = match proc.stat() {
            Ok(s) => s,
            Err(_) => continue,
        };

        let ppid = stat.ppid as u32;
        let comm = stat.comm.clone();
        let cmdline: Vec<String> = proc.cmdline().unwrap_or_default();
        let name = utils::resolve_name(&comm, &cmdline);

        let proc_ticks = stat.utime + stat.stime;
        new_times.insert(pid, proc_ticks);
        let prev_ticks = prev_times.get(&pid).copied().unwrap_or(proc_ticks);
        let delta_ticks = proc_ticks.saturating_sub(prev_ticks) as f32;
        let cpu_percent = if sys_delta > 0.0 {
            (delta_ticks / sys_delta * 100.0).min(100.0)
        } else {
            0.0
        };

        let mem_rss = stat.rss_bytes().get();
        let nice = stat.nice as i32;
        let affinity = utils::get_affinity_str(pid);
        let ionice = read_ionice(pid);

        // Disk I/O — read from /proc/<pid>/io; ignore permission errors
        let (disk_read_bps, disk_write_bps) = read_proc_io(pid, prev_io, &mut new_io);

        snapshot.push(ProcInfo {
            pid,
            ppid,
            name,
            cpu_percent,
            mem_rss,
            nice,
            affinity,
            ionice,
            disk_read_bps,
            disk_write_bps,
            cmdline: std::sync::Arc::new(cmdline.join(" ")),
        });
    }

    *prev_io = new_io;
    (snapshot, new_times, sys_total)
}

fn read_proc_io(
    pid: u32,
    prev_io: &HashMap<u32, (u64, u64)>,
    new_io: &mut HashMap<u32, (u64, u64)>,
) -> (u64, u64) {
    let text = match std::fs::read_to_string(format!("/proc/{pid}/io")) {
        Ok(t) => t,
        Err(_) => return (0, 0),
    };
    let mut read_bytes = 0u64;
    let mut write_bytes = 0u64;
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("read_bytes: ") {
            read_bytes = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("write_bytes: ") {
            write_bytes = v.trim().parse().unwrap_or(0);
        }
    }
    let (prev_r, prev_w) = prev_io.get(&pid).copied().unwrap_or((read_bytes, write_bytes));
    new_io.insert(pid, (read_bytes, write_bytes));
    (
        read_bytes.saturating_sub(prev_r),
        write_bytes.saturating_sub(prev_w),
    )
}

fn read_sys_cpu_total() -> u64 {
    // Read first line of /proc/stat: cpu  user nice system idle iowait irq softirq ...
    if let Ok(text) = std::fs::read_to_string("/proc/stat") {
        if let Some(line) = text.lines().next() {
            return line
                .split_whitespace()
                .skip(1)
                .filter_map(|s| s.parse::<u64>().ok())
                .sum();
        }
    }
    0
}

fn collect_cpu_percents() -> Vec<f32> {
    // Read per-CPU utilisation from procfs
    // We use a static to store previous values
    use std::sync::Mutex as StdMutex;

    static PREV: StdMutex<Option<(Vec<[u64; 10]>, std::time::Instant)>> = StdMutex::new(None);

    let total_cpus = utils::get_cpu_count() as usize;
    let online = utils::get_online_cpus();

    let new_stats = read_percpu_stats();
    let mut result = vec![0.0f32; total_cpus];

    let mut prev_guard = PREV.lock().unwrap();
    if let Some((ref prev_stats, _)) = *prev_guard {
        for (cpu_idx, (prev, new)) in prev_stats.iter().zip(new_stats.iter()).enumerate() {
            // Fields: user nice system idle iowait irq softirq steal guest guest_nice
            let prev_total: u64 = prev.iter().sum();
            let new_total: u64 = new.iter().sum();
            let total_delta = new_total.saturating_sub(prev_total) as f32;
            let idle_delta = new[3].saturating_sub(prev[3]) as f32;
            let pct = if total_delta > 0.0 {
                ((total_delta - idle_delta) / total_delta * 100.0).clamp(0.0, 100.0)
            } else {
                0.0
            };
            // Map procfs cpu index to actual cpu number (only online CPUs)
            let mut online_sorted: Vec<u32> = online.iter().copied().collect();
            online_sorted.sort_unstable();
            if let Some(&cpu_num) = online_sorted.get(cpu_idx) {
                if (cpu_num as usize) < total_cpus {
                    result[cpu_num as usize] = pct;
                }
            }
        }
    }
    *prev_guard = Some((new_stats, std::time::Instant::now()));
    result
}

fn read_percpu_stats() -> Vec<[u64; 10]> {
    let mut result = Vec::new();
    if let Ok(text) = std::fs::read_to_string("/proc/stat") {
        for line in text.lines() {
            if line.starts_with("cpu") && line.len() > 3 && line.as_bytes()[3].is_ascii_digit() {
                let mut fields = [0u64; 10];
                for (i, tok) in line.split_whitespace().skip(1).enumerate() {
                    if i < 10 {
                        fields[i] = tok.parse().unwrap_or(0);
                    }
                }
                result.push(fields);
            }
        }
    }
    result
}

fn read_ionice(pid: u32) -> String {
    // Read /proc/<pid>/io_prio or use ioprio_get syscall via nix
    // For display, we use the raw ioprio value decoded
    use nix::libc;
    let prio = unsafe {
        libc::syscall(libc::SYS_ioprio_get, 1 /* IOPRIO_WHO_PROCESS */, pid as libc::c_int)
    };
    if prio < 0 {
        return String::new();
    }
    let class = (prio as u32 >> 13) & 0x7;
    let level = prio as u32 & 0x1fff;
    format!("{class}/{level}")
}

// ── New PID handling ──────────────────────────────────────────────────────────

fn apply_new_pid(
    proc: &ProcInfo,
    config: &Config,
    rule_engine: &Arc<Mutex<RuleEngine>>,
    original_affinities: &mut HashMap<u32, HashSet<u32>>,
    gaming_mode: bool,
    gaming_elevate_nice: bool,
    gaming_niced: &mut HashMap<u32, i32>,
    log_cb: &impl Fn(String),
) {
    let pid = proc.pid;
    capture_original(pid, original_affinities);

    let actions = if let Ok(re) = rule_engine.lock() {
        re.apply_to_process(pid, &proc.name)
    } else {
        Vec::new()
    };

    if !actions.is_empty() {
        // Rule matched — if gaming mode + elevate_nice, apply nice -1 and pin to preferred cores
        if gaming_mode && gaming_elevate_nice && !gaming_niced.contains_key(&pid) {
            let orig_nice = proc.nice;
            if cpu_park::set_process_nice_via_helper(pid, -1) {
                gaming_niced.insert(pid, orig_nice);
                log_cb(format!("[Gaming Mode] nice -1 → {}({})", proc.name, pid));
            }
            // Pin game process to preferred cores (P-cores / V-Cache CCD)
            let topo = cpu_park::detect_topology();
            if topo.has_asymmetry() {
                let preferred_list = utils::cpuset_to_cpulist(&topo.preferred);
                if utils::set_affinity(pid, &preferred_list) {
                    log_cb(format!("[Gaming Mode] affinity → {} ({}) for {}({})",
                        topo.preferred_label, preferred_list, proc.name, pid));
                }
            }
        }
    } else {
        // No rule matched — apply default affinity if configured
        if let Some(ref default_aff) = config.cpu.default_affinity {
            if !default_aff.is_empty() {
                if utils::set_affinity(pid, default_aff) {
                    log_cb(format!("[Default] affinity={default_aff} → {}({pid})", proc.name));
                }
            }
        }
    }
}

fn capture_original(pid: u32, original_affinities: &mut HashMap<u32, HashSet<u32>>) {
    if original_affinities.contains_key(&pid) {
        return;
    }
    use nix::sched::{sched_getaffinity, CpuSet};
    use nix::unistd::Pid;
    if let Ok(cpu_set) = sched_getaffinity(Pid::from_raw(pid as i32)) {
        let mut cpus = HashSet::new();
        for i in 0..CpuSet::count() {
            if cpu_set.is_set(i).unwrap_or(false) {
                cpus.insert(i as u32);
            }
        }
        original_affinities.insert(pid, cpus);
    }
}

// ── Reset all affinities ──────────────────────────────────────────────────────

fn reset_all_affinities(
    original_affinities: &mut HashMap<u32, HashSet<u32>>,
    log_cb: &impl Fn(String),
) {
    use nix::sched::{sched_setaffinity, CpuSet};
    use nix::unistd::Pid;

    let online = utils::get_cpu_count();
    let all_cpus: HashSet<u32> = (0..online).collect();
    let mut count = 0;

    for (pid, orig) in original_affinities.iter() {
        let mask = if orig.is_empty() { &all_cpus } else { orig };
        let mut cpu_set = CpuSet::new();
        for &c in mask {
            let _ = cpu_set.set(c as usize);
        }
        if sched_setaffinity(Pid::from_raw(*pid as i32), &cpu_set).is_ok() {
            count += 1;
        }
        // Also reset all threads
        let tids = utils::get_tids(*pid);
        for tid in tids {
            if tid != *pid {
                let _ = sched_setaffinity(Pid::from_raw(tid as i32), &cpu_set);
            }
        }
    }
    original_affinities.clear();
    log_cb(format!(
        "[Reset] Restored affinity on {count} processes to original state."
    ));
}

// ── Reapply defaults ──────────────────────────────────────────────────────────

fn reapply_defaults(
    config: &Config,
    rule_engine: &Arc<Mutex<RuleEngine>>,
    known_pids: &HashSet<u32>,
    log_cb: &impl Fn(String),
) {
    let default_aff = match &config.cpu.default_affinity {
        Some(a) if !a.is_empty() => a.clone(),
        _ => return,
    };

    for &pid in known_pids {
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
            .unwrap_or_default();
        let comm = comm.trim();
        let cmdline_raw: Vec<String> = std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
            .unwrap_or_default()
            .split('\0')
            .map(|s| s.to_string())
            .collect();
        let name = utils::resolve_name(comm, &cmdline_raw);

        let actions = if let Ok(re) = rule_engine.lock() {
            re.apply_to_process(pid, &name)
        } else {
            Vec::new()
        };
        if actions.is_empty() {
            if utils::set_affinity(pid, &default_aff) {
                log_cb(format!("[Default] affinity={default_aff} → {name}({pid})"));
            }
        }
    }
}

// ── HW temperature alerts ────────────────────────────────────────────────────

fn check_hw_alerts(
    data: &HwMonitorData,
    cfg: &crate::config::HwAlertConfig,
    notifications_enabled: bool,
    last_alert: &mut HashMap<String, Instant>,
    log_cb: &impl Fn(String),
) {
    if !cfg.enabled {
        return;
    }
    let threshold = cfg.temp_threshold_celsius;
    let cooldown = Duration::from_secs(cfg.cooldown_secs);
    let now = Instant::now();

    for group in &data.groups {
        for sensor in &group.sensors {
            if sensor.unit != "°C" {
                continue;
            }
            if sensor.value >= threshold {
                let key = format!("{}/{}", group.name, sensor.label);
                let last = last_alert.get(&key).copied().unwrap_or(Instant::now() - cooldown - Duration::from_secs(1));
                if now.duration_since(last) >= cooldown {
                    last_alert.insert(key.clone(), now);
                    let msg = format!(
                        "[HW Alert] {} — {} {:.0}{}  (threshold: {:.0}°C)",
                        group.name, sensor.label, sensor.value, sensor.unit, threshold
                    );
                    log_cb(msg.clone());
                    if notifications_enabled {
                        let _ = notify_rust::Notification::new()
                            .summary("Argus-Lasso — Temperature Alert")
                            .body(&format!("{}: {:.0}°C (limit: {:.0}°C)", key, sensor.value, threshold))
                            .timeout(notify_rust::Timeout::Milliseconds(5000))
                            .show();
                    }
                }
            }
        }
    }
}

// ── Restore gaming nices ──────────────────────────────────────────────────────

fn restore_gaming_nices(gaming_niced: &mut HashMap<u32, i32>, log_cb: &impl Fn(String)) {
    let mut count = 0;
    for (&pid, &orig_nice) in gaming_niced.iter() {
        if cpu_park::set_process_nice_via_helper(pid, orig_nice) {
            count += 1;
        }
    }
    gaming_niced.clear();
    log_cb(format!("[Gaming Mode] Restored nice for {count} processes."));
}
