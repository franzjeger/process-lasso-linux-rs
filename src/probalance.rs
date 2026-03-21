//! ProBalance state machine: throttle CPU hogs via nice, restore when calm.
//!
//! Mirrors Python probalance.py exactly:
//!   - Per-PID state: NORMAL → THROTTLED when CPU > threshold for consecutive_seconds
//!   - Restore: THROTTLED → NORMAL when CPU < restore_threshold for restore_hysteresis_seconds

use std::collections::HashMap;

use crate::config::ProBalanceConfig;
use crate::utils;

// ── Per-process state ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum ProcState {
    Normal,
    Throttled,
}

#[derive(Debug, Clone)]
struct ProcEntry {
    state: ProcState,
    consecutive_high: f32,  // seconds spent above threshold
    consecutive_low: f32,   // seconds spent below restore threshold
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
        let lower = name.to_lowercase();
        self.cfg
            .exempt_patterns
            .iter()
            .any(|p| lower.contains(&p.to_lowercase()))
    }

    /// Called every ~1s with the current process snapshot.
    /// tick_seconds is the elapsed time since the last tick.
    pub fn tick(&mut self, snapshot: &[ProcSnapshot], tick_seconds: f32) {
        if !self.cfg.enabled {
            return;
        }

        let threshold      = self.cfg.cpu_threshold_percent;
        let consec_thresh  = self.cfg.consecutive_seconds;
        let adjustment     = self.cfg.nice_adjustment;
        let nice_floor     = self.cfg.nice_floor;
        let restore_thresh = self.cfg.restore_threshold_percent;
        let restore_hyst   = self.cfg.restore_hysteresis_seconds;

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

            match entry.state {
                ProcState::Normal => {
                    if proc.cpu_percent > threshold {
                        entry.consecutive_high += tick_seconds;
                        if entry.consecutive_high >= consec_thresh {
                            // Throttle
                            let new_nice = (proc.nice + adjustment).min(nice_floor);
                            entry.original_nice = Some(proc.nice);
                            if utils::set_nice(proc.pid, new_nice) {
                                pending_logs.push(format!(
                                    "[ProBalance] THROTTLE {}({}) cpu={:.1}% nice {}→{}",
                                    proc.name, proc.pid, proc.cpu_percent, proc.nice, new_nice
                                ));
                                entry.state = ProcState::Throttled;
                                entry.throttle_nice = Some(new_nice);
                                entry.consecutive_high = 0.0;
                                entry.consecutive_low = 0.0;
                            }
                        }
                    } else {
                        entry.consecutive_high = (entry.consecutive_high - tick_seconds).max(0.0);
                    }
                }
                ProcState::Throttled => {
                    if proc.cpu_percent < restore_thresh {
                        entry.consecutive_low += tick_seconds;
                        if entry.consecutive_low >= restore_hyst {
                            // Restore
                            let orig = entry.original_nice.unwrap_or(0);
                            if utils::set_nice(proc.pid, orig) {
                                pending_logs.push(format!(
                                    "[ProBalance] RESTORE {}({}) cpu={:.1}% nice {}→{}",
                                    proc.name, proc.pid, proc.cpu_percent, proc.nice, orig
                                ));
                            }
                            entry.state = ProcState::Normal;
                            entry.consecutive_high = 0.0;
                            entry.consecutive_low = 0.0;
                            entry.original_nice = Some(orig);
                            entry.throttle_nice = None;
                        }
                    } else {
                        entry.consecutive_low = 0.0;
                    }
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
