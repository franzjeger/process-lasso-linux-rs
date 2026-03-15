//! Gaming Mode tab: CPU topology display, parking, game launcher, profiles.

use std::collections::{HashMap, HashSet};
use egui::{Color32, RichText, Ui};

use crate::config::{Config, GamingProfile};
use crate::cpu_park::{
    self, detect_topology, get_smt_siblings_of, is_helper_current,
    is_helper_installed, is_sudoers_installed, park_cpus, unpark_all, CpuTopology,
};
use crate::utils::{get_offline_cpus, get_online_cpus};

// ── Events emitted from this tab ──────────────────────────────────────────────

pub enum GamingEvent {
    GamingModeChanged { active: bool, elevate_nice: bool },
    ResetAll,
    LogMessage(String),
    ConfigChanged(Config),
}

// ── Launcher watch phase ──────────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
pub(crate) enum WatchPhase {
    Idle,
    Waiting,
    Running,
}

// ── GamingModeTab ─────────────────────────────────────────────────────────────

pub struct GamingModeTab {
    pub config: Config,
    pub topo: Option<CpuTopology>,
    pub topo_description: String,
    pub parked: bool,
    pub parking_in_progress: bool,

    // Preferred CCD checkbox grid: cpu_num → checked
    pub preferred_checks: HashMap<u32, bool>,
    pub smt_siblings: HashSet<u32>,

    // Helper status
    pub helper_status_text: String,
    pub helper_ok: bool,

    // Nice elevation
    pub elevate_nice: bool,

    // CPU status line (None = use theme default text color)
    pub cpu_status_text: String,
    pub cpu_status_color: Option<Color32>,

    // Log (local to tab)
    pub log_lines: Vec<String>,

    // Game launcher
    pub game_name: String,
    pub command: String,
    pub auto_restore: bool,
    pub watch_phase: WatchPhase,
    pub launched_pid: Option<u32>,
    pub watch_status: String,
    pub last_poll: std::time::Instant,

    // Profiles
    pub selected_profile: String,

    // Dialogs
    pub install_password: String,
    pub show_install_dialog: bool,
    steam_picker: Option<crate::gui::dialogs::SteamGamePickerDialog>,
    lutris_picker: Option<crate::gui::dialogs::LutrisGamePickerDialog>,

    // Pending re-enable after unpark (profile switch)
    pending_enable_after_unpark: bool,

    // Events to emit to app.rs
    pub events: Vec<GamingEvent>,
}

impl GamingModeTab {
    pub fn new(config: Config) -> Self {
        let topo = detect_topology();
        let topo_description = topo.description.clone();
        let offline = get_offline_cpus();

        let all_cpus: HashSet<u32> = topo.preferred.iter().copied().collect();
        let smt_siblings = get_smt_siblings_of(&all_cpus);

        let mut preferred_checks: HashMap<u32, bool> = HashMap::new();
        for &cpu in &topo.preferred {
            preferred_checks.insert(cpu, !offline.contains(&cpu));
        }

        let parked = !offline.is_empty();
        let helper_ok = is_helper_current() && is_sudoers_installed();

        let mut tab = Self {
            config,
            topo: Some(topo),
            topo_description,
            parked,
            parking_in_progress: false,
            preferred_checks,
            smt_siblings,
            helper_status_text: String::new(),
            helper_ok,
            elevate_nice: true,
            cpu_status_text: String::new(),
            cpu_status_color: None,
            log_lines: Vec::new(),
            game_name: String::new(),
            command: String::new(),
            auto_restore: true,
            watch_phase: WatchPhase::Idle,
            launched_pid: None,
            watch_status: String::new(),
            last_poll: std::time::Instant::now(),
            selected_profile: String::new(),
            install_password: String::new(),
            show_install_dialog: false,
            steam_picker: None,
            lutris_picker: None,
            pending_enable_after_unpark: false,
            events: Vec::new(),
        };
        tab.refresh_helper_status();
        tab.refresh_cpu_status();

        if parked {
            tab.events.push(GamingEvent::GamingModeChanged {
                active: true,
                elevate_nice: tab.elevate_nice,
            });
        }

        tab
    }

    fn refresh_helper_status(&mut self) {
        self.helper_ok = is_helper_current() && is_sudoers_installed();
        self.helper_status_text = if self.helper_ok {
            "✓ Helper installed — parking + nice -1 available".into()
        } else if is_helper_installed() && is_sudoers_installed() {
            "⚠ Helper needs update — click 'Install / Update Helper'".into()
        } else {
            "✗ Helper not installed — click 'Install / Update Helper' to enable parking".into()
        };
    }

