//! Rule dataclass and RuleEngine for matching and applying per-process rules.
//!
//! Mirrors Python rules.py exactly:
//!   - match_type: "contains" (case-insensitive), "exact", "regex"
//!   - apply_to_process applies ALL matching rules (not first-match-stop)

use regex::Regex;

use crate::config::RuleConfig;
use crate::utils;

// ── Rule ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Rule {
    pub rule_id: String,
    pub name: String,
    pub pattern: String,
    /// "contains" | "exact" | "regex"
    pub match_type: String,
    pub affinity: Option<String>,
    pub nice: Option<i32>,
    pub ionice_class: Option<i32>,
    pub ionice_level: Option<i32>,
    pub enabled: bool,
    /// Compiled regex, populated lazily if match_type == "regex"
    cached_regex: Option<Result<Regex, String>>,
}

impl Rule {
    pub fn from_config(c: &RuleConfig) -> Self {
        let cached_regex = if c.match_type == "regex" {
            Some(Regex::new(&c.pattern).map_err(|e| e.to_string()))
        } else {
            None
        };
        Self {
            rule_id: c.rule_id.clone(),
            name: c.name.clone(),
            pattern: c.pattern.clone(),
            match_type: c.match_type.clone(),
            affinity: c.affinity.clone(),
            nice: c.nice,
            ionice_class: c.ionice_class,
            ionice_level: c.ionice_level,
            enabled: c.enabled,
            cached_regex,
        }
    }

    pub fn to_config(&self) -> RuleConfig {
        RuleConfig {
            rule_id: self.rule_id.clone(),
            name: self.name.clone(),
            pattern: self.pattern.clone(),
            match_type: self.match_type.clone(),
            affinity: self.affinity.clone(),
            nice: self.nice,
            ionice_class: self.ionice_class,
            ionice_level: self.ionice_level,
            enabled: self.enabled,
        }
    }

    pub fn new_empty() -> Self {
        Self {
            rule_id: uuid::Uuid::new_v4().to_string(),
            name: String::new(),
            pattern: String::new(),
            match_type: "contains".into(),
            affinity: None,
            nice: None,
            ionice_class: None,
            ionice_level: None,
            enabled: true,
            cached_regex: None,
        }
    }

    /// Returns true if proc_name matches this rule.
    pub fn matches(&self, proc_name: &str) -> bool {
        if !self.enabled || self.pattern.is_empty() {
            return false;
        }
        match self.match_type.as_str() {
            "exact" => proc_name == self.pattern,
            "regex" => {
                if let Some(Ok(re)) = &self.cached_regex {
                    re.is_match(proc_name)
                } else {
                    // Try compiling on the fly (shouldn't happen normally)
                    Regex::new(&self.pattern)
                        .map(|re| re.is_match(proc_name))
                        .unwrap_or(false)
                }
            }
            _ => {
                // "contains" — case-insensitive substring
                proc_name
                    .to_lowercase()
                    .contains(&self.pattern.to_lowercase())
            }
        }
    }

    /// Invalidate cached regex after pattern/match_type change.
    pub fn refresh_regex(&mut self) {
        self.cached_regex = if self.match_type == "regex" {
            Some(Regex::new(&self.pattern).map_err(|e| e.to_string()))
        } else {
            None
        };
    }
}

// ── RuleEngine ────────────────────────────────────────────────────────────────

pub struct RuleEngine {
    rules: Vec<Rule>,
    log_callback: Option<Box<dyn Fn(String) + Send>>,
}

impl RuleEngine {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            log_callback: None,
        }
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

    pub fn load_rules(&mut self, configs: &[RuleConfig]) {
        self.rules = configs.iter().map(Rule::from_config).collect();
    }

    pub fn get_rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn get_rules_mut(&mut self) -> &mut Vec<Rule> {
        &mut self.rules
    }

    pub fn add_rule(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    pub fn remove_rule(&mut self, rule_id: &str) {
        self.rules.retain(|r| r.rule_id != rule_id);
    }

    pub fn update_rule(&mut self, updated: Rule) {
        if let Some(r) = self.rules.iter_mut().find(|r| r.rule_id == updated.rule_id) {
            *r = updated;
        }
    }

    pub fn to_config_list(&self) -> Vec<RuleConfig> {
        self.rules.iter().map(|r| r.to_config()).collect()
    }

    /// Apply all matching rules to a process. Returns list of action descriptions.
    /// All matching rules are applied (not first-match-stop).
    /// Dirty-checks each attribute before calling the syscall so that periodic
    /// re-enforcement does not spam the log with no-op "already correct" entries.
    pub fn apply_to_process(&self, pid: u32, proc_name: &str) -> Vec<String> {
        let mut actions = Vec::new();
        for rule in &self.rules {
            if !rule.matches(proc_name) {
                continue;
            }

            // ── Affinity ─────────────────────────────────────────────────
            if let Some(ref aff) = rule.affinity {
                let target = utils::cpulist_to_set(aff).unwrap_or_default();
                let current_str = utils::get_affinity_str(pid);
                let current = utils::cpulist_to_set(&current_str).unwrap_or_default();
                if current != target {
                    if utils::set_affinity(pid, aff) {
                        let msg = format!(
                            "[Rule:{}] Set affinity={} on {}({})",
                            rule.name, aff, proc_name, pid
                        );
                        self.log(msg.clone());
                        actions.push(msg);
                    }
                }
            }

            // ── Nice ─────────────────────────────────────────────────────
            if let Some(nice) = rule.nice {
                let current_nice = utils::get_nice(pid);
                if current_nice != Some(nice) {
                    if utils::set_nice(pid, nice) {
                        let msg = format!(
                            "[Rule:{}] Set nice={} on {}({})",
                            rule.name, nice, proc_name, pid
                        );
                        self.log(msg.clone());
                        actions.push(msg);
                    } else {
                        let msg = format!(
                            "[Rule:{}] nice={} FAILED (root needed?) for {}({})",
                            rule.name, nice, proc_name, pid
                        );
                        self.log(msg.clone());
                        actions.push(msg);
                    }
                }
            }

            // ── Ionice ───────────────────────────────────────────────────
            if let Some(class) = rule.ionice_class {
                let target_level = rule.ionice_level.unwrap_or(0);
                let current = utils::get_ionice_raw(pid);
                if current != Some((class, target_level)) {
                    if utils::set_ionice(pid, class, rule.ionice_level) {
                        let msg = format!(
                            "[Rule:{}] Set ionice class={} level={:?} on {}({})",
                            rule.name, class, rule.ionice_level, proc_name, pid
                        );
                        self.log(msg.clone());
                        actions.push(msg);
                    }
                }
            }
        }
        actions
    }
}

impl Default for RuleEngine {
    fn default() -> Self {
        Self::new()
    }
}
