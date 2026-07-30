//! CPU core parking: take non-preferred CPUs offline via privileged helper.
//!
//! Mirrors Python cpu_park.py:
//!   - detect_topology(): AMD X3D (L3 cache asymmetry), Intel Hybrid (max freq), or UNIFORM
//!   - park_cpus() / unpark_all() via sudo /usr/local/bin/argus-lasso-sysfs
//!   - get_smt_siblings_of(): reads /sys/.../topology/core_id
//!   - Topology cache: preserved across calls so Gaming Mode doesn't lose it once CPUs are parked

use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::Command;
use std::sync::Mutex;

use crate::utils::{cpuset_to_cpulist, get_offline_cpus, read_cpulist_file};

// ── Constants ─────────────────────────────────────────────────────────────────

pub const HELPER: &str = "/usr/local/bin/argus-lasso-sysfs";
pub const SUDOERS_FILE: &str = "/etc/sudoers.d/argus-lasso";

pub const HELPER_CONTENT: &str = r#"#!/bin/bash
# Argus-Lasso privileged sysfs helper — managed by argus-lasso.
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
    --check-only)
        # Used by is_sudoers_installed to verify the NOPASSWD rule works.
        exit 0
        ;;
    renice-pid)
        [[ "$2" =~ ^-?[0-9]+$ ]] || exit 1
        [[ "$3" =~ ^[0-9]+$ ]]   || exit 1
        renice -n "$2" -p "$3"
        ;;
    cpu-governor)
        [[ "$2" =~ ^[a-z_-]+$ ]] || exit 1
        for f in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
            echo "$2" > "$f" 2>/dev/null || true
        done
        ;;
    cpu-epp)
        [[ "$2" =~ ^[a-z_-]+$ ]] || exit 1
        for f in /sys/devices/system/cpu/cpu*/cpufreq/energy_performance_preference; do
            echo "$2" > "$f" 2>/dev/null || true
        done
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
    /// Short human label for preferred cores, e.g. "P-cores (5.5 GHz)" or "V-Cache CCD (96 MB L3)"
    pub preferred_label: String,
    /// Short human label for non-preferred cores, e.g. "E-cores (4.9 GHz)" or "Standard CCD (32 MB L3)"
    pub non_preferred_label: String,
}

impl CpuTopology {
    pub fn uniform(all_cpus: HashSet<u32>) -> Self {
        Self {
            kind: TopologyKind::Uniform,
            preferred: all_cpus,
            non_preferred: HashSet::new(),
            description: "Uniform topology (no asymmetry detected). All CPUs equal.".into(),
            preferred_label: String::new(),
            non_preferred_label: String::new(),
        }
    }

    pub fn has_asymmetry(&self) -> bool {
        !self.non_preferred.is_empty()
    }

    /// Button label for preferred cores, e.g. "P-cores 0-7 (5.5 GHz)"
    pub fn preferred_button_label(&self) -> String {
        format!(
            "{} ({})",
            self.preferred_label,
            cpuset_to_cpulist(&self.preferred)
        )
    }

    /// Button label for non-preferred cores, e.g. "E-cores 8-23 (4.9 GHz)"
    pub fn non_preferred_button_label(&self) -> String {
        format!(
            "{} ({})",
            self.non_preferred_label,
            cpuset_to_cpulist(&self.non_preferred)
        )
    }