    fn refresh_cpu_status(&mut self) {
        let online = get_online_cpus().into_iter().collect::<std::collections::BTreeSet<_>>();
        let offline = get_offline_cpus().into_iter().collect::<std::collections::BTreeSet<_>>();
        if offline.is_empty() {
            self.cpu_status_text = format!("All CPUs online: {online:?}");
            self.cpu_status_color = None; // theme default text color
        } else {
            self.cpu_status_text = format!("Online: {online:?}  |  Offline (parked): {offline:?}");
            self.cpu_status_color = Some(crate::gui::theme::Breeze::WARNING);
        }
    }

    fn enable_gaming_mode(&mut self) {
        if let Some(ref topo) = self.topo.clone() {
            if !topo.has_asymmetry() { return; }
            if !is_helper_installed() {
                self.append_log("[Gaming Mode] Helper missing — install first.".into());
                return;
            }
            let unchecked: HashSet<u32> = self.preferred_checks.iter()
                .filter(|(_, &checked)| !checked)
                .map(|(&cpu, _)| cpu)
                .collect();
            let to_park: HashSet<u32> = topo.non_preferred.iter().copied()
                .chain(unchecked.into_iter())
                .collect();
            self.append_log(format!("[Gaming Mode] Parking CPUs {:?}…", {
                let mut v: Vec<_> = to_park.iter().copied().collect();
                v.sort_unstable();
                v
            }));
            self.parking_in_progress = true;

            // Park synchronously (blocking — parking is fast, sub-second)
            let log_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let ll = log_lines.clone();
            let ok = park_cpus(&to_park, move |msg| {
                ll.lock().unwrap().push(msg);
            });
            for msg in log_lines.lock().unwrap().drain(..) {
                self.append_log(msg);
            }
            self.parking_in_progress = false;
            self.parked = ok;
            self.refresh_cpu_status();
            if ok {
                self.append_log("[Gaming Mode] ACTIVE — non-preferred CPUs offline.".into());
                self.events.push(GamingEvent::GamingModeChanged {
                    active: true,
                    elevate_nice: self.elevate_nice,
                });
                self.events.push(GamingEvent::LogMessage("[Gaming Mode] enabled".into()));
            } else {
                self.append_log("[Gaming Mode] Parking failed — check log.".into());
            }
        }
    }

    fn disable_gaming_mode(&mut self) {
        self.append_log("[Gaming Mode] Unparking all CPUs…".into());
        let log_lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let ll = log_lines.clone();
        let _ok = unpark_all(move |msg| { ll.lock().unwrap().push(msg); });
        for msg in log_lines.lock().unwrap().drain(..) {
            self.append_log(msg);
        }
        self.parked = false;
        self.refresh_cpu_status();
        // Re-detect topology now all CPUs are back online
        let topo = detect_topology();
        self.topo_description = topo.description.clone();
        self.rebuild_preferred_checks(&topo);
        self.topo = Some(topo);
        self.append_log("[Gaming Mode] Disabled — all CPUs online.".into());
        self.events.push(GamingEvent::GamingModeChanged { active: false, elevate_nice: false });
        self.events.push(GamingEvent::LogMessage("[Gaming Mode] disabled".into()));

        if self.pending_enable_after_unpark {
            self.pending_enable_after_unpark = false;
            self.enable_gaming_mode();
        }
    }

    fn rebuild_preferred_checks(&mut self, topo: &CpuTopology) {
        let offline = get_offline_cpus();
        self.smt_siblings = get_smt_siblings_of(&topo.preferred);
        self.preferred_checks.clear();
        for &cpu in &topo.preferred {
            self.preferred_checks.insert(cpu, !offline.contains(&cpu));
        }
    }

    fn append_log(&mut self, msg: String) {
        self.log_lines.push(msg.clone());
        if self.log_lines.len() > 200 {
            self.log_lines.drain(0..self.log_lines.len() - 200);
        }
        self.events.push(GamingEvent::LogMessage(msg));
    }

