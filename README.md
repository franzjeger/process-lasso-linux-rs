# Process Lasso — Linux

A native Linux process manager written in Rust with an immediate-mode GUI (egui/eframe).
Inspired by the Windows tool of the same name, built from scratch for Linux with KDE/Wayland first-class support.

![Processes tab](assets/screenshots/Processes.png)

---

## Features

### Process Table
- Live sortable table: PID, name, CPU%, memory, nice, affinity, I/O priority, throttle status
- Sort stability — equal-CPU% rows are always ordered by PID, no flickering
- Live filter by name, PID, or full command line (`/` key to focus, `✕` to clear)
- Right-click context menu: kill, force-kill, set affinity, set nice, set I/O priority, add rule
- `Delete` kills selected process, `F5` forces immediate refresh
- Cmdline tooltip on hover over the name column
- Per-CPU load bars with frequency readout and offline/parked indicators
- Rolling 120-sample CPU history chart

### ProBalance
- Automatically throttles high-CPU processes by raising their nice priority
- Configurable CPU threshold, consecutive-seconds trigger, nice adjustment, and restore hysteresis
- Per-process exempt list (pattern matching)
- Desktop notifications (D-Bus/zbus) when processes are throttled or restored

### Gaming Mode
- Detects asymmetric CPU topologies (Intel P/E-cores, AMD X3D preferred/non-preferred CCDs)
- Parks non-preferred CPUs via a privileged helper to maximise L3 cache locality for the active game
- Optional per-process nice elevation for the game process
- Game Launcher: launch a command, watch for its process, auto-restore CPUs when the game exits
- Steam and Lutris library pickers
- Persistent gaming profiles (save/load CPU configurations)

### Rules Engine
- Per-process rules: CPU affinity, nice priority, I/O class/level
- Match by exact name, substring, or regex
- Enable/disable per rule; import and export as JSON
- Rule templates (presets) for common processes (browsers, Steam, audio, video)

### Settings
- Default CPU affinity applied to every unmatched process
- Configurable monitor and rule-enforce intervals
- Breeze Dark / Breeze Light themes
- Window opacity slider (Wayland compositor-side via `wp_alpha_modifier_v1`)
- All sub-windows (dialogs) inherit the main window's opacity
- Autostart toggle (writes/removes `~/.config/autostart/process-lasso.desktop`)
- Config persisted to `~/.config/process-lasso-rs/config.toml`

### System Integration
- System tray icon via D-Bus `StatusNotifierItem` (KDE/freedesktop, no libxdo)
- `--minimized` flag to start hidden to tray
- Systemd user service for autostart (`dist/process-lasso.service`)
- KDE window decoration icon resolved via `xdg_toplevel` app_id → `.desktop` file

---

## Screenshots

| Tab | Preview |
|-----|---------|
| **Processes** | ![Processes](assets/screenshots/Processes.png) |
| **ProBalance** | ![ProBalance](assets/screenshots/ProBalance.png) |
| **Gaming Mode** | ![Gaming Mode](assets/screenshots/GamingMode.png) |
| **Rules** | ![Rules](assets/screenshots/Rules.png) |
| **Settings** | ![Settings](assets/screenshots/Settings.png) |
| **Log** | ![Log](assets/screenshots/Log.png) |

---

## Requirements

### Runtime
| Dependency | Purpose |
|-----------|---------|
| **Wayland compositor** (KDE Plasma, GNOME, Sway…) or X11 | Display |
| **D-Bus session bus** | System tray, desktop notifications |
| `wp_alpha_modifier_v1` compositor protocol | Window opacity (optional — falls back gracefully) |
| `sqlite3` CLI binary | Lutris game library scanning (optional) |

### Build
| Dependency | Purpose |
|-----------|---------|
| Rust ≥ 1.75 (stable) | Compiler |
| `pkg-config` | Used by wayland-sys |
| `libwayland-client` | Wayland client library |
| OpenGL (Mesa / any GL driver) | egui glow renderer |

**Arch / CachyOS / Manjaro:**
```bash
sudo pacman -S rust pkg-config wayland mesa
```

**Ubuntu / Debian:**
```bash
sudo apt install cargo pkg-config libwayland-dev libgl1-mesa-dev
```

**Fedora:**
```bash
sudo dnf install rust cargo pkg-config wayland-devel mesa-libGL-devel
```

---

## Building & Installing

### Quick install (user-local)
```bash
git clone https://github.com/franzjeger/process-lasso-linux-rs.git
cd process-lasso-linux-rs
make install        # builds release binary and installs to ~/.local/
make enable         # enable systemd user service (autostart on login)
```

### Manual build
```bash
cargo build --release
# Binary at: target/release/process-lasso
```

### Makefile targets
| Target | Description |
|--------|-------------|
| `make build` | Build release binary |
| `make install` | Install binary, icon, `.desktop`, and systemd service |
| `make uninstall` | Remove all installed files |
| `make enable` | `systemctl --user enable --now process-lasso` |
| `make disable` | `systemctl --user disable --now process-lasso` |

---

## Usage

```bash
# Launch normally
process-lasso

# Start minimised to system tray
process-lasso --minimized

# Verbose logging
RUST_LOG=debug process-lasso
```

### Keyboard shortcuts (Processes tab)
| Key | Action |
|-----|--------|
| `/` | Focus the filter field |
| `F5` | Force immediate refresh |
| `Delete` | Kill (SIGTERM) selected process |
| Right-click row | Context menu (kill, affinity, nice, I/O, add rule) |

---

## Configuration

Config file: `~/.config/process-lasso-rs/config.toml`
Rules file: `~/.config/process-lasso-rs/rules.json`

The config is created on first run with sensible defaults and is written automatically when settings change. The `[ui]` section stores the last-used theme and opacity so they persist across sessions.

---

## Gaming Mode — Privileged Helper

Parking/unparking CPUs requires writing to `/sys/devices/system/cpu/cpuN/online`, which needs root.
Process Lasso ships a small helper binary (`process-lasso-helper`) that is installed setuid via:

```
Settings → Gaming Mode tab → "Install / Update Helper (root)"
```

This is the only operation that requires elevated privileges. The main application runs entirely as a normal user.

---

## Crate dependencies

| Crate | Purpose |
|-------|---------|
| `eframe` / `egui` / `egui_extras` | Immediate-mode GUI (glow/OpenGL backend) |
| `procfs` | `/proc` filesystem parsing |
| `nix` | `sched_setaffinity`, signals, ioprio |
| `serde` + `toml` | Config serialisation |
| `serde_json` | Rules import/export |
| `regex` | Rule pattern matching |
| `uuid` | Stable rule IDs |
| `ksni` | D-Bus `StatusNotifierItem` system tray |
| `notify-rust` | Desktop notifications |
| `wayland-client` / `wayland-protocols` | `wp_alpha_modifier_v1` opacity |
| `raw-window-handle` | Wayland surface pointer extraction |
| `crossbeam-channel` | GUI ↔ daemon command channel |
| `rfd` | Native file dialogs (rules import/export) |
| `clap` | CLI argument parsing |
| `log` + `env_logger` | Structured logging |
| `png` *(build-dep)* | Icon embedding at compile time |

---

## License

MIT — see [LICENSE](LICENSE).
