//! CPU core parking: take non-preferred CPUs offline via privileged helper.
//!
//! Mirrors Python cpu_park.py:
//!   - detect_topology(): AMD X3D (L3 cache asymmetry), Intel Hybrid (max freq), or UNIFORM
//!   - park_cpus() / unpark_all() via sudo /usr/local/bin/process-lasso-sysfs
//!   - get_smt_siblings_of(): reads /sys/.../topology/core_id
//!   - Topology cache: preserved across calls so Gaming Mode doesn't lose it once CPUs are parked

use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::Command;
use std::sync::Mutex;

use crate::utils::{cpuset_to_cpulist, get_offline_cpus, read_cpulist_file};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const HELPER: &str = "/usr/local/bin/process-lasso-sysfs";
pub const SUDOERS_FILE: &str = "/etc/sudoers.d/process-lasso";

pub const HELPER_CONTENT: &str = r#"#!/bin/bash
# Process Lasso privileged sysfs helper — managed by process-lasso.
set -euo pipefail
case "$1" in
    cpu-online)
        [[ "$2" =~ ^[0-9]+$ ]] || exit 1
        [[ "$3" =~ ^[01]$   ]] || exit 1
        echo "$3" > "/sys/devices/system/cpu/cpu$2/online"
        ;;
    cpu-unpark-all)
        offline=$(cat /sys/devices/system/cpu/offline 2>/dev/null || true)
        [ -z "$offline" ] && exit 0
        for part in $(echo "$offline" | tr ',' ' '); do
            if [[ "$part" == *-* ]]; then
                lo=${part%-*}; hi=${part#*-}
                for ((c=lo; c<=hi; c++)); do
                    echo 1 > "/sys/devices/system/cpu/cpu${c}/online" 2>/dev/null || true
                done
            else
                echo 1 > "/sys/devices/system/cpu/cpu${part}/online" 2>/dev/null || true
            fi
        done
        ;;
    renice-pid)
        [[ "$2" =~ ^-?[0-9]+$ ]] || exit 1
        [[ "$3" =~ ^[0-9]+$ ]]   || exit 1
        renice -n "$2" -p "$3"
        ;;
    *)
        echo "Unknown command: $1" >&2; exit 1 ;;
esac
"#;

// ── Topology ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum TopologyKind {
    AmdX3D,
    IntelHybrid,
    Uniform,
}

#[derive(Debug, Clone)]
pub struct CpuTopology {
    #[allow(dead_code)]
    pub kind: TopologyKind,
    pub preferred: HashSet<u32>,
    pub non_preferred: HashSet<u32>,
    pub description: String,
}

impl CpuTopology {
    pub fn uniform(all_cpus: HashSet<u32>) -> Self {
        Self {
            kind: TopologyKind::Uniform,
            preferred: all_cpus,
            non_preferred: HashSet::new(),
            description: "Uniform topology (no asymmetry detected). All CPUs equal.".into(),
        }
    }

    pub fn has_asymmetry(&self) -> bool {
        !self.non_preferred.is_empty()
    }
}

// ── Topology cache ────────────────────────────────────────────────────────────
// Once we detect an asymmetric topology, we preserve it even after Gaming Mode
// parks one CCD (making sysfs entries for those CPUs unreadable).

static TOPO_CACHE: Mutex<Option<CpuTopology>> = Mutex::new(None);

// ── Detection ─────────────────────────────────────────────────────────────────

/// Auto-detect CPU topology. Tries AMD X3D first, then Intel Hybrid.
/// Caches asymmetric results so topology survives CPU parking.
pub fn detect_topology() -> CpuTopology {
    if let Some(topo) = detect_amd_x3d() {
        if topo.has_asymmetry() {
            *TOPO_CACHE.lock().unwrap() = Some(topo.clone());
            return topo;
        }
    }
    if let Some(topo) = detect_intel_hybrid() {
        if topo.has_asymmetry() {
            *TOPO_CACHE.lock().unwrap() = Some(topo.clone());
            return topo;
        }
    }
    // If live detection is UNIFORM but we have a cached asymmetric result
    // (e.g. Gaming Mode already parked one CCD), return the cache.
    if let Some(cached) = TOPO_CACHE.lock().unwrap().clone() {
        if cached.has_asymmetry() {
            return cached;
        }
    }
    let all = present_cpus();
    CpuTopology::uniform(all)
}

