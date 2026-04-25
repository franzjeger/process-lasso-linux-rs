//! ProBalance state machine: throttle CPU hogs via nice, restore when calm.
//!
//! Mirrors Python probalance.py exactly:
//!   - Per-PID state: NORMAL → THROTTLED when CPU > threshold for consecutive_seconds
//!   - Restore: THROTTLED → NORMAL when CPU < restore_threshold for restore_hysteresis_seconds

use std::collections::HashMap;

use crate::config::ProBalanceConfig;
use crate::utils;

/// Pure exempt-list check: case-insensitive substring match against any pattern.
/// Empty patterns are ignored so a stray blank entry in config doesn't exempt
/// every process.
fn is_exempt_match(name: &str, patterns: &[String]) -> bool {
    let lower = name.to_lowercase();
    patterns
        .iter()
        .filter(|p| !p.is_empty())
        .any(|p| lower.contains(&p.to_lowercase()))
}

// ── Per-process state ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum ProcState {
    Normal,
    Throttled,
}

#[derive(Debug, Clone)]
struct ProcEntry {
    state: ProcState,
    consecutive_high: f32, // seconds spent above threshold
    consecutive_low: f32,  // seconds spent below restore threshold
    original_nice: Option<i32>,
    throttle_nice: Option<i32>,
}

impl ProcEntry {
    fn new(original_nice: i32) -> Self {
        Self {
            state: ProcState::Normal,
            consecutive_high: 0.0,
            consecutive_low: 0.0,
            original_nice: Some(original_nice),
            throttle_nice: None,
        }
    }
}

// ── Throttle detail for UI display ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ThrottleInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub original_nice: i32,
    pub throttle_nice: i32,
    /// Seconds spent below restore threshold so far (progress toward restore).
    pub consecutive_low: f32,
    /// Target seconds below restore threshold before restoring.
    pub restore_hysteresis: f32,
}

// ── A snapshot of one process (what ProBalance needs) ────────────────────────

#[derive(Debug, Clone)]
pub struct ProcSnapshot {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f32,
    pub nice: i32,
}

// ── State-machine kernel (pure, syscall-free) ────────────────────────────────
//
// `tick()` splits cleanly into three steps:
//   1. decide()           — advance counters, choose an action
//   2. utils::set_nice()  — the only impure call
//   3. finalize_*()       — commit the action's effect on the entry
//
// The split exists so the state machine can be unit-tested without touching
// any process. Tests drive decide() directly and simulate syscall outcomes
// by passing `ok = true` / `false` into the finalize helpers.

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Decision {
    None,
    Throttle { new_nice: i32 },
    Restore { original_nice: i32 },
}

/// Pure state-machine step. Mutates `entry` counters and returns the action
/// to apply. Does NOT change `entry.state` — that's the finalize_*'s job.
fn decide(
    entry: &mut ProcEntry,
    proc: &ProcSnapshot,
    tick_seconds: f32,
    cfg: &ProBalanceConfig,
) -> Decision {
    match entry.state {
        ProcState::Normal => {
            if proc.cpu_percent > cfg.cpu_threshold_percent {
                entry.consecutive_high += tick_seconds;
                if entry.consecutive_high >= cfg.consecutive_seconds {
                    let new_nice = (proc.nice + cfg.nice_adjustment).min(cfg.nice_floor);
                    // Capture the pre-throttle nice so we can restore it later,
                    // even if the syscall fails and we retry on a future tick.
                    entry.original_nice = Some(proc.nice);
                    return Decision::Throttle { new_nice };
                }
            } else {
                entry.consecutive_high = (entry.consecutive_high - tick_seconds).max(0.0);
            }
        }
        ProcState::Throttled => {
            if proc.cpu_percent < cfg.restore_threshold_percent {
                entry.consecutive_low += tick_seconds;
                if entry.consecutive_low >= cfg.restore_hysteresis_seconds {
                    let orig = entry.original_nice.unwrap_or(0);
                    return Decision::Restore {
                        original_nice: orig,
                    };
                }
            } else {
                entry.consecutive_low = 0.0;
            }
        }
    }
    Decision::None
}

/// Apply the result of a Throttle decision after the set_nice syscall.
/// On failure the entry stays Normal with consecutive_high intact, so the
/// next tick can retry without re-accumulating the trigger window.
fn finalize_throttle(entry: &mut ProcEntry, new_nice: i32, syscall_ok: bool) {
    if syscall_ok {
        entry.state = ProcState::Throttled;
        entry.throttle_nice = Some(new_nice);
        entry.consecutive_high = 0.0;
        entry.consecutive_low = 0.0;
    }
}

