# Argus-Lasso

A native Linux process manager written in Rust with an immediate-mode GUI (egui/eframe).
Inspired by Windows Process Lasso, rebuilt from scratch for Linux with KDE/Wayland first-class support — and significantly expanded in scope.

![Overview tab](assets/screenshots/Overview.png)

---

## Features

### Overview Dashboard
- System-wide CPU history graph (filled area chart)
- RAM usage bar with used/total
- Load average (1m / 5m / 15m)
- Top-10 processes by CPU in a live table

### Process Table
- Live sortable table: PID, name, CPU%, memory, nice, affinity, I/O priority, status
- Sort stability — equal-CPU% rows always ordered by PID, no flickering
- Live filter by name, PID, or full command line (`/` to focus, `✕` to clear)
- Right-click context menu: kill, force-kill, suspend/resume, set affinity, set nice, set I/O priority, add rule
- **Suspend / Resume** — SIGSTOP / SIGCONT with `⏸ Suspended` status indicator
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
- Parks non-preferred CPUs via a privileged helper to maximise L3 cache locality
- Optional per-process nice elevation for the game process
- Game Launcher: launch a command, watch for its process, auto-restore CPUs when the game exits
- Steam and Lutris library pickers
- Persistent named gaming profiles (save/load CPU configurations)

### Rules Engine
- Per-process rules: CPU affinity, nice priority, I/O class/level
- Match by exact name, substring, or regex
- Enable/disable per rule; import and export as JSON
- Rule templates (presets) for common processes (browsers, Steam, audio, video)
- Confirm dialogs before destructive actions (delete rule, load/delete profile)

### HW Monitor
- Real-time CPU, GPU, disk, and NVMe temperature/power/fan sensors
- Min / max / avg columns with persistent per-session width
- Sort by value column

### Benchmark
- Memory latency benchmark (pointer-chase across configurable array sizes)
- Memory bandwidth benchmark (sequential read throughput)
- Per-run delta column: shows improvement/regression vs. previous run
- Export results to CSV

### Log
- Scrolling event log: ProBalance throttle/restore, rule matches, gaming mode changes, startup info
- Auto-scroll toggle; save log to file

### Settings
- Default CPU affinity applied to every unmatched process
- Configurable monitor and rule-enforce intervals with quick presets (0.5s / 1s / 2s / 5s)
- Breeze Dark / Breeze Light themes
- Window opacity slider (Wayland compositor-side via `wp_alpha_modifier_v1`)
- CPU scaling governor and energy performance preference (EPP) selector
- Temperature alert threshold and cooldown
- Desktop notifications toggle (gates ProBalance, HW alerts, and kill events)
- Autostart toggle — writes XDG autostart entry (`~/.config/autostart/`) **and** systemd user service (works on GNOME, KDE, XFCE, and other desktops)

### System Integration
- System tray icon via D-Bus `StatusNotifierItem` (KDE/freedesktop, no libxdo required)
- Embedded icon pixmap fallback — tray icon works without a system icon theme entry
- `--minimized` flag to start hidden to tray
- `--no-tray` flag to disable the tray entirely
- Config auto-migrated from `~/.config/process-lasso-rs/` on first launch

### CLI
```bash
# Kill a process by PID
argus-lasso kill <pid> [--force]

# Set CPU affinity
argus-lasso set-affinity <pid> <cpu-list>   # e.g. "0-7,16-23"
```

---

## Screenshots

| Tab | Preview |
|-----|---------|
| **Overview** | ![Overview](assets/screenshots/Overview.png) |
| **Processes** | ![Processes](assets/screenshots/Processes.png) |
| **ProBalance** | ![ProBalance](assets/screenshots/ProBalance.png) |
| **Gaming Mode** | ![Gaming Mode](assets/screenshots/GamingMode.png) |
| **Rules** | ![Rules](assets/screenshots/Rules.png) |
| **HW Monitor** | ![HW Monitor](assets/screenshots/HwMonitor.png) |
| **Benchmark** | ![Benchmark](assets/screenshots/Benchmark.png) |
| **Settings** | ![Settings](assets/screenshots/Settings.png) |
| **Log** | ![Log](assets/screenshots/Log.png) |

