//! In-app update check and self-install against the project's GitHub releases.
//!
//! The app replaces its own binary in place. That only works for the
//! per-user install (`~/.local/bin/argus-lasso`, what `make install` and the
//! release tarball produce); a system-wide or distro-packaged install is
//! owned by root and is left to the package manager, which this module
//! detects and reports rather than failing halfway through.
//!
//! Integrity: the release publishes a `.sha256` beside each tarball and the
//! download is checked against it. That catches a truncated or corrupted
//! transfer, not a compromised release — both files come from the same
//! place. Real tamper-resistance needs a detached signature over the tarball
//! with a key that does not live in the release; see the note in
//! `docs/design-updates.md`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

const REPO: &str = "franzjeger/process-lasso-linux-rs";
const USER_AGENT: &str = concat!("argus-lasso/", env!("CARGO_PKG_VERSION"));
/// Releases are small (single-digit MB); anything larger is not ours.
const MAX_DOWNLOAD_BYTES: u64 = 64 * 1024 * 1024;
const NET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The version this binary was built as.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A release newer than the running binary, with the assets for this arch.
#[derive(Debug, Clone)]
pub struct Update {
    pub tag: String,
    pub version: String,
    pub page_url: String,
    tarball_url: String,
    sha256_url: String,
}

/// Progress and results from the background worker.
#[derive(Debug)]
pub enum Status {
    /// Already on the newest release.
    UpToDate,
    /// A newer release exists.
    Available(Box<Update>),
    /// Install finished; the new binary is in place and needs a restart.
    Installed,
    Error(String),
}

/// Handle to a background check or install.
pub struct Job {
    rx: Receiver<Status>,
}

impl Job {
    /// Non-blocking poll. Returns `None` while the work is still running.
    pub fn poll(&self) -> Option<Status> {
        use std::sync::mpsc::TryRecvError;
        match self.rx.try_recv() {
            Ok(status) => Some(status),
            Err(TryRecvError::Empty) => None,
            // The worker died without sending. Reporting it as an error is
            // what clears `busy`; treating it as "still running" would wedge
            // the UI, since both entry points refuse to start while busy.
            Err(TryRecvError::Disconnected) => Some(Status::Error(
                "the update worker stopped unexpectedly — try again".into(),
            )),
        }
    }
}

/// Start a check for a newer release.
pub fn check() -> Job {
    spawn(|tx| {
        let result = match latest_release() {
            Ok(Some(u)) => Status::Available(Box::new(u)),
            Ok(None) => Status::UpToDate,
            Err(e) => Status::Error(e),
        };
        let _ = tx.send(result);
    })
}

/// Start downloading and installing `update` over the running binary.
pub fn install(update: Update) -> Job {
    spawn(move |tx| {
        let result = match install_blocking(&update) {
            Ok(()) => Status::Installed,
            Err(e) => Status::Error(e),
        };
        let _ = tx.send(result);
    })
}

fn spawn(f: impl FnOnce(Sender<Status>) + Send + 'static) -> Job {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || f(tx));
    Job { rx }
}

// ── Release lookup ────────────────────────────────────────────────────────

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(NET_TIMEOUT))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// Fetch the latest release and return it if it is newer than this build.
fn latest_release() -> Result<Option<Update>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not reach GitHub: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("could not read the response: {e}"))?;

    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("unexpected response from GitHub: {e}"))?;

    let tag = json["tag_name"]
        .as_str()
        .ok_or("release has no tag_name")?
        .to_string();
    let version = tag.trim_start_matches('v').to_string();

    if !is_newer(&version, current_version()) {
        return Ok(None);
    }

    // Assets are named argus-lasso-<version>-<arch>-linux.tar.gz
    let arch = std::env::consts::ARCH; // "x86_64" | "aarch64"
    let want = format!("-{arch}-linux.tar.gz");
    let assets = json["assets"].as_array().ok_or("release has no assets")?;

    let find = |suffix: &str| -> Option<String> {
        assets.iter().find_map(|a| {
            let name = a["name"].as_str()?;
            if name.ends_with(suffix) {
                a["browser_download_url"].as_str().map(str::to_owned)
            } else {
                None
            }
        })
    };

    let tarball_url =
        find(&want).ok_or_else(|| format!("release {tag} has no build for {arch}"))?;
    let sha256_url = find(&format!("{want}.sha256"))
        .ok_or_else(|| format!("release {tag} has no checksum for {arch}"))?;

    Ok(Some(Update {
        tag,
        version,
        page_url: json["html_url"].as_str().unwrap_or_default().to_string(),
        tarball_url,
        sha256_url,
    }))
}

