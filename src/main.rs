//! Argus-Lasso Linux — Rust edition entry point.

mod app;
mod config;
mod cpu_park;
mod file_dialog;
mod gui;
mod hw_monitor;
mod icon;
mod mem_bench;
mod monitor;
mod probalance;
mod rules;
mod utils;
mod wayland_opacity;

use std::sync::{Arc, Mutex};

use clap::Parser;

// ── App icon (embedded at compile time from assets/icon.png via build.rs) ─────

const ICON_RGBA_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon_rgba.bin"));
const ICON_W: u32 = 64;
const ICON_H: u32 = 64;

fn make_icon_rgba() -> Vec<u8> {
    ICON_RGBA_BYTES.to_vec()
}

// ── System tray (KDE/freedesktop StatusNotifierItem via D-Bus) ─────────────────

struct ArgusLassoTray {
    state:  Arc<Mutex<monitor::AppState>>,
    cmd_tx: crossbeam_channel::Sender<monitor::DaemonCmd>,
}

/// Convert embedded RGBA bytes to ARGB32 network-byte-order as required by D-Bus SNI.
fn make_tray_icon() -> ksni::Icon {
    let mut data = crate::icon::RGBA.to_vec();
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1); // [R,G,B,A] → [A,R,G,B]
    }
    ksni::Icon {
        width:  crate::icon::W as i32,
        height: crate::icon::H as i32,
        data,
    }
}

