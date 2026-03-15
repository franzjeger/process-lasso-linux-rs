//! Low-level Linux helpers: CPU affinity, nice, ionice, cpulist parsing.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::Command;

// ── cpulist parsing / formatting ──────────────────────────────────────────────

/// Parse "0-7,16-23" → {0,1,2,3,4,5,6,7,16,17,18,19,20,21,22,23}
pub fn cpulist_to_set(cpulist: &str) -> Result<HashSet<u32>, String> {
    let mut result = HashSet::new();
    let trimmed = cpulist.trim();
    if trimmed.is_empty() {
        return Ok(result);
    }
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((lo, hi)) = part.split_once('-') {
            let lo: u32 = lo.trim().parse().map_err(|e| format!("bad cpulist range '{part}': {e}"))?;
            let hi: u32 = hi.trim().parse().map_err(|e| format!("bad cpulist range '{part}': {e}"))?;
            for c in lo..=hi {
                result.insert(c);
            }
        } else {
            let n: u32 = part.parse().map_err(|e| format!("bad cpulist item '{part}': {e}"))?;
            result.insert(n);
        }
    }
    Ok(result)
}

/// Convert {0,1,2,3,5} → "0-3,5"
pub fn cpuset_to_cpulist(cpus: &HashSet<u32>) -> String {
    if cpus.is_empty() {
        return String::new();
    }
    let mut sorted: Vec<u32> = cpus.iter().copied().collect();
    sorted.sort_unstable();

    let mut ranges: Vec<String> = Vec::new();
    let mut start = sorted[0];
    let mut end = sorted[0];

    for &c in &sorted[1..] {
        if c == end + 1 {
            end = c;
        } else {
            ranges.push(if start == end {
                start.to_string()
            } else {
                format!("{start}-{end}")
            });
            start = c;
            end = c;
        }
    }
    ranges.push(if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    });
    ranges.join(",")
}

#[allow(dead_code)]
pub fn validate_cpulist(cpulist: &str) -> bool {
    let max_cpu = get_cpu_count().saturating_sub(1);
    match cpulist_to_set(cpulist) {
        Ok(set) if !set.is_empty() => set.iter().all(|&c| c <= max_cpu),
        _ => false,
    }
}

// ── Thread enumeration ────────────────────────────────────────────────────────

/// Return all thread IDs (TIDs) for a process by reading /proc/<pid>/task/.
/// Falls back to [pid] on error.
pub fn get_tids(pid: u32) -> Vec<u32> {
    let task_dir = format!("/proc/{pid}/task");
    match fs::read_dir(&task_dir) {
        Ok(entries) => entries
            .filter_map(|e| {
                e.ok()
                    .and_then(|e| e.file_name().to_str().map(|s| s.to_owned()))
                    .and_then(|s| s.parse::<u32>().ok())
            })
            .collect(),
        Err(_) => vec![pid],
    }
}

// ── sched_setaffinity ────────────────────────────────────────────────────────

/// Apply CPU affinity to a process AND all its threads via sched_setaffinity(2).
/// Returns true if at least one thread was set successfully.
pub fn set_affinity(pid: u32, cpulist: &str) -> bool {
    let cpuset = match cpulist_to_set(cpulist) {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            log::warn!("set_affinity: empty cpulist for pid {pid}");
            return false;
        }
        Err(e) => {
            log::warn!("set_affinity: bad cpulist {cpulist:?} for pid {pid}: {e}");
            return false;
        }
    };

    // Build nix CpuSet
    use nix::sched::{sched_setaffinity, CpuSet};
    use nix::unistd::Pid;

    let mut cpu_set = CpuSet::new();
    for cpu in &cpuset {
        if let Err(e) = cpu_set.set(*cpu as usize) {
            log::warn!("CpuSet::set cpu={cpu}: {e}");
        }
    }

    let tids = get_tids(pid);
    let mut any_ok = false;
    for tid in tids {
        match sched_setaffinity(Pid::from_raw(tid as i32), &cpu_set) {
            Ok(_) => {
                any_ok = true;
            }
            Err(e) => {
                log::debug!("sched_setaffinity tid={tid}: {e}");
            }
        }
    }
    if any_ok {
        log::debug!("affinity pid={pid} cpulist={cpulist}: applied");
    }
    any_ok
}

/// Read current affinity of the main thread as a cpulist string.
pub fn get_affinity_str(pid: u32) -> String {
    use nix::sched::sched_getaffinity;
    use nix::unistd::Pid;
    match sched_getaffinity(Pid::from_raw(pid as i32)) {
        Ok(cpu_set) => {
            let mut cpus = HashSet::new();
            for i in 0..CpuSet::count() {
                if cpu_set.is_set(i).unwrap_or(false) {
                    cpus.insert(i as u32);
                }
            }
            cpuset_to_cpulist(&cpus)
        }
        Err(_) => String::new(),
    }
}

use nix::sched::CpuSet;

// ── nice ──────────────────────────────────────────────────────────────────────