/// Apply the result of a Restore decision. State always returns to Normal —
/// even if set_nice failed, we shouldn't keep marking the process as throttled
/// when our own bookkeeping is the only thing remembering it.
fn finalize_restore(entry: &mut ProcEntry, original_nice: i32) {
    entry.state = ProcState::Normal;
    entry.consecutive_high = 0.0;
    entry.consecutive_low = 0.0;
    entry.original_nice = Some(original_nice);
    entry.throttle_nice = None;
}

// ── ProBalance ────────────────────────────────────────────────────────────────

pub struct ProBalance {
    cfg: ProBalanceConfig,
    states: HashMap<u32, ProcEntry>,
    log_callback: Option<Box<dyn Fn(String) + Send>>,
}

impl ProBalance {
    pub fn new(cfg: ProBalanceConfig) -> Self {
        Self {
            cfg,
            states: HashMap::new(),
            log_callback: None,
        }
    }

    pub fn update_config(&mut self, cfg: ProBalanceConfig) {
        self.cfg = cfg;
    }

    pub fn set_log_callback<F: Fn(String) + Send + 'static>(&mut self, cb: F) {
        self.log_callback = Some(Box::new(cb));
    }

    fn log(&self, msg: String) {
        log::info!("{msg}");
        if let Some(cb) = &self.log_callback {
            cb(msg);
        }
    }

    fn is_exempt(&self, name: &str) -> bool {
        is_exempt_match(name, &self.cfg.exempt_patterns)
    }

    /// Called every ~1s with the current process snapshot.
    /// tick_seconds is the elapsed time since the last tick.
    pub fn tick(&mut self, snapshot: &[ProcSnapshot], tick_seconds: f32) {
        if !self.cfg.enabled {
            return;
        }

        // Clean up dead PIDs
        let alive: std::collections::HashSet<u32> = snapshot.iter().map(|p| p.pid).collect();
        self.states.retain(|pid, _| alive.contains(pid));

        // Collect log messages separately to avoid holding &mut self while calling self.log()
        let mut pending_logs: Vec<String> = Vec::new();

        for proc in snapshot {
            if self.is_exempt(&proc.name) {
                continue;
            }

            let entry = self
                .states
                .entry(proc.pid)
                .or_insert_with(|| ProcEntry::new(proc.nice));

            // Pure step: advance counters and decide what to do. No syscalls.
            let decision = decide(entry, proc, tick_seconds, &self.cfg);

            // Apply syscall at the boundary, then finalize entry state.
            match decision {
                Decision::None => {}
                Decision::Throttle { new_nice } => {
                    let ok = utils::set_nice(proc.pid, new_nice);
                    if ok {
                        pending_logs.push(format!(
                            "[ProBalance] THROTTLE {}({}) cpu={:.1}% nice {}→{}",
                            proc.name, proc.pid, proc.cpu_percent, proc.nice, new_nice
                        ));
                    }
                    finalize_throttle(entry, new_nice, ok);
                }
                Decision::Restore { original_nice } => {
                    let ok = utils::set_nice(proc.pid, original_nice);
                    if ok {
                        pending_logs.push(format!(
                            "[ProBalance] RESTORE {}({}) cpu={:.1}% nice {}→{}",
                            proc.name, proc.pid, proc.cpu_percent, proc.nice, original_nice
                        ));
                    }
                    finalize_restore(entry, original_nice);
                }
            }
        }

        // Flush log messages now that the mutable borrow of self.states is released
        for msg in pending_logs {
            self.log(msg);
        }
    }

    /// Return the set of currently throttled PIDs (for UI display).
    pub fn throttled_pids(&self) -> std::collections::HashSet<u32> {
        self.states
            .iter()
            .filter(|(_, e)| e.state == ProcState::Throttled)
            .map(|(&pid, _)| pid)
            .collect()
    }

    /// Test-only: count how many PIDs are currently being tracked. Exposed via
    /// `pub(crate)` so we can verify exempt processes are skipped in unit tests
    /// without exercising the syscall path.
    #[cfg(test)]
    pub(crate) fn tracked_pid_count(&self) -> usize {
        self.states.len()
    }

    /// Return detailed info for all currently throttled processes.
    pub fn throttle_infos(&self, snapshot: &[ProcSnapshot]) -> Vec<ThrottleInfo> {
        let name_map: HashMap<u32, (&str, f32)> = snapshot
            .iter()
            .map(|p| (p.pid, (p.name.as_str(), p.cpu_percent)))
            .collect();
        self.states
            .iter()
            .filter(|(_, e)| e.state == ProcState::Throttled)
            .map(|(&pid, e)| {
                let (name, cpu_percent) = name_map.get(&pid).copied().unwrap_or(("unknown", 0.0));
                ThrottleInfo {
                    pid,
                    name: name.to_string(),
                    cpu_percent,
                    original_nice: e.original_nice.unwrap_or(0),
                    throttle_nice: e.throttle_nice.unwrap_or(0),
                    consecutive_low: e.consecutive_low,
                    restore_hysteresis: self.cfg.restore_hysteresis_seconds,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProBalanceConfig;

    fn pat(s: &str) -> Vec<String> {
        vec![s.into()]
    }

    #[test]
    fn exempt_match_is_case_insensitive() {
        assert!(is_exempt_match("KWin", &pat("kwin")));
        assert!(is_exempt_match("kwin_wayland", &pat("KWIN")));
    }

    #[test]
    fn exempt_match_is_substring_not_anchored() {
        // "systemd" should match "systemd-journald" too — this is intentional.
        assert!(is_exempt_match("systemd-journald", &pat("systemd")));
        assert!(is_exempt_match("dbus-systemd-helper", &pat("systemd")));
    }

    #[test]
    fn exempt_match_no_pattern_means_not_exempt() {
        assert!(!is_exempt_match("anything", &[]));
    }

    #[test]
    fn exempt_match_blank_pattern_is_ignored() {
        // Empty patterns must not exempt every process — guards against a stray
        // blank entry in the exempt list config nuking ProBalance silently.
        assert!(!is_exempt_match("steam", &pat("")));
        // But a real pattern alongside the blank still works.
        let mixed = ["".to_string(), "steam".to_string()];
        assert!(is_exempt_match("steam", &mixed));
    }

    #[test]
    fn exempt_match_unrelated_name() {
        assert!(!is_exempt_match("firefox", &pat("kwin")));
    }

    #[test]
    fn tick_disabled_is_noop() {
        let cfg = ProBalanceConfig {
            enabled: false,
            ..Default::default()
        };
        let mut pb = ProBalance::new(cfg);
        let snap = vec![ProcSnapshot {
            pid: 99999,
            name: "anything".into(),
            cpu_percent: 99.0,
            nice: 0,
        }];
        pb.tick(&snap, 1.0);
        // Disabled → state map stays empty, no syscalls attempted.
        assert_eq!(pb.tracked_pid_count(), 0);
    }

    #[test]
    fn tick_skips_exempt_processes_without_tracking() {
        let cfg = ProBalanceConfig {
            exempt_patterns: vec!["kwin".into()],
            ..Default::default()
        };
        let mut pb = ProBalance::new(cfg);
        let snap = vec![ProcSnapshot {
            pid: 4242,
            name: "kwin_wayland".into(),
            cpu_percent: 99.0,
            nice: 0,
        }];
        pb.tick(&snap, 10.0);
        // Exempt processes are skipped entirely — no entry created in state map.
        assert_eq!(pb.tracked_pid_count(), 0);
    }

    #[test]
    fn tick_tracks_non_exempt_below_threshold_without_throttling() {
        // Below threshold → entry exists but stays Normal, no syscalls fire.
        let mut pb = ProBalance::new(ProBalanceConfig::default());
        let snap = vec![ProcSnapshot {
            pid: 4242,
            name: "myapp".into(),
            cpu_percent: 10.0,
            nice: 0,
        }];
        pb.tick(&snap, 1.0);
        assert_eq!(pb.tracked_pid_count(), 1);
        assert!(pb.throttled_pids().is_empty());
    }

    #[test]
    fn tick_evicts_dead_pids() {
        let mut pb = ProBalance::new(ProBalanceConfig::default());
        let snap1 = vec![ProcSnapshot {
            pid: 4242,
            name: "myapp".into(),
            cpu_percent: 10.0,
            nice: 0,
        }];
        pb.tick(&snap1, 1.0);
        assert_eq!(pb.tracked_pid_count(), 1);

        // PID disappears from snapshot → state entry should be cleaned up.
        pb.tick(&[], 1.0);
        assert_eq!(pb.tracked_pid_count(), 0);
    }

    // ── State-machine kernel tests (drive decide() directly) ────────────────
    //
    // These exercise the full Normal↔Throttled lifecycle with no syscalls by
    // calling decide() and feeding the result to finalize_*() directly. They
    // verify the pure decisions, the threshold/hysteresis windows, the
    // nice_floor cap, and the syscall-failure retry behavior.

    fn cfg_for_state_tests() -> ProBalanceConfig {
        ProBalanceConfig {
            enabled: true,
            cpu_threshold_percent: 80.0,
            consecutive_seconds: 3.0,
            nice_adjustment: 5,
            nice_floor: 19,
            restore_threshold_percent: 30.0,
            restore_hysteresis_seconds: 4.0,
            exempt_patterns: vec![],
        }
    }

    fn snap(pid: u32, cpu: f32, nice: i32) -> ProcSnapshot {
        ProcSnapshot {
            pid,
            name: "test".into(),
            cpu_percent: cpu,
            nice,
        }
    }

    #[test]
    fn decide_below_threshold_decays_counter() {
        let cfg = cfg_for_state_tests();
        let mut e = ProcEntry::new(0);
        e.consecutive_high = 2.0;
        let d = decide(&mut e, &snap(1, 10.0, 0), 1.0, &cfg);
        assert_eq!(d, Decision::None);
        assert!((e.consecutive_high - 1.0).abs() < 1e-6);
        // Doesn't go negative
        let d = decide(&mut e, &snap(1, 10.0, 0), 5.0, &cfg);
        assert_eq!(d, Decision::None);
        assert_eq!(e.consecutive_high, 0.0);
    }

    #[test]
    fn decide_above_threshold_below_window_no_action() {
        let cfg = cfg_for_state_tests(); // window = 3.0s
        let mut e = ProcEntry::new(0);
        // 1s above threshold — not yet enough.
        let d = decide(&mut e, &snap(1, 95.0, 0), 1.0, &cfg);
        assert_eq!(d, Decision::None);
        assert!((e.consecutive_high - 1.0).abs() < 1e-6);
        // Total 2s — still not enough.
        let d = decide(&mut e, &snap(1, 95.0, 0), 1.0, &cfg);
        assert_eq!(d, Decision::None);
        assert_eq!(e.state, ProcState::Normal);
    }

    #[test]
    fn decide_throttles_after_consecutive_seconds() {
        let cfg = cfg_for_state_tests();
        let mut e = ProcEntry::new(0);
        let _ = decide(&mut e, &snap(1, 95.0, 0), 1.5, &cfg);
        let d = decide(&mut e, &snap(1, 95.0, 0), 1.5, &cfg); // total 3.0s
        match d {
            Decision::Throttle { new_nice } => {
                // proc.nice (0) + adjustment (5) = 5, capped at nice_floor (19)
                assert_eq!(new_nice, 5);
            }
            other => panic!("expected Throttle, got {other:?}"),
        }
        // decide() captures original_nice but does NOT flip the state — that's finalize_*'s job.
        assert_eq!(e.state, ProcState::Normal);
        assert_eq!(e.original_nice, Some(0));
    }

    #[test]
    fn decide_caps_new_nice_at_nice_floor() {
        let cfg = cfg_for_state_tests(); // floor=19, adjustment=5
        let mut e = ProcEntry::new(18);
        let _ = decide(&mut e, &snap(1, 95.0, 18), 3.0, &cfg);
        // 18 + 5 = 23, but capped at floor=19
        match decide(&mut e, &snap(1, 95.0, 18), 0.01, &cfg) {
            Decision::Throttle { new_nice } => assert_eq!(new_nice, 19),
            // The first decide() already triggered. Re-derive directly:
            _ => {
                // In rare cases the first call was the trigger — that's fine,
                // re-create to verify the cap once.
                let mut e2 = ProcEntry::new(18);
                if let Decision::Throttle { new_nice } =
                    decide(&mut e2, &snap(1, 95.0, 18), 3.5, &cfg)
                {
                    assert_eq!(new_nice, 19);
                } else {
                    panic!("expected Throttle on second attempt");
                }
            }
        }
    }

    #[test]
    fn finalize_throttle_success_flips_state_and_resets_counters() {
        let mut e = ProcEntry::new(0);
        e.consecutive_high = 3.5;
        e.consecutive_low = 1.0;
        finalize_throttle(&mut e, 10, true);
        assert_eq!(e.state, ProcState::Throttled);
        assert_eq!(e.throttle_nice, Some(10));
        assert_eq!(e.consecutive_high, 0.0);
        assert_eq!(e.consecutive_low, 0.0);
    }

    #[test]
    fn finalize_throttle_failure_keeps_state_for_retry() {
        // If set_nice fails (e.g., process exited), we stay Normal with
        // consecutive_high intact — next tick re-decides cleanly.
        let mut e = ProcEntry::new(0);
        e.consecutive_high = 3.0;
        finalize_throttle(&mut e, 10, false);
        assert_eq!(e.state, ProcState::Normal);
        assert_eq!(e.throttle_nice, None);
        assert_eq!(e.consecutive_high, 3.0); // not reset
    }

    #[test]
    fn decide_restored_resets_low_counter_on_high_cpu() {
        let cfg = cfg_for_state_tests();
        let mut e = ProcEntry::new(0);
        e.state = ProcState::Throttled;
        e.consecutive_low = 3.0;
        // CPU above restore_threshold (30%) → counter resets, no decision.
        let d = decide(&mut e, &snap(1, 50.0, 0), 1.0, &cfg);
        assert_eq!(d, Decision::None);
        assert_eq!(e.consecutive_low, 0.0);
        assert_eq!(e.state, ProcState::Throttled);
    }

    #[test]
    fn decide_restores_after_hysteresis_window() {
        let cfg = cfg_for_state_tests(); // restore_hyst = 4.0s
        let mut e = ProcEntry::new(0);
        e.state = ProcState::Throttled;
        e.original_nice = Some(2);
        e.throttle_nice = Some(7);

        // 2s low — not enough.
        let d = decide(&mut e, &snap(1, 5.0, 7), 2.0, &cfg);
        assert_eq!(d, Decision::None);
        // 4.5s total — past hysteresis.
        let d = decide(&mut e, &snap(1, 5.0, 7), 2.5, &cfg);
        match d {
            Decision::Restore { original_nice } => assert_eq!(original_nice, 2),
            other => panic!("expected Restore, got {other:?}"),
        }
        // Still Throttled until finalize runs.
        assert_eq!(e.state, ProcState::Throttled);
    }

    #[test]
    fn finalize_restore_resets_state_regardless_of_syscall() {
        // Even when set_nice fails, we return to Normal — our bookkeeping
        // shouldn't outlive the kernel state we couldn't write.
        let mut e = ProcEntry::new(0);
        e.state = ProcState::Throttled;
        e.throttle_nice = Some(15);
        e.consecutive_low = 4.0;
        finalize_restore(&mut e, 0);
        assert_eq!(e.state, ProcState::Normal);
        assert_eq!(e.throttle_nice, None);
        assert_eq!(e.consecutive_low, 0.0);
        assert_eq!(e.original_nice, Some(0));
    }

    #[test]
    fn full_lifecycle_normal_throttled_normal() {
        // End-to-end: drive a process from idle → high → low → idle through
        // decide() + simulated syscall successes.
        let cfg = cfg_for_state_tests();
        let mut e = ProcEntry::new(0);

        // Stay below threshold first — counter remains 0.
        let _ = decide(&mut e, &snap(1, 5.0, 0), 1.0, &cfg);
        assert_eq!(e.state, ProcState::Normal);

        // CPU spikes for 4s → throttle decision on 3rd second.
        let _ = decide(&mut e, &snap(1, 95.0, 0), 1.0, &cfg);
        let _ = decide(&mut e, &snap(1, 95.0, 0), 1.0, &cfg);
        let d = decide(&mut e, &snap(1, 95.0, 0), 1.0, &cfg);
        let new_nice = match d {
            Decision::Throttle { new_nice } => new_nice,
            other => panic!("expected Throttle, got {other:?}"),
        };
        finalize_throttle(&mut e, new_nice, true);
        assert_eq!(e.state, ProcState::Throttled);

        // CPU drops to 5% — restore after 4s hysteresis.
        let _ = decide(&mut e, &snap(1, 5.0, 5), 2.0, &cfg);
        let d = decide(&mut e, &snap(1, 5.0, 5), 2.0, &cfg);
        let orig = match d {
            Decision::Restore { original_nice } => original_nice,
            other => panic!("expected Restore, got {other:?}"),
        };
        assert_eq!(orig, 0);
        finalize_restore(&mut e, orig);
        assert_eq!(e.state, ProcState::Normal);
        assert_eq!(e.throttle_nice, None);
    }
}