    /// Short kind label for display
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            TopologyKind::AmdX3D => "AMD X3D",
            TopologyKind::IntelHybrid => "Intel Hybrid",
            TopologyKind::Uniform => "Symmetric",
        }
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
    read_cpulist_file("/sys/devices/system/cpu/present").unwrap_or_else(|| {
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
            // One unparsable size file must skip that CPU, not abort the whole
            // detection (a `?` here degraded real X3D machines to Uniform).
            let kb: Option<u64> = if let Some(s) = raw.strip_suffix('K') {
                s.parse().ok()
            } else if let Some(s) = raw.strip_suffix('M') {
                s.parse::<u64>().ok().map(|mb| mb * 1024)
            } else {
                raw.parse().ok()
            };
            let Some(kb) = kb else { continue };
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
        // Only interpret offline CPUs as "the other CCD is parked" when a
        // previous detection actually saw an X3D topology — otherwise any
        // machine with a manually offlined core would be misdetected as X3D.
        let cached_is_x3d = TOPO_CACHE
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|t| t.kind == TopologyKind::AmdX3D);
        if !offline.is_empty() && cached_is_x3d {
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
                preferred_label: format!("V-Cache CCD ({} MB L3)", online_kb / 1024),
                non_preferred_label: "Standard CCD (parked)".into(),
            });
        }
        return None; // genuine uniform L3
    }

    let max_kb = *sizes.iter().max().unwrap();
    let min_kb = *sizes.iter().min().unwrap();
    let preferred: HashSet<u32> = l3
        .iter()
        .filter(|(_, &s)| s == max_kb)
        .map(|(&c, _)| c)
        .collect();
    let non_preferred: HashSet<u32> = l3
        .iter()
        .filter(|(_, &s)| s == min_kb)
        .map(|(&c, _)| c)
        .collect();

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
        preferred_label: format!("V-Cache CCD ({} MB L3)", max_kb / 1024),
        non_preferred_label: format!("Standard CCD ({} MB L3)", min_kb / 1024),
    })
}

/// Detect Intel Hybrid: P-cores vs E-cores.
/// Primary: kernel sysfs cpu_core/cpu_atom (reliable, available since Linux 5.18+).
/// Fallback: frequency-based classification using the midpoint between max and min freq.
fn detect_intel_hybrid() -> Option<CpuTopology> {
    // ── Primary: kernel cpu_core / cpu_atom classification ────────────────
    let p_cores = read_cpulist_file("/sys/devices/cpu_core/cpus");
    let e_cores = read_cpulist_file("/sys/devices/cpu_atom/cpus");
    if let (Some(p), Some(e)) = (p_cores, e_cores) {
        if !p.is_empty() && !e.is_empty() {
            // Read max freq for labels (best-effort)
            let p_max = p
                .iter()
                .filter_map(|&c| {
                    fs::read_to_string(format!(
                        "/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq"
                    ))
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                })
                .max()
                .unwrap_or(0);
            let e_max = e
                .iter()
                .filter_map(|&c| {
                    fs::read_to_string(format!(
                        "/sys/devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq"
                    ))
                    .ok()
                    .and_then(|s| s.trim().parse::<u64>().ok())
                })
                .max()
                .unwrap_or(0);

            return Some(CpuTopology {
                kind: TopologyKind::IntelHybrid,
                preferred: p.clone(),
                non_preferred: e.clone(),
                description: format!(
                    "Intel Hybrid detected. P-cores ({:.1} GHz max): CPUs {}. E-cores ({:.1} GHz max): CPUs {}.",
                    p_max as f64 / 1_000_000.0,
                    cpuset_to_cpulist(&p),
                    e_max as f64 / 1_000_000.0,
                    cpuset_to_cpulist(&e),
                ),
                preferred_label: format!("P-cores ({:.1} GHz)", p_max as f64 / 1_000_000.0),
                non_preferred_label: format!("E-cores ({:.1} GHz)", e_max as f64 / 1_000_000.0),
            });
        }
    }

    // ── Fallback: frequency-based detection ──────────────────────────────
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
    // Use midpoint between highest and lowest freq as threshold —
    // much more robust than 80% of max for close P/E freq gaps.
    let threshold = (max_f + min_f) / 2;
    let preferred: HashSet<u32> = max_freq
        .iter()
        .filter(|(_, &f)| f >= threshold)
        .map(|(&c, _)| c)
        .collect();
    let non_preferred: HashSet<u32> = max_freq
        .iter()
        .filter(|(_, &f)| f < threshold)
        .map(|(&c, _)| c)
        .collect();

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
        preferred_label: format!("P-cores ({:.1} GHz)", max_f as f64 / 1_000_000.0),
        non_preferred_label: format!("E-cores ({:.1} GHz)", min_f as f64 / 1_000_000.0),
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
        .map(|s| s.contains("cpu-governor") && s.contains("--check-only"))
        .unwrap_or(false)
}