/// Compare dotted numeric versions. Non-numeric parts sort as 0, so a
/// pre-release suffix never reads as newer than the release it precedes.
fn is_newer(candidate: &str, current: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split(['.', '-', '+'])
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(candidate), parts(current));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

// ── Install ───────────────────────────────────────────────────────────────

/// Where this process's own binary lives, resolved through any symlink.
fn install_target() -> Result<PathBuf, String> {
    let exe =
        std::env::current_exe().map_err(|e| format!("could not find the running binary: {e}"))?;
    Ok(std::fs::canonicalize(&exe).unwrap_or(exe))
}

/// True when we can replace `path` — i.e. its directory is writable by us.
fn can_write(path: &Path) -> bool {
    let dir = match path.parent() {
        Some(d) => d,
        None => return false,
    };
    // Probe by creating and removing a temp file; a read-only check on the
    // mode bits would miss ACLs, read-only mounts and root-owned dirs.
    let probe = dir.join(".argus-lasso-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

fn install_blocking(update: &Update) -> Result<(), String> {
    let target = install_target()?;
    if !can_write(&target) {
        return Err(format!(
            "{} is not writable by this user — it looks like a system-wide \
             install, so update it with your package manager instead.",
            target.display()
        ));
    }

    let tarball = fetch(&update.tarball_url)?;

    // The checksum file is "<hex>  <filename>"; take the first field.
    let sha_text = String::from_utf8(fetch(&update.sha256_url)?)
        .map_err(|_| "checksum file is not valid text".to_string())?;
    let expected = sha_text
        .split_whitespace()
        .next()
        .ok_or("checksum file is empty")?
        .to_lowercase();
    let actual = sha256_hex(&tarball);
    if actual != expected {
        return Err(format!(
            "checksum mismatch — expected {expected}, got {actual}. \
             The download was not installed."
        ));
    }

    let binary = extract_binary(&tarball)?;

    // Write beside the target and rename: rename is atomic, and it works
    // even though the old binary is running, whereas writing over it in
    // place fails with ETXTBSY.
    let dir = target.parent().ok_or("binary has no parent directory")?;
    let staged = dir.join(".argus-lasso.update");
    std::fs::write(&staged, &binary).map_err(|e| format!("could not stage the update: {e}"))?;
    set_executable(&staged)?;
    std::fs::rename(&staged, &target).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("could not replace the binary: {e}")
    })?;
    Ok(())
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let mut resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("download failed: {e}"))?;
    let mut buf = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_DOWNLOAD_BYTES)
        .read_to_end(&mut buf)
        .map_err(|e| format!("download failed: {e}"))?;
    if buf.is_empty() {
        return Err("download was empty".into());
    }
    Ok(buf)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Release tarballs unpack to a single `argus-lasso-<version>-<arch>-linux/`
/// directory, so the binary always sits exactly one level down.
///
/// `--no-wildcards-match-slash` matters: tar's `*` spans `/` by default, so a
/// bare `*/argus-lasso` would match at any depth. Pinning it to one level,
/// plus the member count check in `extract_binary`, keeps a stray second
/// match from being concatenated onto the real binary.
const BINARY_MEMBER: &str = "*/argus-lasso";
const TAR_MATCH_FLAGS: [&str; 2] = ["--wildcards", "--no-wildcards-match-slash"];

