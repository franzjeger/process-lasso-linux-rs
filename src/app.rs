//! eframe App: shared state, tab routing, purple theme, system tray.

use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Context, RichText};

use crossbeam_channel::Sender;

use crate::config::{self, Config};
use crate::gui::bench_tab::BenchTab;
use crate::gui::dialogs::{AffinityDialog, IoNiceDialog, NiceDialog};
use crate::gui::gaming_mode_tab::{GamingEvent, GamingModeTab};
use crate::gui::hw_monitor_tab::HwMonitorTab;
use crate::gui::log_tab::LogTab;
use crate::gui::overview_tab::OverviewTab;
use crate::gui::probalance_tab::ProBalanceTab;
use crate::gui::process_tab::{ProcessTab, TableAction};
use crate::gui::rules_tab::RulesTab;
use crate::gui::settings_tab::SettingsTab;
use crate::monitor::{AppState, DaemonCmd};
use crate::rules::RuleEngine;
use crate::utils;

// ── Active tab ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
enum Tab {
    Overview,
    Processes,
    Rules,
    ProBalance,
    GamingMode,
    HwMonitor,
    Benchmark,
    Settings,
    Log,
}

// ── CPU temperature ───────────────────────────────────────────────────────────

/// Read CPU temperature from hwmon sysfs. Returns degrees Celsius or None.
fn read_cpu_temp() -> Option<f32> {
    const KNOWN_NAMES: &[&str] = &["k10temp", "zenpower", "coretemp"];

    let hwmon_dir = std::path::Path::new("/sys/class/hwmon");
    let entries = std::fs::read_dir(hwmon_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        let name_path = path.join("name");
        let name = std::fs::read_to_string(&name_path)
            .ok()
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        let is_match = KNOWN_NAMES.contains(&name.as_str()) || name.starts_with("it8");

        if !is_match {
            continue;
        }

        // Collect all temp*_input files and return the highest value
        let mut max_temp: Option<f32> = None;
        if let Ok(dir_entries) = std::fs::read_dir(&path) {
            for de in dir_entries.flatten() {
                let fname = de.file_name();
                let fname_str = fname.to_string_lossy();
                if fname_str.starts_with("temp") && fname_str.ends_with("_input") {
                    if let Ok(raw) = std::fs::read_to_string(de.path()) {
                        if let Ok(val) = raw.trim().parse::<i64>() {
                            let celsius = val as f32 / 1000.0;
                            max_temp = Some(max_temp.map_or(celsius, |m: f32| m.max(celsius)));
                        }
                    }
                }
            }
        }

        if max_temp.is_some() {
            return max_temp;
        }
    }

    None
}

// ── "Remember settings" offer ─────────────────────────────────────────────────

/// After a manual affinity/nice/ionice change, offer to persist it as a rule
/// so it survives process restarts (à la Process Lasso's "remember" prompt).
struct RuleOffer {
    proc_name: String,
    affinity: Option<String>,
    nice: Option<i32>,
    ionice: Option<(i32, i32)>,
}

impl RuleOffer {
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(a) = &self.affinity {
            parts.push(format!("affinity {a}"));
        }
        if let Some(n) = self.nice {
            parts.push(format!("nice {n}"));
        }
        if let Some((c, l)) = self.ionice {
            parts.push(format!("ionice {c}/{l}"));
        }
        parts.join(", ")
    }
}

// ── ArgusLassoApp ─────────────────────────────────────────────────────────────

pub struct ArgusLassoApp {
    state: Arc<Mutex<AppState>>,
    cmd_tx: Sender<DaemonCmd>,
    rule_engine: Arc<Mutex<RuleEngine>>,

    active_tab: Tab,
    process_tab: ProcessTab,
    rules_tab: RulesTab,
    probalance_tab: ProBalanceTab,
    gaming_mode_tab: GamingModeTab,
    hw_monitor_tab: HwMonitorTab,
    bench_tab: BenchTab,
    overview_tab: OverviewTab,
    settings_tab: SettingsTab,
    log_tab: LogTab,