impl ksni::Tray for ArgusLassoTray {
    fn id(&self) -> String {
        "argus-lasso".into()
    }
    fn icon_name(&self) -> String {
        // Named icon in the system theme (works after `make install`).
        // icon_pixmap() provides the embedded fallback.
        "argus-lasso".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![make_tray_icon()]
    }
    fn title(&self) -> String {
        let avg = self.state.lock()
            .map(|s| s.cpu_avg)
            .unwrap_or(0.0);
        format!("Argus-Lasso  CPU {avg:.0}%")
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let avg = self.state.lock().map(|s| s.cpu_avg).unwrap_or(0.0);
        ksni::ToolTip {
            title: format!("Argus-Lasso — CPU {avg:.0}%"),
            description: "Right-click for options".into(),
            icon_name: String::new(),
            icon_pixmap: vec![make_tray_icon()],
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        let gaming_active = self.state.lock()
            .map(|s| s.gaming_active)
            .unwrap_or(false);

        vec![
            ksni::MenuItem::Checkmark(ksni::menu::CheckmarkItem {
                label:   "Gaming Mode".into(),
                checked: gaming_active,
                activate: Box::new(|tray: &mut Self| {
                    let currently = tray.state.lock()
                        .map(|s| s.gaming_active)
                        .unwrap_or(false);
                    let _ = tray.cmd_tx.send(monitor::DaemonCmd::SetGamingMode {
                        active:       !currently,
                        elevate_nice: true,
                        park:         true,
                    });
                }),
                ..Default::default()
            }),
            ksni::MenuItem::Separator,
            ksni::MenuItem::Standard(ksni::menu::StandardItem {
                label:    "Quit".into(),
                activate: Box::new(|_| std::process::exit(0)),
                ..Default::default()
            }),
        ]
    }
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Kill a process by PID (sends SIGTERM, or SIGKILL with --force)
    Kill {
        /// PID to kill
        pid: u32,
        /// Use SIGKILL instead of SIGTERM
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// Set CPU affinity for a process by PID
    SetAffinity {
        /// PID to modify
        pid: u32,
        /// CPU list, e.g. "0-7" or "0,2,4"
        mask: String,
    },
}

#[derive(Parser, Debug)]
#[command(name = "argus-lasso", about = "Argus-Lasso — Linux process manager")]
struct Args {
    /// Start minimised to system tray
    #[arg(long, default_value_t = false)]
    minimized: bool,

    /// Disable system tray icon
    #[arg(long, default_value_t = false)]
    no_tray: bool,

    #[command(subcommand)]
    command: Option<Cmd>,
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    // Handle CLI subcommands — run action and exit without launching the GUI.
    if let Some(cmd) = args.command {
        match cmd {
            Cmd::Kill { pid, force } => {
                use nix::sys::signal::{self, Signal};
                use nix::unistd::Pid;
                let sig = if force { Signal::SIGKILL } else { Signal::SIGTERM };
                match signal::kill(Pid::from_raw(pid as i32), sig) {
                    Ok(_) => println!("{}illed PID {pid}", if force { "Force k" } else { "K" }),
                    Err(e) => { eprintln!("Kill failed: {e}"); std::process::exit(1); }
                }
                return;
            }
            Cmd::SetAffinity { pid, mask } => {
                if utils::set_affinity(pid, &mask) {
                    println!("Affinity set to '{mask}' for PID {pid}");
                } else {
                    eprintln!("Failed to set affinity for PID {pid}");
                    std::process::exit(1);
                }
                return;
            }
        }
    }

    // Build icon RGBA once; reused for window decoration icon.
    let icon_rgba = make_icon_rgba();

    // Load config
    let cfg = config::load();

    // Build shared state
    let state = Arc::new(Mutex::new(monitor::AppState::default()));
    {
        if let Ok(mut s) = state.lock() {
            s.config = cfg.clone();
            s.cpu_model = monitor::read_cpu_model();
        }
    }

    // Build rule engine
    let rule_engine = {
        let mut re = rules::RuleEngine::new();
        let state_clone = state.clone();
        re.set_log_callback(move |msg| {
            if let Ok(mut s) = state_clone.lock() {
                s.append_log(msg);
            }
        });
        re.load_rules(&cfg.rules);
        Arc::new(Mutex::new(re))
    };

    // Spawn daemon thread
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded();
    monitor::spawn(Arc::clone(&state), cmd_rx, cfg.clone(), Arc::clone(&rule_engine));

    // System tray via D-Bus StatusNotifierItem (KDE/freedesktop, no libxdo).
    // Spawned after state + cmd_tx exist so the menu can read/toggle gaming mode.
    let _tray_handle = if !args.no_tray {
        use ksni::blocking::TrayMethods;
        match (ArgusLassoTray {
            state:  Arc::clone(&state),
            cmd_tx: cmd_tx.clone(),
        }).spawn() {
            Ok(h) => {
                log::info!("SNI tray icon registered");
                Some(h)
            }
            Err(e) => {
                log::warn!("Tray icon unavailable: {e}");
                None
            }
        }
    } else {
        None
    };

    // Launch GUI
    // transparent: true enables per-pixel alpha compositing on Wayland/X11 so the
    // fallback opacity path (ctx.visuals window_fill alpha) works when the compositor
    // does not support wp_alpha_modifier_v1.
    let window_icon = egui::IconData {
        rgba: icon_rgba,
        width: ICON_W,
        height: ICON_H,
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Argus-Lasso — Linux")
            // app_id must match the .desktop filename (argus-lasso.desktop)
            // so KDE/KWin resolves Icon=argus-lasso from that file.
            .with_app_id("argus-lasso")
            .with_inner_size([1100.0, 700.0])
            .with_min_inner_size([800.0, 500.0])
            .with_transparent(true)
            .with_visible(!args.minimized)
            .with_icon(window_icon),
        ..Default::default()
    };

    let state_gui   = Arc::clone(&state);
    let re_gui      = Arc::clone(&rule_engine);
    let cfg_gui     = cfg.clone();
    let cmd_tx_gui  = cmd_tx.clone();

    eframe::run_native(
        "Argus-Lasso",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(app::ArgusLassoApp::new(
                cc,
                state_gui,
                cmd_tx_gui,
                re_gui,
                cfg_gui,
            )))
        }),
    )
    .expect("eframe launch failed");
}