/// Run `tar` with the tarball on stdin and return its stdout.
///
/// Shells out rather than taking a tar+gzip dependency: tar is present on
/// every target system, and the archive is our own.
fn run_tar(args: &[&str], tarball: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("tar")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run tar: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("tar refused stdin")?;
    let data = tarball.to_vec();
    // Feed tar from a thread: a release tarball is larger than the pipe
    // buffer, so writing inline would deadlock against our own read.
    let writer = std::thread::spawn(move || stdin.write_all(&data));

    let out = child
        .wait_with_output()
        .map_err(|e| format!("tar failed: {e}"))?;
    let _ = writer.join();

    if !out.status.success() {
        return Err(format!(
            "could not unpack the release: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Pull the `argus-lasso` binary out of the release tarball.
fn extract_binary(tarball: &[u8]) -> Result<Vec<u8>, String> {
    // List first: `tar -xO` concatenates every match into one stream, so an
    // archive carrying two `*/argus-lasso` entries would yield a spliced file
    // that we would then mark executable. Refuse anything but a single match.
    let listing = run_tar(
        &["-tz", TAR_MATCH_FLAGS[0], TAR_MATCH_FLAGS[1], BINARY_MEMBER],
        tarball,
    )?;
    let matches = String::from_utf8_lossy(&listing)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count();
    match matches {
        0 => return Err("the release archive contained no argus-lasso binary".into()),
        1 => {}
        n => {
            return Err(format!(
                "the release archive contained {n} argus-lasso entries; refusing to install"
            ))
        }
    }

    let binary = run_tar(
        &[
            "-xzO",
            TAR_MATCH_FLAGS[0],
            TAR_MATCH_FLAGS[1],
            BINARY_MEMBER,
        ],
        tarball,
    )?;
    if binary.is_empty() {
        return Err("the release archive contained no argus-lasso binary".into());
    }
    Ok(binary)
}

fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("could not mark the update executable: {e}"))
}

/// Replace this process with the freshly installed binary.
///
/// `exec` keeps the same PID and never returns on success, so there is no
/// window where neither the old nor the new process is running.
///
/// It also means nothing unwinds: no destructors, and eframe's `on_exit`
/// never runs. The caller MUST have driven the daemon shutdown first (see
/// `monitor::shutdown_and_wait`), or parked CPUs stay offline and throttled
/// processes keep their raised nice — the originals live only in this
/// process's memory and are gone the moment the image is replaced. Callers
/// set `UpdateState::restart_requested` rather than calling this directly, so
/// that ordering lives in exactly one place.
pub fn restart() -> String {
    use std::os::unix::process::CommandExt;
    let exe = match install_target() {
        Ok(p) => p,
        Err(e) => return e,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    format!(
        "could not restart: {}",
        std::process::Command::new(exe).args(args).exec()
    )
}

#[cfg(test)]
mod tests {
    use super::{extract_binary, is_newer, sha256_hex};

    #[test]
    fn newer_patch_minor_and_major() {
        assert!(is_newer("1.0.10", "1.0.9"));
        assert!(is_newer("1.1.0", "1.0.9"));
        assert!(is_newer("2.0.0", "1.9.9"));
    }

    #[test]
    fn same_or_older_is_not_newer() {
        assert!(!is_newer("1.0.9", "1.0.9"));
        assert!(!is_newer("1.0.8", "1.0.9"));
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn missing_components_count_as_zero() {
        assert!(is_newer("1.1", "1.0.9"));
        assert!(!is_newer("1.0", "1.0.0"));
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        // Empty input, from FIPS 180-4.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// Build a release-shaped tarball and pull the binary back out, which is
    /// the step most likely to break if the archive layout ever changes.
    #[test]
    fn extract_binary_finds_the_payload() {
        let dir = std::env::temp_dir().join(format!("argus-extract-{}", std::process::id()));
        let pkg = dir.join("argus-lasso-9.9.9-x86_64-linux");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("argus-lasso"), b"#!/bin/sh\necho hi\n").unwrap();
        std::fs::write(pkg.join("README.md"), b"not the binary").unwrap();

        let out = std::process::Command::new("tar")
            .arg("-czf")
            .arg(dir.join("release.tar.gz"))
            .arg("-C")
            .arg(&dir)
            .arg("argus-lasso-9.9.9-x86_64-linux")
            .output()
            .unwrap();
        assert!(out.status.success(), "tar -czf failed");

        let tarball = std::fs::read(dir.join("release.tar.gz")).unwrap();
        let binary = extract_binary(&tarball).expect("extract");
        assert_eq!(binary, b"#!/bin/sh\necho hi\n");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extract_binary_rejects_an_archive_without_it() {
        let dir = std::env::temp_dir().join(format!("argus-empty-{}", std::process::id()));
        let pkg = dir.join("some-other-package");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("README.md"), b"nothing useful").unwrap();
        let out = std::process::Command::new("tar")
            .arg("-czf")
            .arg(dir.join("r.tar.gz"))
            .arg("-C")
            .arg(&dir)
            .arg("some-other-package")
            .output()
            .unwrap();
        assert!(out.status.success());
        let tarball = std::fs::read(dir.join("r.tar.gz")).unwrap();
        assert!(extract_binary(&tarball).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `tar -xO` concatenates every match, so two entries would splice into
    /// one file that we would mark executable. It must be refused instead.
    #[test]
    fn extract_binary_rejects_two_matching_entries() {
        let dir = std::env::temp_dir().join(format!("argus-dup-{}", std::process::id()));
        for pkg in [
            "argus-lasso-9.9.9-x86_64-linux",
            "argus-lasso-9.9.8-x86_64-linux",
        ] {
            let p = dir.join(pkg);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("argus-lasso"), b"payload").unwrap();
        }
        let out = std::process::Command::new("tar")
            .arg("-czf")
            .arg(dir.join("r.tar.gz"))
            .arg("-C")
            .arg(&dir)
            .arg("argus-lasso-9.9.9-x86_64-linux")
            .arg("argus-lasso-9.9.8-x86_64-linux")
            .output()
            .unwrap();
        assert!(out.status.success(), "tar -czf failed");

        let tarball = std::fs::read(dir.join("r.tar.gz")).unwrap();
        let err = extract_binary(&tarball).expect_err("two entries must be refused");
        assert!(err.contains('2'), "error should name the count: {err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A nested path must not match: tar's `*` spans `/` unless told not to,
    /// and a deep entry is not the release layout we publish.
    #[test]
    fn extract_binary_ignores_a_nested_match() {
        let dir = std::env::temp_dir().join(format!("argus-nested-{}", std::process::id()));
        let deep = dir.join("argus-lasso-9.9.9-x86_64-linux").join("dist");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("argus-lasso"), b"not the real one").unwrap();
        let out = std::process::Command::new("tar")
            .arg("-czf")
            .arg(dir.join("r.tar.gz"))
            .arg("-C")
            .arg(&dir)
            .arg("argus-lasso-9.9.9-x86_64-linux")
            .output()
            .unwrap();
        assert!(out.status.success(), "tar -czf failed");

        let tarball = std::fs::read(dir.join("r.tar.gz")).unwrap();
        assert!(extract_binary(&tarball).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prerelease_does_not_outrank_its_release() {
        // "1.1.0-rc1" parses as 1.1.0.0 — equal to 1.1.0 on the first three
        // components, so it must not be offered as an upgrade from 1.1.0.
        assert!(!is_newer("1.1.0-rc1", "1.1.0"));
    }
}

// ── UI-facing state ───────────────────────────────────────────────────────

/// What the update UI is doing right now. Owned by the app, polled once per
/// frame, and rendered by both the banner and the Settings card.
#[derive(Default)]
pub struct UpdateState {
    job: Option<Job>,
    /// A release newer than this build, once a check has found one.
    pub available: Option<Update>,
    /// Human-readable outcome of the last action.
    pub message: String,
    /// True while a check or install is running.
    pub busy: bool,
    /// Set once the new binary is in place and a restart is all that is left.
    pub installed: bool,
    /// The user closed the banner for this release.
    pub banner_dismissed: bool,
    /// The user asked to restart into the new binary. Handled centrally by
    /// the app at the end of the frame, which shuts the daemon down before
    /// `restart()` replaces the process image — see that function's contract.
    pub restart_requested: bool,
}

impl UpdateState {
    pub fn start_check(&mut self) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.installed = false;
        self.message = "Checking for updates…".into();
        self.job = Some(check());
    }

    pub fn start_install(&mut self) {
        let Some(update) = self.available.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        self.busy = true;
        self.message = format!("Downloading {}…", update.tag);
        self.job = Some(install(update));
    }

    /// Poll the worker. Returns true when something changed, so the caller
    /// can request a repaint.
    pub fn poll(&mut self) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        let Some(status) = job.poll() else {
            return false;
        };
        self.job = None;
        self.busy = false;
        match status {
            Status::UpToDate => {
                self.available = None;
                self.message = format!("Up to date — v{} is the latest.", current_version());
            }
            Status::Available(u) => {
                self.message = format!("v{} is available.", u.version);
                self.banner_dismissed = false;
                self.available = Some(*u);
            }
            Status::Installed => {
                self.installed = true;
                let tag = self
                    .available
                    .as_ref()
                    .map(|u| u.tag.clone())
                    .unwrap_or_default();
                self.message = format!("{tag} installed — restart to run it.");
            }
            Status::Error(e) => {
                self.message = e;
            }
        }
        true
    }
}