/// Check whether the sudoers NOPASSWD rule is in place by doing a dry-run sudo.
pub fn is_sudoers_installed() -> bool {
    if !is_helper_installed() {
        return false;
    }
    // Only exit code 0 counts: the helper's --check-only case exits 0, while
    // sudo refusing for lack of a NOPASSWD rule exits 1 — treating 1 as
    // success used to report "installed" on machines with no sudoers rule.
    Command::new("sudo")
        .args(["-n", HELPER, "--check-only"])
        .output()
        .map(|o| o.status.code() == Some(0))
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
            (
                false,
                if msg.is_empty() {
                    String::from_utf8_lossy(&o.stdout).trim().to_string()
                } else {
                    msg
                },
            )
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
            log_cb("[Park] Skipping CPU 0 (bootstrap processor, cannot offline)".to_string());
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

/// True if polkit's pkexec is available — the preferred install path, since
/// authentication happens in the system dialog and no password ever passes
/// through this process.
pub fn is_pkexec_available() -> bool {
    Command::new("which")
        .arg("pkexec")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Validate the username, stage the helper script in the user's private config
/// dir, and build the root shell command that installs helper + sudoers rule.
/// Returns (command, staged_path) — the caller removes the staged file after
/// the privileged command finishes.
fn stage_install(username: &str) -> Result<(String, std::path::PathBuf), String> {
    let username = if username.is_empty() {
        std::env::var("USER").unwrap_or_default()
    } else {
        username.to_string()
    };
    if username.is_empty() {
        return Err("Could not determine current username.".into());
    }
    // The username is interpolated into a root shell command and a sudoers
    // file — reject anything outside the safe POSIX username charset so a
    // crafted value can't break out of the quoting or corrupt sudoers.
    if !username
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        || username.starts_with('-')
    {
        return Err(format!("Invalid username: {username:?}"));
    }

    let sudoers_line = format!("{username} ALL=(root) NOPASSWD: {HELPER}");

    // Stage the helper in the user's private config dir — NOT a fixed
    // world-writable /tmp path, which another local user could swap between
    // our write and root's cp (a straight local-root escalation).
    let stage_dir = crate::config::config_dir();
    fs::create_dir_all(&stage_dir).map_err(|e| format!("Failed to create staging dir: {e}"))?;
    let tmp_path = stage_dir.join("pl-sysfs.staged");
    let tmp = match tmp_path.to_str() {
        Some(s) if !s.contains('\'') && !s.contains(char::is_whitespace) => s.to_string(),
        _ => return Err("Staging path contains unsafe characters.".into()),
    };
    fs::write(&tmp_path, HELPER_CONTENT)
        .map_err(|e| format!("Failed to write staged helper: {e}"))?;

    let cmd = format!(
        "cp {tmp} {HELPER} && \
         chmod 755 {HELPER} && \
         chown root:root {HELPER} && \
         printf '%s\\n' '{sudoers_line}' > {SUDOERS_FILE} && \
         chmod 440 {SUDOERS_FILE} && \
         echo INSTALL_OK"
    );
    Ok((cmd, tmp_path))
}

fn install_outcome(o: &std::process::Output) -> (bool, String) {
    let out = String::from_utf8_lossy(&o.stdout);
    let err = String::from_utf8_lossy(&o.stderr);
    let combined = format!("{out}{err}");
    if combined.contains("INSTALL_OK") {
        (true, "Helper and sudoers rule installed.".into())
    } else {
        let tail: String = {
            let t: Vec<char> = combined.trim().chars().collect();
            t[t.len().saturating_sub(300)..].iter().collect()
        };
        (
            false,
            format!("Install failed (rc={:?}): {tail}", o.status.code()),
        )
    }
}

/// Install helper + sudoers rule via polkit (pkexec). Authentication is
/// handled by the desktop's polkit agent — no password touches this process.
pub fn install_helper_via_pkexec(username: &str) -> (bool, String) {
    let (cmd, tmp_path) = match stage_install(username) {
        Ok(v) => v,
        Err(e) => return (false, e),
    };
    let result = Command::new("pkexec")
        .args(["/bin/sh", "-c", &cmd])
        .output();
    let _ = fs::remove_file(&tmp_path);
    match result {
        Ok(o) if o.status.code() == Some(126) || o.status.code() == Some(127) => {
            // 126 = auth dialog dismissed, 127 = auth failed / no agent
            (false, "Authentication cancelled or failed.".into())
        }
        Ok(o) => install_outcome(&o),
        Err(e) => (false, format!("pkexec spawn failed: {e}")),
    }
}

/// Fallback install path via `su root -c`, feeding the root password on stdin.
/// Used only when pkexec/polkit is unavailable.
pub fn install_helper_as_root(username: &str, password: &str) -> (bool, String) {
    use std::io::Write;

    if password.is_empty() {
        return (false, "No root password provided.".into());
    }
    let (cmd, tmp_path) = match stage_install(username) {
        Ok(v) => v,
        Err(e) => return (false, e),
    };

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
            let result = child.wait_with_output();
            let _ = fs::remove_file(&tmp_path);
            match result {
                Ok(o) => install_outcome(&o),
                Err(e) => (false, format!("su wait failed: {e}")),
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            (false, format!("su spawn failed: {e}"))
        }
    }
}

// ── Power profiles (governor + EPP via helper) ───────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerProfile {
    Performance,
    Balanced,
    PowerSave,
}