    // Per-process dialogs — each tracks its own target PID so two open
    // dialogs can never apply one process's settings to another.
    affinity_dialog: Option<(u32, AffinityDialog)>,
    nice_dialog: Option<(u32, NiceDialog)>,
    ionice_dialog: Option<(u32, IoNiceDialog)>,

    // Process count for tab title
    proc_count: usize,
    throttled_count: usize,

    // Generation counter: only push CPU history when daemon emits new data
    last_cpu_gen: u64,

    // Wayland compositor-side opacity via wp_alpha_modifier_v1
    wayland_opacity: Option<crate::wayland_opacity::WaylandOpacity>,
    // Current window opacity (0.1–1.0); tracked so we only call set() when it changes
    opacity: f32,
    // Native pixels-per-point at startup (for HiDPI scaling)
    native_ppp: f32,

    // Repaint rate diagnostics
    repaint_count: u32,
    last_repaint_log: std::time::Instant,

    // Track last persisted opacity/theme to detect changes for immediate save
    last_saved_opacity: f32,
    last_saved_theme: String,

    // CPU temperature read from hwmon sysfs
    cpu_temp: Option<f32>,
    // Pending kill awaiting undo
    pending_kill: Option<crate::gui::process_tab::PendingKill>,
    // Pending "create a rule from this manual change?" offer
    rule_offer: Option<RuleOffer>,
    // CPU model string for status bar
    cpu_model: String,
}