fn present_cpus() -> HashSet<u32> {
    read_cpulist_file("/sys/devices/system/cpu/present")
        .unwrap_or_else(|| {
            let n = std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1);
            (0..n).collect()
        })
}

/// Detect AMD X3D: preferred CCD has larger L3 (3D V-Cache).
fn detect_amd_x3d() -> Option<CpuTopology> {
    let present = present_cpus();
    let offline = get_offline_cpus();

    // Read L3 cache sizes for all present CPUs
    let mut l3: HashMap<u32, u64> = HashMap::new();
    for cpu in &present {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/cache/index3/size");
        if let Ok(raw) = fs::read_to_string(&path) {
            let raw = raw.trim();
            let kb: u64 = if let Some(s) = raw.strip_suffix('K') {
                s.parse().ok()?
            } else if let Some(s) = raw.strip_suffix('M') {
                let mb: u64 = s.parse().ok()?;
                mb * 1024
            } else {
                raw.parse().ok()?
            };
            l3.insert(*cpu, kb);
        }
        // offline CPUs have no sysfs entry — silently skip
    }

    if l3.is_empty() {
        return None;
    }

    let sizes: HashSet<u64> = l3.values().copied().collect();
    if sizes.len() <= 1 {
        // All readable CPUs have the same L3.
        // If there are offline CPUs, the other CCD is already parked by Gaming Mode.
        if !offline.is_empty() {
            let online_kb = *sizes.iter().next().unwrap();
            let online_set: HashSet<u32> = l3.keys().copied().collect();
            return Some(CpuTopology {
                kind: TopologyKind::AmdX3D,
                preferred: online_set.clone(),
                non_preferred: offline.clone(),
                description: format!(
                    "AMD X3D detected (other CCD currently parked). \
                     Preferred (V-Cache, {}MB L3): CPUs {}. Non-preferred (parked): CPUs {}.",
                    online_kb / 1024,
                    cpuset_to_cpulist(&online_set),
                    cpuset_to_cpulist(&offline),
                ),
            });
        }
        return None; // genuine uniform L3
    }

    let max_kb = *sizes.iter().max().unwrap();
    let min_kb = *sizes.iter().min().unwrap();
    let preferred: HashSet<u32> = l3.iter().filter(|(_, &s)| s == max_kb).map(|(&c, _)| c).collect();
    let non_preferred: HashSet<u32> = l3.iter().filter(|(_, &s)| s == min_kb).map(|(&c, _)| c).collect();

    Some(CpuTopology {
        kind: TopologyKind::AmdX3D,
        preferred: preferred.clone(),
        non_preferred: non_preferred.clone(),
        description: format!(
            "AMD X3D detected. Preferred (V-Cache, {}MB L3): CPUs {}. Non-preferred ({}MB L3): CPUs {}.",
            max_kb / 1024,
            cpuset_to_cpulist(&preferred),
            min_kb / 1024,
            cpuset_to_cpulist(&non_preferred),
        ),
    })
}