impl PowerProfile {
    pub fn label(&self) -> &'static str {
        match self {
            PowerProfile::Performance => "Performance",
            PowerProfile::Balanced => "Balanced",
            PowerProfile::PowerSave => "Power Save",
        }
    }

    /// (governor, energy_performance_preference) for this profile.
    /// Both amd-pstate and intel_pstate expose performance/powersave governors
    /// and the EPP knob; the helper writes best-effort to every CPU.
    fn settings(&self) -> (&'static str, &'static str) {
        match self {
            PowerProfile::Performance => ("performance", "performance"),
            PowerProfile::Balanced => ("powersave", "balance_performance"),
            PowerProfile::PowerSave => ("powersave", "power"),
        }
    }
}

/// Apply a power profile via the privileged helper. Returns (ok, message).
pub fn apply_power_profile(profile: PowerProfile) -> (bool, String) {
    let (governor, epp) = profile.settings();
    let (gov_ok, gov_msg) = run_helper(&["cpu-governor", governor]);
    // EPP is absent on acpi-cpufreq systems — the helper's glob writes are
    // best-effort, so a failure here is only reported, not fatal.
    let (epp_ok, epp_msg) = run_helper(&["cpu-epp", epp]);
    if gov_ok {
        let epp_note = if epp_ok {
            format!(", EPP={epp}")
        } else {
            format!(" (EPP unavailable: {epp_msg})")
        };
        (
            true,
            format!(
                "[Power] {} — governor={governor}{epp_note}",
                profile.label()
            ),
        )
    } else {
        (false, format!("[Power] governor change failed: {gov_msg}"))
    }
}

/// Read the current scaling governor of cpu0 (representative for display).
pub fn current_governor() -> Option<String> {
    fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
        .ok()
        .map(|s| s.trim().to_string())
}

/// Read the current EPP of cpu0, if the platform exposes it.
pub fn current_epp() -> Option<String> {
    fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference")
        .ok()
        .map(|s| s.trim().to_string())
}