impl ArgusLassoApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        state: Arc<Mutex<AppState>>,
        cmd_tx: Sender<DaemonCmd>,
        rule_engine: Arc<Mutex<RuleEngine>>,
        config: Config,
    ) -> Self {
        // native_pixels_per_point is set by the platform integration before new() is called.
        let native_ppp = cc.egui_ctx.pixels_per_point();
        let startup_theme = crate::gui::theme::AppTheme::from_str(&config.ui.theme);
        crate::gui::theme::apply_theme(&cc.egui_ctx, native_ppp, &startup_theme);

        let probalance_tab = ProBalanceTab::new(config.probalance.clone());
        let gaming_mode_tab = GamingModeTab::new(config.clone());
        let mut settings_tab = SettingsTab::new(config.clone());
        settings_tab.native_ppp = native_ppp;

        // Initialise Wayland compositor-side opacity via wp_alpha_modifier_v1.
        // Extract the raw wl_display* and wl_surface* that eframe already holds.
        use raw_window_handle::{
            HasDisplayHandle as _, HasWindowHandle as _, RawDisplayHandle, RawWindowHandle,
        };
        let display_ptr: *mut std::ffi::c_void = cc
            .display_handle()
            .ok()
            .and_then(|dh| match dh.as_raw() {
                RawDisplayHandle::Wayland(h) => Some(h.display.as_ptr()),
                _ => None,
            })
            .unwrap_or(std::ptr::null_mut());
        let surface_ptr: *mut std::ffi::c_void = cc
            .window_handle()
            .ok()
            .and_then(|wh| match wh.as_raw() {
                RawWindowHandle::Wayland(h) => Some(h.surface.as_ptr()),
                _ => None,
            })
            .unwrap_or(std::ptr::null_mut());

        let wayland_opacity = crate::wayland_opacity::WaylandOpacity::new(display_ptr, surface_ptr);
        if wayland_opacity.is_none() {
            log::warn!(
                "Wayland opacity unavailable — compositor does not support wp_alpha_modifier_v1"
            );
        }

        // Restore saved opacity; apply immediately so it takes effect on first frame.
        let saved_opacity = config.ui.opacity.clamp(0.1, 1.0);
        if (saved_opacity - 1.0).abs() > 0.001 {
            if let Some(ref wo) = wayland_opacity {
                wo.set(saved_opacity);
            }
        }

        // Sync state config
        if let Ok(mut s) = state.lock() {
            s.config = config.clone();
        }

        let last_saved_opacity = saved_opacity;
        let last_saved_theme = startup_theme.to_str().to_string();
        let cpu_temp = read_cpu_temp();
        let cpu_model = crate::monitor::read_cpu_model();

        Self {
            state,
            cmd_tx,
            rule_engine,
            active_tab: Tab::Overview,
            process_tab: ProcessTab::new(&config.ui.col_widths),
            rules_tab: RulesTab::new(),
            probalance_tab,
            gaming_mode_tab,
            hw_monitor_tab: HwMonitorTab::new_with_widths(&config.ui.hw_mon_col_widths),
            bench_tab: BenchTab::new(),
            overview_tab: OverviewTab::new(),
            settings_tab,
            log_tab: LogTab::new(),
            affinity_dialog: None,
            nice_dialog: None,
            ionice_dialog: None,
            proc_count: 0,
            throttled_count: 0,
            last_cpu_gen: 0,
            wayland_opacity,
            opacity: saved_opacity,
            native_ppp,
            repaint_count: 0,
            last_repaint_log: std::time::Instant::now(),
            last_saved_opacity,
            last_saved_theme,
            cpu_temp,
            pending_kill: None,
            rule_offer: None,
            cpu_model,
        }
    }

    /// Record a manual change so the "remember as rule?" prompt can offer it.
    /// Consecutive changes to the same process merge into one offer.
    fn offer_rule(
        &mut self,
        proc_name: String,
        affinity: Option<String>,
        nice: Option<i32>,
        ionice: Option<(i32, i32)>,
    ) {
        match &mut self.rule_offer {
            Some(offer) if offer.proc_name == proc_name => {
                if affinity.is_some() {
                    offer.affinity = affinity;
                }
                if nice.is_some() {
                    offer.nice = nice;
                }
                if ionice.is_some() {
                    offer.ionice = ionice;
                }
            }
            _ => {
                self.rule_offer = Some(RuleOffer {
                    proc_name,
                    affinity,
                    nice,
                    ionice,
                });
            }
        }
    }

    fn send(&self, cmd: DaemonCmd) {
        let _ = self.cmd_tx.send(cmd);
    }

    fn save_config(&self) {
        let cfg = if let Ok(s) = self.state.lock() {
            s.config.clone()
        } else {
            return;
        };
        if let Err(e) = config::save(&cfg) {
            log::warn!("Config save failed: {e}");
        }
    }

    /// Send the actual kill signal. The target was SIGSTOPped for the undo
    /// window, and a stopped process never sees SIGTERM — so always follow up
    /// with SIGCONT to deliver it (harmless for SIGKILL).
    fn deliver_kill(pid: u32, force: bool) -> Result<(), nix::Error> {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;
        let sig = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let result = signal::kill(Pid::from_raw(pid as i32), sig);
        let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGCONT);
        result
    }

    fn handle_table_action(&mut self, action: TableAction, _ctx: &Context) {
        match action {
            TableAction::Kill { pid, name, force } => {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                // A second kill within the undo window must not drop the first
                // one on the floor (it would stay SIGSTOPped forever) — the
                // user asked for it and never undid it, so execute it now.
                if let Some(old) = self.pending_kill.take() {
                    let msg = match Self::deliver_kill(old.pid, old.force) {
                        Ok(_) => format!(
                            "{}illed {} ({}) — superseded by new kill",
                            if old.force { "Force k" } else { "K" },
                            old.name,
                            old.pid
                        ),
                        Err(e) => format!("Kill failed for {} ({}): {e}", old.name, old.pid),
                    };
                    if let Ok(mut s) = self.state.lock() {
                        s.append_log(msg);
                    }
                }
                let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGSTOP);
                self.pending_kill = Some(crate::gui::process_tab::PendingKill {
                    pid,
                    name: name.clone(),
                    force,
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                });
                if let Ok(mut s) = self.state.lock() {
                    s.append_log(format!(
                        "Suspended {} ({}) — will {} in 5s",
                        name,
                        pid,
                        if force { "force kill" } else { "kill" }
                    ));
                }
            }
            TableAction::Suspend { pid, name } => {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGSTOP);
                if let Ok(mut s) = self.state.lock() {
                    s.suspended_pids.insert(pid);
                    s.append_log(format!("Suspended {} ({})", name, pid));
                }
            }
            TableAction::Resume { pid, name } => {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pid as i32), Signal::SIGCONT);
                if let Ok(mut s) = self.state.lock() {
                    s.suspended_pids.remove(&pid);
                    s.append_log(format!("Resumed {} ({})", name, pid));
                }
            }
            TableAction::SetAffinity { pid, name, current } => {
                self.affinity_dialog = Some((pid, AffinityDialog::new(&current, &name)));
            }
            TableAction::SetNice { pid, name, current } => {
                self.nice_dialog = Some((pid, NiceDialog::new(current, &name)));
            }
            TableAction::SetIonice { pid, name } => {
                self.ionice_dialog = Some((pid, IoNiceDialog::new(&name)));
            }
            TableAction::AddRule { name } => {
                let mut rule = crate::rules::Rule::new_empty();
                rule.name = name.clone();
                rule.pattern = name;
                rule.match_type = "contains".into();
                self.rules_tab.open_add_dialog(Some(rule));
                self.active_tab = Tab::Rules;
            }
            TableAction::None => {}
        }
    }

    fn poll_dialogs(&mut self, ctx: &Context) {
        // Affinity dialog
        if let Some((pid, ref mut dlg)) = self.affinity_dialog {
            let proc_name = dlg.title.clone();
            if let Some(result) = dlg.show(ctx, self.opacity) {
                let cpulist = result.as_str();
                if !cpulist.is_empty() && utils::set_affinity(pid, cpulist) {
                    self.send(DaemonCmd::SetManualOverride {
                        pid,
                        duration_secs: 30.0,
                    });
                    if let Ok(mut s) = self.state.lock() {
                        s.append_log(format!("[Manual] affinity={cpulist} → PID {pid}"));
                    }
                    self.offer_rule(proc_name, Some(result.clone()), None, None);
                }
                self.affinity_dialog = None;
            }
        }

        // Nice dialog
        if let Some((pid, ref mut dlg)) = self.nice_dialog {
            let proc_name = dlg.title.clone();
            if let Some(result) = dlg.show(ctx, self.opacity) {
                if let Some(nice) = result {
                    if utils::set_nice(pid, nice) {
                        if let Ok(mut s) = self.state.lock() {
                            s.append_log(format!("[Manual] nice={nice} → PID {pid}"));
                        }
                        self.offer_rule(proc_name, None, Some(nice), None);
                    }
                }
                self.nice_dialog = None;
            }
        }

        // IoNice dialog
        if let Some((pid, ref mut dlg)) = self.ionice_dialog {
            let proc_name = dlg.title.clone();
            if let Some(result) = dlg.show(ctx, self.opacity) {
                if let Some((class, level)) = result {
                    if utils::set_ionice(pid, class, Some(level)) {
                        if let Ok(mut s) = self.state.lock() {
                            s.append_log(format!(
                                "[Manual] ionice class={class} level={level} → PID {pid}"
                            ));
                        }
                        self.offer_rule(proc_name, None, None, Some((class, level)));
                    }
                }
                self.ionice_dialog = None;
            }
        }

        // "Remember settings?" prompt for the latest manual change
        if let Some(offer) = &self.rule_offer {
            let mut create = false;
            let mut dismiss = false;
            egui::Window::new("Remember settings?")
                .id(egui::Id::new("rule_offer_window"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -40.0))
                .resizable(false)
                .collapsible(false)
                .show(ctx, |ui| {
                    ui.label(format!(
                        "Keep {} for '{}' with a rule?\nThe setting will be re-applied every time the process starts.",
                        offer.summary(),
                        offer.proc_name
                    ));
                    ui.horizontal(|ui| {
                        if ui.button("Create rule").clicked() {
                            create = true;
                        }
                        if ui.button("No thanks").clicked() {
                            dismiss = true;
                        }
                    });
                });
            if create {
                let offer = self.rule_offer.take().unwrap();
                let mut rule = crate::rules::Rule::new_empty();
                rule.name = offer.proc_name.clone();
                rule.pattern = offer.proc_name.clone();
                rule.match_type = "exact".into();
                rule.affinity = offer.affinity.clone();
                rule.nice = offer.nice;
                rule.ionice_class = offer.ionice.map(|(c, _)| c);
                rule.ionice_level = offer.ionice.map(|(_, l)| l);
                // Lock ORDER matters: the daemon nests state inside the rule
                // engine (engine → state via the log callback), so the GUI must
                // never nest engine inside state or the two deadlock. Collect
                // the rule list first, then take the state lock.
                let rules_cfg = if let Ok(mut re) = self.rule_engine.lock() {
                    re.add_rule(rule);
                    re.to_config_list()
                } else {
                    Vec::new()
                };
                if let Ok(mut s) = self.state.lock() {
                    s.config.rules = rules_cfg;
                    s.append_log(format!(
                        "[Rule] Created rule for '{}' ({}) from manual change",
                        offer.proc_name,
                        offer.summary()
                    ));
                }
                self.save_config();
                self.send(DaemonCmd::ReapplyDefaults);
            } else if dismiss {
                self.rule_offer = None;
            }
        }
    }
}

