//! Process Lasso Linux — Rust edition entry point.

mod app;
mod config;
mod cpu_park;
mod gui;
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

struct ProcessLassoTray {
    state:  Arc<Mutex<monitor::AppState>>,
    cmd_tx: crossbeam_channel::Sender<monitor::DaemonCmd>,
}

impl ksni::Tray for ProcessLassoTray {
    fn id(&self) -> String {
        "process-lasso-linux".into()
    }
    fn icon_name(&self) -> String {
        "process-lasso-linux".into()
    }
    fn title(&self) -> String {
        "Process Lasso".into()
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

#[derive(Parser, Debug)]
#[command(name = "process-lasso", about = "Process Lasso — Linux process manager")]
struct Args {
    /// Start minimised to system tray
    #[arg(long, default_value_t = false)]
    minimized: bool,

    /// Disable system tray icon
    #[arg(long, default_value_t = false)]
    no_tray: bool,
}

fn main() {
    env_logger::init();

    let args = Args::parse();

    // Build icon RGBA once; reused for window decoration icon.
    let icon_rgba = make_icon_rgba();

    // Load config
    let cfg = config::load();

    // Build shared state
    let state = Arc::new(Mutex::new(monitor::AppState::default()));
    {
        if let Ok(mut s) = state.lock() {
            s.config = cfg.clone();
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
        match (ProcessLassoTray {
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
            .with_title("Process Lasso — Linux")
            // app_id must match the .desktop filename (process-lasso.desktop)
            // so KDE/KWin resolves Icon=process-lasso-linux from that file.
            .with_app_id("process-lasso")
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
        "Process Lasso",
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(app::ProcessLassoApp::new(
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
