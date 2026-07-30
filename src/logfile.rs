//! Persistent log file with size-based rotation.
//!
//! The in-app log is a 2000-line ring buffer; anything older is lost on
//! overflow or exit. This module mirrors every line to
//! `$XDG_DATA_HOME/argus-lasso/argus-lasso.log` (fallback
//! `~/.local/share/...`), rotating to `.log.1` at 1 MiB so disk use stays
//! bounded at ~2 MiB.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_BYTES: u64 = 1024 * 1024;

struct LogFile {
    file: File,
    written: u64,
    path: PathBuf,
}

static LOG: Mutex<Option<LogFile>> = Mutex::new(None);

fn log_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(base.join("argus-lasso").join("argus-lasso.log"))
}

fn open() -> Option<LogFile> {
    let path = log_path()?;
    fs::create_dir_all(path.parent()?).ok()?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    let written = file.metadata().map(|m| m.len()).unwrap_or(0);
    Some(LogFile {
        file,
        written,
        path,
    })
}

/// Append one line (already timestamped by the caller). Failures are silent —
/// logging must never take the app down or spam itself with errors.
pub fn append(line: &str) {
    let Ok(mut guard) = LOG.lock() else { return };
    if guard.is_none() {
        *guard = open();
    }
    let Some(lf) = guard.as_mut() else { return };

    if lf.written >= MAX_BYTES {
        // Rotate: current → .1 (replacing any previous .1), then reopen.
        let rotated = lf.path.with_extension("log.1");
        let _ = fs::rename(&lf.path, rotated);
        match open() {
            Some(new_lf) => *lf = new_lf,
            None => {
                *guard = None;
                return;
            }
        }
    }

    let lf = guard.as_mut().unwrap();
    if writeln!(lf.file, "{line}").is_ok() {
        lf.written += line.len() as u64 + 1;
    }
}