impl eframe::App for ArgusLassoApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // Repaint rate diagnostics — log repaints/sec approximately every 10s
        self.repaint_count += 1;
        let elapsed = self.last_repaint_log.elapsed();
        if elapsed >= std::time::Duration::from_secs(10) {
            let rate = self.repaint_count as f32 / elapsed.as_secs_f32();
            log::debug!(
                "repaint rate: {:.1}/sec ({} in {:.1}s)",
                rate,
                self.repaint_count,
                elapsed.as_secs_f32()
            );
            self.repaint_count = 0;
            self.last_repaint_log = std::time::Instant::now();
        }

        // Pull snapshot from shared state — lock held only for this clone block.
        // Expensive clones (log_lines, hw_monitor) only when the relevant tab is active.
        let on_log_tab = self.active_tab == Tab::Log;
        let on_hw_tab = self.active_tab == Tab::HwMonitor;
        let on_pb_tab = self.active_tab == Tab::ProBalance;
        let on_proc_tab = self.active_tab == Tab::Processes || self.active_tab == Tab::Overview;
        let on_overview_tab = self.active_tab == Tab::Overview;
        let (
            snapshot,
            cpu_pcts,
            cpu_gen,
            throttled_pids,
            suspended_pids,
            throttle_infos,
            log_lines,
            config,
            gaming_active,
            hw_monitor,
            proc_cpu_history,
            cpu_history,
            cpu_avg,
        ) = {
            if let Ok(s) = self.state.lock() {
                (
                    s.snapshot.clone(),
                    s.cpu_percents.clone(),
                    s.cpu_generation,
                    s.throttled_pids.clone(),
                    s.suspended_pids.clone(),
                    if on_pb_tab {
                        s.throttle_infos.clone()
                    } else {
                        Default::default()
                    },
                    if on_log_tab {
                        s.log_lines.clone()
                    } else {
                        Default::default()
                    },
                    s.config.clone(),
                    s.gaming_active,
                    if on_hw_tab {
                        s.hw_monitor.clone()
                    } else {
                        Default::default()
                    },
                    if on_proc_tab {
                        s.proc_cpu_history.clone()
                    } else {
                        Default::default()
                    },
                    if on_overview_tab {
                        s.cpu_history.clone()
                    } else {
                        Default::default()
                    },
                    s.cpu_avg,
                )
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
                return;
            }
        };

        self.proc_count = snapshot.len();
        self.throttled_count = throttled_pids.len();

        // Only push CPU bars + history when the daemon has emitted a new sample.
        // The hwmon temp scan (a full /sys/class/hwmon walk) also lives here —
        // it's far too expensive to run on every 60fps repaint.
        if cpu_gen != self.last_cpu_gen && !cpu_pcts.is_empty() {
            self.last_cpu_gen = cpu_gen;
            self.process_tab.update_cpu(cpu_pcts.clone());
            self.cpu_temp = read_cpu_temp();
        }

        // Poll active dialogs
        self.poll_dialogs(ctx);

        // Check pending kill
        if let Some(ref pk) = self.pending_kill {
            if std::time::Instant::now() >= pk.deadline {
                let name = pk.name.clone();
                let pid = pk.pid;
                let force = pk.force;
                let msg = match Self::deliver_kill(pid, force) {
                    Ok(_) => format!(
                        "{}illed {} ({})",
                        if force { "Force k" } else { "K" },
                        name,
                        pid
                    ),
                    Err(e) => format!("Kill failed for {} ({}): {e}", name, pid),
                };
                if config.ui.notifications_enabled {
                    let _ = notify_rust::Notification::new()
                        .summary("Argus-Lasso")
                        .body(&msg)
                        .timeout(notify_rust::Timeout::Milliseconds(3000))
                        .show();
                }
                if let Ok(mut s) = self.state.lock() {
                    s.append_log(msg);
                }
                self.pending_kill = None;
            }
        }

        // ── Top-level panels ─────────────────────────────────────────────
        // Build pending-kill display info before the panel closure (avoids borrow issues)
        let pending_kill_info: Option<(u32, String, u64)> = self.pending_kill.as_ref().map(|pk| {
            let remaining = pk
                .deadline
                .saturating_duration_since(std::time::Instant::now())
                .as_secs();
            (pk.pid, pk.name.clone(), remaining)
        });
        let mut undo_requested = false;

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("Processes: {}", self.proc_count));
                ui.separator();
                let avg = if cpu_pcts.is_empty() {
                    0.0
                } else {
                    cpu_pcts.iter().sum::<f32>() / cpu_pcts.len() as f32
                };
                ui.label(format!("CPU avg: {avg:.0}%"));
                if let Some(temp) = self.cpu_temp {
                    ui.separator();
                    ui.label(format!("CPU temp: {temp:.0}°C"));
                }
                if !self.cpu_model.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new(&self.cpu_model).weak());
                }
                ui.separator();
                if gaming_active {
                    ui.colored_label(crate::gui::theme::Breeze::POSITIVE, "⚡ Gaming Mode ACTIVE");
                }
                if let Some((_, ref kill_name, remaining)) = pending_kill_info {
                    ui.separator();
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 120, 60),
                        format!("Killing '{}' in {}s", kill_name, remaining + 1),
                    );
                    if ui.button("Undo").clicked() {
                        undo_requested = true;
                    }
                }
            });
        });

        if undo_requested {
            if let Some(ref pk) = self.pending_kill {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let _ = signal::kill(Pid::from_raw(pk.pid as i32), Signal::SIGCONT);
                let name = pk.name.clone();
                let pid = pk.pid;
                if let Ok(mut s) = self.state.lock() {
                    s.append_log(format!("Kill cancelled — resumed {} ({})", name, pid));
                }
            }
            self.pending_kill = None;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            // Tab bar
            ui.horizontal(|ui| {
                let proc_label = format!("Processes ({})", self.proc_count);
                let pb_label = if self.throttled_count > 0 {
                    format!("ProBalance ({})", self.throttled_count)
                } else {
                    "ProBalance".into()
                };

                for (label, tab) in [
                    ("Overview", Tab::Overview),
                    (proc_label.as_str(), Tab::Processes),
                    ("Rules", Tab::Rules),
                    (pb_label.as_str(), Tab::ProBalance),
                    ("Gaming Mode", Tab::GamingMode),
                    ("HW Monitor", Tab::HwMonitor),
                    ("Benchmark", Tab::Benchmark),
                    ("Settings", Tab::Settings),
                    ("Log", Tab::Log),
                ] {
                    let selected = self.active_tab == tab;
                    let text = if selected {
                        RichText::new(label)
                            .color(crate::gui::theme::Breeze::HIGHLIGHT)
                            .strong()
                    } else {
                        RichText::new(label) // inherits theme text color — readable on both dark and light
                    };
                    if ui.selectable_label(selected, text).clicked() {
                        self.active_tab = tab;
                    }
                }
            });
            ui.separator();

            // ── Tab content ──────────────────────────────────────────────
            match self.active_tab {
                Tab::Overview => {
                    self.overview_tab.show(ui, &cpu_history, cpu_avg, &snapshot);
                }

                Tab::Processes => {
                    let action = self.process_tab.show(
                        ui,
                        &snapshot,
                        &throttled_pids,
                        &suspended_pids,
                        &self.cmd_tx,
                        &self.rule_engine,
                        gaming_active,
                        &proc_cpu_history,
                    );
                    self.handle_table_action(action, ctx);
                    // Persist col_widths when user drags a column divider
                    if self.process_tab.cols_dirty {
                        if let Ok(mut s) = self.state.lock() {
                            s.config.ui.col_widths = self.process_tab.col_widths.clone();
                        }
                        self.save_config();
                    }
                }

                Tab::Rules => {
                    let mut rules_changed = false;
                    let mut profiles_changed = false;
                    let mut rule_profiles = config.rule_profiles.clone();
                    self.rules_tab.show(
                        ui,
                        ctx,
                        &self.rule_engine,
                        &mut rules_changed,
                        self.opacity,
                        &mut rule_profiles,
                        &mut profiles_changed,
                    );
                    if rules_changed {
                        // Never nest the engine lock inside the state lock —
                        // the daemon nests them the other way around.
                        let rules_cfg = self
                            .rule_engine
                            .lock()
                            .map(|re| re.to_config_list())
                            .unwrap_or_default();
                        if let Ok(mut s) = self.state.lock() {
                            s.config.rules = rules_cfg;
                        }
                        self.send(DaemonCmd::ReapplyDefaults);
                        self.save_config();
                    }
                    if profiles_changed {
                        if let Ok(mut s) = self.state.lock() {
                            s.config.rule_profiles = rule_profiles;
                        }
                        self.save_config();
                    }
                }

                Tab::ProBalance => {
                    if let Some(pb_cfg) = self.probalance_tab.show(ui, &snapshot, &throttle_infos) {
                        if let Ok(mut s) = self.state.lock() {
                            s.config.probalance = pb_cfg.clone();
                        }
                        let mut updated = config.clone();
                        updated.probalance = pb_cfg;
                        self.send(DaemonCmd::UpdateConfig(Box::new(updated)));
                        self.save_config();
                    }
                }

                Tab::GamingMode => {
                    self.gaming_mode_tab.show(ui, ctx, self.opacity);
                    // Drain events
                    let events: Vec<GamingEvent> = std::mem::take(&mut self.gaming_mode_tab.events);
                    for event in events {
                        match event {
                            GamingEvent::GamingModeChanged {
                                active,
                                elevate_nice,
                            } => {
                                self.send(DaemonCmd::SetGamingMode {
                                    active,
                                    elevate_nice,
                                    park: false,
                                });
                                if active {
                                    self.send(DaemonCmd::ReapplyDefaults);
                                }
                            }
                            GamingEvent::ResetAll => {
                                self.send(DaemonCmd::ResetAffinities);
                            }
                            GamingEvent::LogMessage(msg) => {
                                if let Ok(mut s) = self.state.lock() {
                                    s.append_log(msg);
                                }
                            }
                            GamingEvent::ConfigChanged(cfg) => {
                                if let Ok(mut s) = self.state.lock() {
                                    s.config.clone_from(&cfg);
                                }
                                self.send(DaemonCmd::UpdateConfig(cfg));
                                self.save_config();
                            }
                        }
                    }
                }

                Tab::HwMonitor => {
                    self.hw_monitor_tab.show(ui, &hw_monitor);
                    if self.hw_monitor_tab.cols_dirty {
                        let widths = self.hw_monitor_tab.col_widths.to_vec();
                        if let Ok(mut s) = self.state.lock() {
                            s.config.ui.hw_mon_col_widths = widths;
                        }
                        self.save_config();
                    }
                }

                Tab::Benchmark => {
                    self.bench_tab.show(ui);
                }

                Tab::Settings => {
                    let config_changed = self.settings_tab.show(ui, ctx, self.opacity);

                    // Live opacity preview — apply every frame the slider moves,
                    // regardless of whether the Apply button was clicked.
                    let new_opacity = self.settings_tab.opacity;
                    if (new_opacity - self.opacity).abs() > 0.001 {
                        self.opacity = new_opacity;
                        eprintln!("[opacity] applying opacity={new_opacity:.3}");
                        if let Some(ref wo) = self.wayland_opacity {
                            wo.set(new_opacity);
                        } else {
                            // Fallback: control opacity via window_fill alpha so the
                            // compositor sees a semi-transparent clear colour.
                            let alpha = (new_opacity * 255.0) as u8;
                            let theme = &self.settings_tab.theme;
                            ctx.style_mut(|s| {
                                let (r, g, b) = crate::gui::theme::window_bg_rgb(theme);
                                let col = egui::Color32::from_rgba_unmultiplied(r, g, b, alpha);
                                s.visuals.window_fill = col;
                                s.visuals.panel_fill = col;
                            });
                        }
                    }

                    if let Some(updated) = config_changed {
                        if let Ok(mut s) = self.state.lock() {
                            s.config = updated.clone();
                        }
                        // Re-apply full theme (resets window_fill to opaque if needed)
                        crate::gui::theme::apply_theme(
                            ctx,
                            self.native_ppp,
                            &self.settings_tab.theme,
                        );
                        // Then re-apply opacity on top of the fresh theme
                        if let Some(ref wo) = self.wayland_opacity {
                            wo.set(self.opacity);
                        }
                        self.send(DaemonCmd::UpdateConfig(Box::new(updated.clone())));
                        self.send(DaemonCmd::ReapplyDefaults);
                        self.last_saved_opacity = self.settings_tab.opacity;
                        self.last_saved_theme = self.settings_tab.theme.to_str().to_string();
                        self.save_config();
                    }

                    // Detect live theme/opacity changes and persist immediately (no Apply needed)
                    let cur_opacity = self.settings_tab.opacity;
                    let cur_theme = self.settings_tab.theme.to_str().to_string();
                    if (cur_opacity - self.last_saved_opacity).abs() > 0.001
                        || cur_theme != self.last_saved_theme
                    {
                        self.last_saved_opacity = cur_opacity;
                        self.last_saved_theme = cur_theme.clone();
                        if let Ok(mut s) = self.state.lock() {
                            s.config.ui.opacity = cur_opacity;
                            s.config.ui.theme = cur_theme;
                        }
                        self.save_config();
                    }
                }

                Tab::Log => {
                    let (clear, save) = self.log_tab.show_with_clear(ui, &log_lines);
                    if clear {
                        if let Ok(mut s) = self.state.lock() {
                            s.log_lines.clear();
                        }
                    }
                    if save {
                        // Run the picker + write on a background thread — the
                        // dialog subprocess blocks until closed and would
                        // freeze the whole UI (same pattern as rules import).
                        let content = log_lines.iter().cloned().collect::<Vec<_>>().join("\n");
                        let state = self.state.clone();
                        std::thread::spawn(move || {
                            if let Some(p) =
                                crate::file_dialog::save("argus-lasso.log", "*.log *.txt")
                            {
                                let msg = match std::fs::write(&p, content) {
                                    Ok(_) => format!("Log saved to {}", p.display()),
                                    Err(e) => format!("Log save FAILED: {e}"),
                                };
                                if let Ok(mut s) = state.lock() {
                                    s.append_log(msg);
                                }
                            }
                        });
                    }
                }
            }
        });

        // Repaint when next display refresh is due — avoids continuous 60fps rendering.
        // While a kill countdown is pending, repaint fast enough that the
        // countdown updates and the SIGTERM actually fires near its deadline
        // (with a long refresh interval it could otherwise fire seconds late).
        let repaint_ms = if self.pending_kill.is_some() {
            250
        } else {
            config.monitor.display_refresh_interval_ms
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(repaint_ms));
    }
}
