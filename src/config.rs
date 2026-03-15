//! Load/save config from ~/.config/process-lasso-rs/config.toml
//!
//! Config is stored as TOML. On load, missing keys are filled from
//! DEFAULT_CONFIG via a deep-merge at the serde level (Option defaults).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Sub-structs ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CpuConfig {
    /// Applied to every process not matched by a specific rule.
    /// None = disabled. e.g. "8-15,24-31"
    pub default_affinity: Option<String>,
}

impl Default for CpuConfig {
    fn default() -> Self {
        Self {
            default_affinity: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProBalanceConfig {
    pub enabled: bool,
    pub cpu_threshold_percent: f32,
    pub consecutive_seconds: f32,
    pub nice_adjustment: i32,
    pub nice_floor: i32,
    pub restore_threshold_percent: f32,
    pub restore_hysteresis_seconds: f32,
    pub exempt_patterns: Vec<String>,
}

impl Default for ProBalanceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cpu_threshold_percent: 85.0,
            consecutive_seconds: 3.0,
            nice_adjustment: 10,
            nice_floor: 15,
            restore_threshold_percent: 40.0,
            restore_hysteresis_seconds: 5.0,
            exempt_patterns: vec![
                "kwin".into(),
                "plasmashell".into(),
                "systemd".into(),
                "kthreadd".into(),
                "Xorg".into(),
                "xwayland".into(),
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorConfig {
    pub display_refresh_interval_ms: u64,
    pub rule_enforce_interval_ms: u64,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            display_refresh_interval_ms: 2000,
            rule_enforce_interval_ms: 500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub start_minimized: bool,
    /// Window opacity 0.1–1.0
    pub opacity: f32,
    /// "BreezeDark" | "BreezeLight"
    pub theme: String,
    pub sort_column: String,
    pub sort_ascending: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            start_minimized: false,
            opacity: 1.0,
            theme: "BreezeDark".into(),
            sort_column: "cpu_percent".into(),
            sort_ascending: false,
        }
    }
}

/// A single gaming-mode profile (game launcher + CPU park settings).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GamingProfile {
    pub game_name: String,
    pub command: String,
    /// Map of cpu_index (as string) → keep_online bool
    pub cpu_states: std::collections::HashMap<String, bool>,
    pub elevate_nice: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GamingModeConfig {
    pub profiles: std::collections::HashMap<String, GamingProfile>,
}

impl Default for GamingModeConfig {
    fn default() -> Self {
        Self {
            profiles: Default::default(),
        }
    }
}

// ── Rule (stored inline in config) ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleConfig {
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
}

impl Default for RuleConfig {
    fn default() -> Self {
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
        }
    }
}

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub rules: Vec<RuleConfig>,
    pub cpu: CpuConfig,
    pub probalance: ProBalanceConfig,
    pub monitor: MonitorConfig,
    pub ui: UiConfig,
    pub gaming_mode: GamingModeConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            rules: vec![],
            cpu: CpuConfig::default(),
            probalance: ProBalanceConfig::default(),
            monitor: MonitorConfig::default(),
            ui: UiConfig::default(),
            gaming_mode: GamingModeConfig::default(),
        }
    }
}

// ── Paths ─────────────────────────────────────────────────────────────────────

pub fn config_dir() -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"));
    base.join(".config").join("process-lasso-rs")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

// ── Load / Save ───────────────────────────────────────────────────────────────

/// Load config from disk, filling missing keys with defaults via serde.
pub fn load() -> Config {
    let path = config_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => {
                    log::info!("Loaded config from {}", path.display());
                    return cfg;
                }
                Err(e) => {
                    log::warn!("Config parse error (using defaults): {e}");
                }
            },
            Err(e) => {
                log::warn!("Config read error (using defaults): {e}");
            }
        }
    }
    Config::default()
}

/// Atomically save config to disk (write to .tmp, then rename).
pub fn save(cfg: &Config) -> std::io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = config_path();
    let tmp = path.with_extension("toml.tmp");
    let text = toml::to_string_pretty(cfg).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
    })?;
    fs::write(&tmp, text)?;
    fs::rename(&tmp, &path)?;
    log::debug!("Config saved to {}", path.display());
    Ok(())
}
