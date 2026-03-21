//! Portable file-dialog helpers.
//!
//! Tries kdialog (KDE) → zenity (GNOME/GTK) → qarma (Qt/Wayland) → None.
//! All functions spawn a subprocess so they are safe to call from any thread
//! without an async runtime.

use std::path::PathBuf;
use std::process::Command;

// ── Backend detection ─────────────────────────────────────────────────────────

fn which(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

enum Backend { Kdialog, Zenity, Qarma }

fn backend() -> Option<Backend> {
    if which("kdialog")  { return Some(Backend::Kdialog); }
    if which("zenity")   { return Some(Backend::Zenity); }
    if which("qarma")    { return Some(Backend::Qarma); }
    None
}

fn run(args: &[&str]) -> Option<String> {
    let out = Command::new(args[0]).args(&args[1..]).output().ok()?;
    if out.status.success() {
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !s.is_empty() { Some(s) } else { None }
    } else {
        None
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Open a file picker. `filter` is a glob string, e.g. `"*.json"`.
/// Returns the selected path, or None if cancelled / no dialog available.
pub fn open(filter: &str) -> Option<PathBuf> {
    let s = match backend()? {
        Backend::Kdialog => run(&["kdialog", "--getopenfilename", ".", filter]),
        Backend::Zenity  => run(&["zenity", "--file-selection", "--title=Open file"]),
        Backend::Qarma   => run(&["qarma", "--file-selection"]),
    };
    s.map(PathBuf::from)
}

/// Save-as picker. `default_name` is the pre-filled filename, `filter` is a glob.
/// Returns the selected path, or None if cancelled / no dialog available.
pub fn save(default_name: &str, filter: &str) -> Option<PathBuf> {
    let s = match backend()? {
        Backend::Kdialog => run(&["kdialog", "--getsavefilename", default_name, filter]),
        Backend::Zenity  => run(&[
            "zenity", "--file-selection", "--save",
            "--confirm-overwrite",
            &format!("--filename={default_name}"),
        ]),
        Backend::Qarma   => run(&[
            "qarma", "--file-selection", "--save",
            &format!("--filename={default_name}"),
        ]),
    };
    s.map(PathBuf::from)
}