    fn poll_game_process(&mut self) {
        if self.watch_phase == WatchPhase::Idle { return; }
        if self.last_poll.elapsed().as_secs_f32() < if self.watch_phase == WatchPhase::Running { 5.0 } else { 2.0 } {
            return;
        }
        self.last_poll = std::time::Instant::now();

        let pids: Vec<u32> = std::fs::read_dir("/proc")
            .ok()
            .map(|d| d.filter_map(|e| e.ok().and_then(|e| e.file_name().to_str().and_then(|s| s.parse().ok()))).collect())
            .unwrap_or_default();

        let name = self.game_name.clone();

        if self.watch_phase == WatchPhase::Waiting {
            for &pid in &pids {
                if proc_name_matches(&name, pid) {
                    self.launched_pid = Some(pid);
                    self.watch_phase = WatchPhase::Running;
                    self.watch_status = format!("Game running (PID {pid})");
                    self.append_log(format!("[Launcher] Game process found: PID {pid}"));
                    return;
                }
            }
        } else if self.watch_phase == WatchPhase::Running {
            if let Some(pid) = self.launched_pid {
                if !pids.contains(&pid) {
                    // Check for replacement
                    if let Some(new_pid) = pids.iter().find(|&&p| proc_name_matches(&name, p)).copied() {
                        self.launched_pid = Some(new_pid);
                        self.append_log(format!("[Launcher] Game PID changed → {new_pid}"));
                    } else {
                        self.append_log(format!("[Launcher] Game (PID {pid}) exited."));
                        if self.auto_restore && self.parked {
                            self.disable_gaming_mode();
                        }
                        self.watch_phase = WatchPhase::Idle;
                        self.launched_pid = None;
                        self.watch_status = String::new();
                    }
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut Ui, ctx: &egui::Context, opacity: f32) {
        self.events.clear();
        self.poll_game_process();

        egui::ScrollArea::vertical().show(ui, |ui| {
            // ── Topology info ─────────────────────────────────────────────
            ui.label(RichText::new("CPU Topology").strong());
            ui.label(&self.topo_description);
            ui.add_space(8.0);
            ui.separator();

            // ── Gaming Mode / CPU Parking ─────────────────────────────────
            ui.label(RichText::new("Gaming Mode — CPU Parking").strong());
            ui.label("Parks non-preferred CPUs so the game initialises its thread pool correctly.\nMirrors gamemoderun: AMD X3D → parks non-V-Cache CCD, Intel Hybrid → parks E-cores.");
            ui.add_space(4.0);

            let has_asym = self.topo.as_ref().map(|t| t.has_asymmetry()).unwrap_or(false);

            // Preferred CCD checkboxes
            if has_asym {
                ui.label(RichText::new("Preferred CCD — Active Cores in Gaming Mode").strong());
                ui.label("Uncheck any CPU to park it along with the non-preferred CCD.");

                let mut cpus: Vec<u32> = self.preferred_checks.keys().copied().collect();
                cpus.sort_unstable();

                ui.horizontal_wrapped(|ui| {
                    for cpu in &cpus {
                        let is_smt = self.smt_siblings.contains(cpu);
                        let label = if is_smt { format!("CPU {cpu} (HT)") } else { format!("CPU {cpu}") };
                        let checked = self.preferred_checks.get_mut(cpu).unwrap();
                        ui.checkbox(checked, label);
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("All").clicked() {
                        for v in self.preferred_checks.values_mut() { *v = true; }
                    }
                    let has_smt = !self.smt_siblings.is_empty();
                    ui.add_enabled_ui(has_smt, |ui| {
                        if ui.button("No SMT (physical only)").clicked() {
                            for (&cpu, v) in &mut self.preferred_checks {
                                *v = !self.smt_siblings.contains(&cpu);
                            }
                        }
                    });
                    if ui.button("None").clicked() {
                        for v in self.preferred_checks.values_mut() { *v = false; }
                    }
                });
                ui.add_space(4.0);
            }

            // Helper status
            let helper_color = if self.helper_ok {
                crate::gui::theme::Breeze::POSITIVE
            } else if is_helper_installed() && is_sudoers_installed() {
                crate::gui::theme::Breeze::WARNING
            } else {
                crate::gui::theme::Breeze::NEGATIVE
            };
            ui.horizontal(|ui| {
                ui.colored_label(helper_color, &self.helper_status_text);
                if ui.button("Install / Update Helper (root)").clicked() {
                    self.show_install_dialog = true;
                }
            });

            ui.checkbox(&mut self.elevate_nice, "Elevate game priority (nice -1) — gives game processes higher scheduling priority");
            ui.add_space(4.0);

            // Enable/disable button
            let btn_text = if self.parked {
                "⏹  Disable Gaming Mode (Unpark CPUs)"
            } else {
                "▶  Enable Gaming Mode (Park non-preferred CPUs)"
            };
            let btn_color = if self.parked {
                egui::Color32::from_rgb(30, 74, 42)
            } else {
                egui::Color32::from_rgb(76, 29, 149)
            };
            let enabled = has_asym && self.helper_ok && !self.parking_in_progress;
            ui.add_enabled_ui(enabled, |ui| {
                let btn = egui::Button::new(RichText::new(btn_text).strong().color(Color32::WHITE))
                    .min_size(egui::vec2(ui.available_width(), 40.0))
                    .fill(btn_color);
                if ui.add(btn).clicked() {
                    if self.parked { self.disable_gaming_mode(); } else { self.enable_gaming_mode(); }
                }
            });

            let status_color = self.cpu_status_color.unwrap_or_else(|| ui.visuals().text_color());
            ui.colored_label(status_color, &self.cpu_status_text);

            ui.add_space(8.0);
            ui.separator();

            // ── Reset All ─────────────────────────────────────────────────
            ui.label(RichText::new("Reset All Changes").strong());
            ui.label("Restores all per-process CPU affinities and unparks any parked CPUs.");
            if ui.button("↩  Reset All Changes").clicked() {
                if self.parked {
                    self.events.push(GamingEvent::GamingModeChanged { active: false, elevate_nice: false });
                    self.parked = false;
                }
                if !get_offline_cpus().is_empty() {
                    self.append_log("[Reset] Unparking CPUs…".into());
                    let ll = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
                    let l2 = ll.clone();
                    unpark_all(move |m| l2.lock().unwrap().push(m));
                    for m in ll.lock().unwrap().drain(..) { self.append_log(m); }
                    self.refresh_cpu_status();
                    let topo = detect_topology();
                    self.rebuild_preferred_checks(&topo);
                    self.topo_description = topo.description.clone();
                    self.topo = Some(topo);
                }
                self.events.push(GamingEvent::ResetAll);
            }

            ui.add_space(8.0);
            ui.separator();

            // ── Game Launcher ─────────────────────────────────────────────
            ui.label(RichText::new("Game Launcher").strong());

            // Profile combo
            ui.horizontal(|ui| {
                ui.label("Profile:");
                let profiles = self.config.gaming_mode.profiles.keys().cloned().collect::<Vec<_>>();
                egui::ComboBox::from_id_salt("profile_combo")
                    .selected_text(&self.selected_profile)
                    .show_ui(ui, |ui| {
                        for name in &profiles {
                            if ui.selectable_label(*name == self.selected_profile, name).clicked() {
                                self.selected_profile = name.clone();
                                self.load_profile(name);
                            }
                        }
                    });
                if ui.button("Save").clicked() {
                    self.save_profile();
                }
                if ui.button("Delete").clicked() {
                    let name = self.selected_profile.clone();
                    self.config.gaming_mode.profiles.remove(&name);
                    self.selected_profile.clear();
                    self.events.push(GamingEvent::ConfigChanged(self.config.clone()));
                }
            });

            // Game name
            ui.horizontal(|ui| {
                ui.label("Game:");
                ui.text_edit_singleline(&mut self.game_name);
                if ui.button("Steam…").clicked() {
                    self.steam_picker = Some(crate::gui::dialogs::SteamGamePickerDialog::new());
                }
                if ui.button("Lutris…").clicked() {
                    self.lutris_picker = Some(crate::gui::dialogs::LutrisGamePickerDialog::new());
                }
            });

            // Command
            ui.horizontal(|ui| {
                ui.label("Command:");
                ui.text_edit_singleline(&mut self.command);
            });

            // Launch row
            ui.horizontal(|ui| {
                let can_launch = !self.game_name.is_empty() && !self.command.is_empty();
                ui.add_enabled_ui(can_launch, |ui| {
                    if ui.button("▶  Launch").clicked() {
                        self.launch_game();
                    }
                });
                ui.checkbox(&mut self.auto_restore, "Auto-disable Gaming Mode when game exits");
            });

            // Kill / watch status
            ui.horizontal(|ui| {
                let can_kill = self.watch_phase != WatchPhase::Idle;
                ui.add_enabled_ui(can_kill, |ui| {
                    if ui.button("⏹ Kill Game").clicked() {
                        if let Some(pid) = self.launched_pid {
                            let _ = nix::sys::signal::kill(
                                nix::unistd::Pid::from_raw(pid as i32),
                                nix::sys::signal::Signal::SIGTERM,
                            );
                            self.append_log(format!("[Launcher] Sent SIGTERM to PID {pid}"));
                        }
                        if self.auto_restore && self.parked { self.disable_gaming_mode(); }
                        self.watch_phase = WatchPhase::Idle;
                        self.launched_pid = None;
                        self.watch_status = String::new();
                    }
                });
                if !self.watch_status.is_empty() {
                    ui.colored_label(crate::gui::theme::Breeze::POSITIVE, &self.watch_status);
                }
            });

            ui.add_space(8.0);
            ui.separator();

            // ── Log ───────────────────────────────────────────────────────
            egui::ScrollArea::vertical().max_height(120.0).stick_to_bottom(true).show(ui, |ui| {
                for line in &self.log_lines {
                    ui.label(line);
                }
            });
        });

        // ── Install helper dialog ─────────────────────────────────────────
        if self.show_install_dialog {
            egui::Window::new("Install Privileged Helper")
                .resizable(false).collapsible(false)
                .show(ctx, |ui| {
                    ui.label("Enter root password to install the privileged sysfs helper:");
                    ui.add(egui::TextEdit::singleline(&mut self.install_password).password(true));
                    ui.horizontal(|ui| {
                        if ui.button("Install").clicked() {
                            let password = self.install_password.clone();
                            self.install_password.clear();
                            self.show_install_dialog = false;
                            self.append_log("Installing privileged helper…".into());
                            let (_ok, msg) = cpu_park::install_helper_as_root("", &password);
                            self.append_log(msg);
                            self.refresh_helper_status();
                        }
                        if ui.button("Cancel").clicked() {
                            self.install_password.clear();
                            self.show_install_dialog = false;
                        }
                    });
                });
        }

        // ── Steam picker ──────────────────────────────────────────────────
        if let Some(ref mut picker) = self.steam_picker {
            if let Some(result) = picker.show(ctx, opacity) {
                if let Some((appid, name)) = result {
                    self.game_name = name;
                    self.command = format!("steam -applaunch {appid}");
                }
                self.steam_picker = None;
            }
        }

        // ── Lutris picker ─────────────────────────────────────────────────
        if let Some(ref mut picker) = self.lutris_picker {
            if let Some(result) = picker.show(ctx, opacity) {
                if let Some((slug, name)) = result {
                    self.game_name = name;
                    self.command = format!("lutris lutris:rungame/{slug}");
                }
                self.lutris_picker = None;
            }
        }
    }

    fn load_profile(&mut self, name: &str) {
        if let Some(profile) = self.config.gaming_mode.profiles.get(name).cloned() {
            self.game_name = profile.game_name.clone();
            self.command = profile.command.clone();
            self.elevate_nice = profile.elevate_nice;
            for (&cpu, checked) in &mut self.preferred_checks {
                if let Some(&v) = profile.cpu_states.get(&cpu.to_string()) {
                    *checked = v;
                }
            }
            self.append_log(format!("[Profile] Loaded '{name}' — {}", profile.command));
            if self.parked {
                self.append_log(format!("[Profile] Re-applying CPU parking for '{name}'…"));
                self.disable_gaming_mode();
                self.pending_enable_after_unpark = true;
            }
        }
    }

    fn save_profile(&mut self) {
        let name = if self.selected_profile.is_empty() {
            self.game_name.clone()
        } else {
            self.selected_profile.clone()
        };
        if name.is_empty() { return; }

        let cpu_states: HashMap<String, bool> = self.preferred_checks.iter()
            .map(|(&k, &v)| (k.to_string(), v))
            .collect();

        self.config.gaming_mode.profiles.insert(name.clone(), GamingProfile {
            game_name: self.game_name.clone(),
            command: self.command.clone(),
            cpu_states,
            elevate_nice: self.elevate_nice,
        });
        self.selected_profile = name.clone();
        self.events.push(GamingEvent::ConfigChanged(self.config.clone()));
        self.append_log(format!("[Profile] Saved '{name}'"));
    }

    fn launch_game(&mut self) {
        if !self.parked { self.enable_gaming_mode(); }

        let cmd = self.command.clone();
        self.append_log(format!("[Launcher] Launching '{}': {cmd}", self.game_name));
        self.watch_phase = WatchPhase::Waiting;
        self.watch_status = "Waiting for game process…".into();
        self.last_poll = std::time::Instant::now();

        // Spawn detached
        let parts: Vec<_> = cmd.split_whitespace().collect();
        if let Some((prog, args)) = parts.split_first() {
            let _ = std::process::Command::new(prog).args(args).spawn();
        }
    }
}

fn proc_name_matches(game_name: &str, pid: u32) -> bool {
    let norm = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect()
    };
    let name_n = norm(game_name);
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
    let comm_n = norm(comm.trim());
    if name_n.contains(&comm_n) || comm_n.contains(&name_n) {
        return true;
    }
    // Fallback: cmdline
    if let Ok(cmdline) = std::fs::read_to_string(format!("/proc/{pid}/cmdline")) {
        if norm(&cmdline).contains(&name_n) {
            return true;
        }
    }
    false
}