/// Set nice priority via `renice` subprocess.
/// Negative values require root. Returns true on success.
pub fn set_nice(pid: u32, nice: i32) -> bool {
    let output = Command::new("renice")
        .args(["-n", &nice.to_string(), "-p", &pid.to_string()])
        .output();
    match output {
        Ok(o) if o.status.success() => {
            log::debug!("renice pid={pid} nice={nice}: OK");
            true
        }
        Ok(o) => {
            log::warn!(
                "renice pid={pid} nice={nice} failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            log::warn!("renice pid={pid}: {e}");
            false
        }
    }
}

// ── ionice ───────────────────────────────────────────────────────────────────

/// Set I/O priority via `ionice` subprocess.
/// class: 1=realtime, 2=best-effort, 3=idle. level: 0-7 (RT and BE only).
pub fn set_ionice(pid: u32, class: i32, level: Option<i32>) -> bool {
    let mut cmd = Command::new("ionice");
    cmd.args(["-c", &class.to_string()]);
    if let Some(lvl) = level {
        if class == 1 || class == 2 {
            cmd.args(["-n", &lvl.to_string()]);
        }
    }
    cmd.args(["-p", &pid.to_string()]);
    match cmd.output() {
        Ok(o) if o.status.success() => {
            log::debug!("ionice pid={pid} class={class} level={level:?}: OK");
            true
        }
        Ok(o) => {
            log::warn!(
                "ionice pid={pid} class={class} failed: {}",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            false
        }
        Err(e) => {
            log::warn!("ionice pid={pid}: {e}");
            false
        }
    }
}

// ── Dirty-check reads (avoid redundant syscalls) ─────────────────────────────

/// Read the current nice value for a process from /proc/<pid>/stat.
/// Returns None if the process has exited or cannot be read.
pub fn get_nice(pid: u32) -> Option<i32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Format: pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt
    //         majflt cmajflt utime stime cutime cstime priority nice ...
    // Skip "pid (comm) " by finding the last ')' (comm may contain spaces/parens).
    let after_comm = stat.rfind(')')?.checked_add(1)?;
    let rest = &stat[after_comm..];
    // After closing ')', fields are space-separated starting with state.
    // Index 16 (0-based) is nice (state=0, ppid=1, …, priority=15, nice=16).
    rest.split_whitespace().nth(16).and_then(|s| s.parse().ok())
}

/// Read the current ionice class and level for a process via ioprio_get syscall.
/// Returns None if the syscall fails (e.g., process gone, unsupported).
pub fn get_ionice_raw(pid: u32) -> Option<(i32, i32)> {
    use nix::libc;
    let prio = unsafe {
        libc::syscall(libc::SYS_ioprio_get, 1 /* IOPRIO_WHO_PROCESS */, pid as libc::c_int)
    };
    if prio < 0 {
        return None;
    }
    let class = ((prio as u32 >> 13) & 0x7) as i32;
    let level = (prio as u32 & 0x1fff) as i32;
    Some((class, level))
}

// ── CPU topology helpers ──────────────────────────────────────────────────────

/// Return the set of currently online CPU numbers from /sys/devices/system/cpu/online.
pub fn get_online_cpus() -> HashSet<u32> {
    read_cpulist_file("/sys/devices/system/cpu/online")
        .unwrap_or_else(|| (0..get_cpu_count()).collect())
}

/// Return the set of offline CPU numbers from /sys/devices/system/cpu/offline.
pub fn get_offline_cpus() -> HashSet<u32> {
    read_cpulist_file("/sys/devices/system/cpu/offline").unwrap_or_default()
}

/// Return total logical CPU count including parked CPUs.
/// Uses /sys/devices/system/cpu/present so parked CPUs are counted.
pub fn get_cpu_count() -> u32 {
    if let Some(cpus) = read_cpulist_file("/sys/devices/system/cpu/present") {
        if let Some(&max) = cpus.iter().max() {
            return max + 1;
        }
    }
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

pub fn read_cpulist_file(path: &str) -> Option<HashSet<u32>> {
    let text = fs::read_to_string(path).ok()?;
    cpulist_to_set(text.trim()).ok()
}

/// Returns a map: primary CPU (lowest-numbered logical CPU per physical core)
/// → its HT sibling CPUs. Read from /sys topology/core_id.
/// Used for grouped affinity display in the process table.
pub fn build_core_pairs() -> HashMap<u32, Vec<u32>> {
    let n = get_cpu_count();
    let mut core_to_logical: HashMap<u32, Vec<u32>> = HashMap::new();
    for cpu in 0..n {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/topology/core_id");
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(core_id) = raw.trim().parse::<u32>() {
                core_to_logical.entry(core_id).or_default().push(cpu);
            }
        }
    }
    let mut pairs: HashMap<u32, Vec<u32>> = HashMap::new();
    for mut logical in core_to_logical.into_values() {
        logical.sort_unstable();
        if logical.len() >= 2 {
            pairs.insert(logical[0], logical[1..].to_vec());
        }
    }
    pairs
}

// ── Wine/Proton name resolution ───────────────────────────────────────────────

/// Return the best human-readable process name.
///
/// Wine/Proton processes have comm='Main' (or other generic names) but
/// cmdline[0] is a Windows path like Z:\...\PathOfExileSteam.exe.
/// Also handles comm truncated at 15 chars.
pub fn resolve_name(comm: &str, cmdline: &[String]) -> String {
    if let Some(arg0) = cmdline.first() {
        // Windows path: contains backslash and ends with .exe
        if arg0.contains('\\') && arg0.to_lowercase().ends_with(".exe") {
            let basename = arg0.replace('\\', "/");
            let basename = basename.trim_end_matches('/');
            if let Some(name) = basename.rsplit('/').next() {
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
        // comm is capped at 15 chars by the kernel; try cmdline[0] basename
        if comm.len() == 15 {
            if let Some(basename) = std::path::Path::new(arg0).file_name() {
                let s = basename.to_string_lossy();
                if s.len() > 15 {
                    return s.into_owned();
                }
            }
        }
    }
    comm.to_string()
}