---

## Requirements

### Runtime
| Dependency | Purpose |
|-----------|---------|
| **Wayland compositor** (KDE Plasma, GNOME + AppIndicator ext., Sway…) or X11 | Display |
| **D-Bus session bus** | System tray, desktop notifications |
| `wp_alpha_modifier_v1` compositor protocol | Window opacity (optional — falls back gracefully) |
| `kdialog` **or** `zenity` **or** `qarma` | File open/save dialogs (optional — any one suffices) |
| `sqlite3` CLI binary | Lutris game library scanning (optional) |

### Build
| Dependency | Purpose |
|-----------|---------|
| Rust ≥ 1.75 (stable) | Compiler |
| `pkg-config` | Used by wayland-sys |
| `libwayland-client` | Wayland client library |
| OpenGL (Mesa / any GL driver) | egui glow renderer |
| `imagemagick` (`magick`) | Multi-size icon install via `make install` |

**Arch / CachyOS / Manjaro:**
```bash
sudo pacman -S rust pkg-config wayland mesa imagemagick
```

**Ubuntu / Debian:**
```bash
sudo apt install cargo pkg-config libwayland-dev libgl1-mesa-dev imagemagick
```

**Fedora:**
```bash
sudo dnf install rust cargo pkg-config wayland-devel mesa-libGL-devel ImageMagick
```

---

## Building & Installing

### Quick install (user-local)
```bash
git clone https://github.com/franzjeger/process-lasso-linux-rs.git
cd process-lasso-linux-rs
make install        # build release binary, install to ~/.local/, refresh icon/desktop caches
make enable         # enable systemd user service (autostart on login)
```

### Manual build
```bash
cargo build --release
# Binary at: target/release/argus-lasso
```

### Makefile targets
| Target | Description |
|--------|-------------|
| `make build` | Build release binary |
| `make install` | Install binary, icons (all sizes), `.desktop`, and systemd service |
| `make reinstall` | Rebuild and restart running instance |
| `make uninstall` | Remove all installed files |
| `make enable` | `systemctl --user enable --now argus-lasso` |
| `make disable` | `systemctl --user disable --now argus-lasso` |

---

## Usage

```bash
# Launch normally
argus-lasso

# Start minimised to system tray
argus-lasso --minimized

# Disable tray icon
argus-lasso --no-tray

# Kill a process by PID
argus-lasso kill 1234

# Force-kill a process
argus-lasso kill 1234 --force

# Set CPU affinity
argus-lasso set-affinity 1234 "0-7,16-23"

# Verbose logging
RUST_LOG=debug argus-lasso
```

### Keyboard shortcuts (Processes tab)
| Key | Action |
|-----|--------|
| `/` | Focus the filter field |
| `F5` | Force immediate refresh |
| `Delete` | Kill (SIGTERM) selected process |
| Right-click row | Context menu (kill, suspend/resume, affinity, nice, I/O, add rule) |

---

## Configuration

Config file: `~/.config/argus-lasso/config.toml`

Created on first run with sensible defaults; written automatically when settings change.
Existing configs from `~/.config/process-lasso-rs/` are automatically migrated on first launch.

---

## Gaming Mode — Privileged Helper

Parking/unparking CPUs requires writing to `/sys/devices/system/cpu/cpuN/online`, which needs root.
Argus-Lasso ships a small helper script (`argus-lasso-sysfs`) installed via:

```
Settings → Gaming Mode tab → "Install / Update Helper (root)"
```

This is the only operation requiring elevated privileges. The main application runs entirely as a normal user.

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
| `clap` | CLI argument parsing |
| `log` + `env_logger` | Structured logging |
| `png` *(build-dep)* | Icon embedding at compile time |

---

## License

MIT — see [LICENSE](LICENSE).