/// Detect Intel Hybrid: P-cores have higher max freq than E-cores.
fn detect_intel_hybrid() -> Option<CpuTopology> {
    let present = present_cpus();
    let mut max_freq: HashMap<u32, u64> = HashMap::new();

    for cpu in &present {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq");
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(f) = raw.trim().parse::<u64>() {
                max_freq.insert(*cpu, f);
            }
        }
    }

    if max_freq.is_empty() {
        return None;
    }

    let freqs: HashSet<u64> = max_freq.values().copied().collect();
    if freqs.len() <= 1 {
        return None; // uniform max freq
    }

    let max_f = *freqs.iter().max().unwrap();
    let min_f = *freqs.iter().min().unwrap();
    // P-cores = anything ≥ 80% of max freq (handles slight variance)
    let threshold = (max_f as f64 * 0.80) as u64;
    let preferred: HashSet<u32> = max_freq.iter().filter(|(_, &f)| f >= threshold).map(|(&c, _)| c).collect();
    let non_preferred: HashSet<u32> = max_freq.iter().filter(|(_, &f)| f < threshold).map(|(&c, _)| c).collect();

    Some(CpuTopology {
        kind: TopologyKind::IntelHybrid,
        preferred: preferred.clone(),
        non_preferred: non_preferred.clone(),
        description: format!(
            "Intel Hybrid detected. P-cores ({:.1} GHz max): CPUs {}. E-cores ({:.1} GHz max): CPUs {}.",
            max_f as f64 / 1_000_000.0,
            cpuset_to_cpulist(&preferred),
            min_f as f64 / 1_000_000.0,
            cpuset_to_cpulist(&non_preferred),
        ),
    })
}

// ── SMT sibling detection ─────────────────────────────────────────────────────

/// Return the SMT sibling threads within a set of CPUs.
/// For each physical core with 2+ logical CPUs, all but the lowest-numbered are siblings.
pub fn get_smt_siblings_of(cpus: &HashSet<u32>) -> HashSet<u32> {
    let mut core_to_logical: HashMap<u32, Vec<u32>> = HashMap::new();
    for &cpu in cpus {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/core_id");
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(core_id) = raw.trim().parse::<u32>() {
                core_to_logical.entry(core_id).or_default().push(cpu);
            }
        }
    }
    let mut siblings = HashSet::new();
    for mut logical_cpus in core_to_logical.into_values() {
        if logical_cpus.len() >= 2 {
            logical_cpus.sort_unstable();
            let primary = logical_cpus[0];
            for &c in &logical_cpus[1..] {
                if c != primary {
                    siblings.insert(c);
                }
            }
        }
    }
    siblings
}

// ── Helper check ─────────────────────────────────────────────────────────────

pub fn is_helper_installed() -> bool {
    std::path::Path::new(HELPER).exists()
        && std::fs::metadata(HELPER)
            .ok()
            .map(|m| {
                use std::os::unix::fs::PermissionsExt;
                m.permissions().mode() & 0o111 != 0
            })
            .unwrap_or(false)
}

pub fn is_helper_current() -> bool {
    if !is_helper_installed() {
        return false;
    }
    fs::read_to_string(HELPER)
        .map(|s| s.contains("renice-pid"))
        .unwrap_or(false)
}

/// Check whether the sudoers NOPASSWD rule is in place by doing a dry-run sudo.
pub fn is_sudoers_installed() -> bool {
    if !is_helper_installed() {
        return false;
    }
    Command::new("sudo")
        .args(["-n", HELPER, "--check-only"])
        .output()
        .map(|o| matches!(o.status.code(), Some(0) | Some(1)))
        .unwrap_or(false)
}

// ── Park / Unpark ─────────────────────────────────────────────────────────────

fn run_helper(args: &[&str]) -> (bool, String) {
    if !is_helper_installed() {
        return (false, "Helper not installed. Run install first.".into());
    }
    let mut cmd = Command::new("sudo");
    cmd.arg(HELPER);
    for a in args {
        cmd.arg(a);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => (true, String::new()),
        Ok(o) => {
            let msg = String::from_utf8_lossy(&o.stderr).trim().to_string();
            (false, if msg.is_empty() { String::from_utf8_lossy(&o.stdout).trim().to_string() } else { msg })
        }
        Err(e) => (false, e.to_string()),
    }
}

/// Take CPUs offline. Returns true if all succeeded.
pub fn park_cpus(cpus: &HashSet<u32>, log_cb: impl Fn(String)) -> bool {
    if cpus.is_empty() {
        return true;
    }
    let mut ok = true;
    let mut sorted: Vec<u32> = cpus.iter().copied().collect();
    sorted.sort_unstable();
    for cpu in sorted {
        if cpu == 0 {
            log_cb(format!("[Park] Skipping CPU 0 (bootstrap processor, cannot offline)"));
            continue;
        }
        let (success, msg) = run_helper(&["cpu-online", &cpu.to_string(), "0"]);
        if success {
            log_cb(format!("[Park] CPU {cpu} → offline"));
        } else {
            log::warn!("park cpu{cpu} failed: {msg}");
            log_cb(format!("[Park] CPU {cpu} FAILED: {msg}"));
            ok = false;
        }
    }
    ok
}

/// Bring all offline CPUs back online.
pub fn unpark_all(log_cb: impl Fn(String)) -> bool {
    let offline = get_offline_cpus();
    if offline.is_empty() {
        log_cb("[Park] No offline CPUs to restore.".into());
        return true;
    }
    let (success, msg) = run_helper(&["cpu-unpark-all"]);
    if success {
        log_cb(format!("[Park] CPUs {:?} restored online.", {
            let mut v: Vec<u32> = offline.iter().copied().collect();
            v.sort_unstable();
            v
        }));
        true
    } else {
        log::warn!("unpark-all failed: {msg}");
        log_cb(format!("[Park] Unpark all FAILED: {msg}"));
        false
    }
}

/// Set process nice value via privileged helper (required for negative nice).
pub fn set_process_nice_via_helper(pid: u32, nice: i32) -> bool {
    let (ok, msg) = run_helper(&["renice-pid", &nice.to_string(), &pid.to_string()]);
    if !ok {
        log::warn!("renice-pid pid={pid} nice={nice} failed: {msg}");
    }
    ok
}

// ── Helper installation ───────────────────────────────────────────────────────

/// Write helper + sudoers rule via `su root` with a PTY.
/// Returns (ok, message).
pub fn install_helper_as_root(username: &str, password: &str) -> (bool, String) {
    use std::io::Write;

    let username = if username.is_empty() {
        std::env::var("USER").unwrap_or_default()
    } else {
        username.to_string()
    };
    if username.is_empty() {
        return (false, "Could not determine current username.".into());
    }
    if password.is_empty() {
        return (false, "No root password provided.".into());
    }

    let sudoers_line = format!("{username} ALL=(root) NOPASSWD: {HELPER}");

    // Write helper to /tmp
    let tmp = "/tmp/pl-sysfs.tmp";
    if let Err(e) = fs::write(tmp, HELPER_CONTENT) {
        return (false, format!("Failed to write tmp helper: {e}"));
    }
    // Set executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(tmp, fs::Permissions::from_mode(0o755));
    }

    let cmd = format!(
        "cp {tmp} {HELPER} && \
         chmod 755 {HELPER} && \
         chown root:root {HELPER} && \
         printf '%s\\n' '{sudoers_line}' > {SUDOERS_FILE} && \
         chmod 440 {SUDOERS_FILE} && \
         echo INSTALL_OK"
    );

    // Use `su -c` and pipe the password via stdin on a PTY
    // We use the `script` approach: spawn `su root -c cmd`, feed password
    let output = Command::new("su")
        .args(["root", "-c", &cmd])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    match output {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = writeln!(stdin, "{password}");
            }
            match child.wait_with_output() {
                Ok(o) => {
                    let out = String::from_utf8_lossy(&o.stdout);
                    let err = String::from_utf8_lossy(&o.stderr);
                    let combined = format!("{out}{err}");
                    if combined.contains("INSTALL_OK") {
                        (true, "Helper and sudoers rule installed.".into())
                    } else {
                        (
                            false,
                            format!(
                                "Install failed (rc={:?}): {}",
                                o.status.code(),
                                combined.trim().chars().rev().take(300).collect::<String>().chars().rev().collect::<String>()
                            ),
                        )
                    }
                }
                Err(e) => (false, format!("su wait failed: {e}")),
            }
        }
        Err(e) => (false, format!("su spawn failed: {e}")),
    }
}
