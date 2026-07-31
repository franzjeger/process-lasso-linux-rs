//! cgroup v2 per-unit throttling via `systemctl --user set-property`.
//!
//! Design: docs/design-cgroup-probalance.md. The sanctioned, rootless way to
//! throttle a desktop app is to set CPUWeight/CPUQuota on the systemd *unit*
//! (app scope) the process lives in — never to migrate PIDs between cgroups
//! behind systemd's back.

use std::process::Command;

/// Resolve the throttleable systemd user unit for a PID, if any.
///
/// Returns None for processes outside the user manager's subtree (system
/// services, kernel threads), for session scopes, and for processes directly
/// under `user@.service` — throttling those would hit far more than one app.
pub fn unit_for_pid(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let path = text.lines().find_map(|l| l.strip_prefix("0::"))?.trim();
    unit_from_cgroup_path(path)
}

/// Pure classification of a cgroup v2 path (the `0::` line without prefix).
fn unit_from_cgroup_path(path: &str) -> Option<String> {
    // Must be inside a user manager subtree: .../user@<uid>.service/...
    let (_, after_user) = path.split_once("/user@")?;
    let comps: Vec<&str> = after_user.split('/').filter(|c| !c.is_empty()).collect();
    // comps[0] = "<uid>.service"; require at least one slice level between the
    // user manager and the unit (e.g. app.slice/app-firefox-1234.scope) so we
    // never throttle user@.service itself or its direct children.
    if comps.len() < 3 {
        return None;
    }
    let unit = *comps.last()?;
    if !(unit.ends_with(".scope") || unit.ends_with(".service")) {
        return None;
    }
    // The login session scope contains the whole desktop — never throttle it.
    if unit.starts_with("session-") {
        return None;
    }
    Some(unit.to_string())
}

fn systemctl_user(args: &[&str]) -> Option<std::process::Output> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .ok()
}

/// Read the unit's current CPUWeight. None = not set (kernel default 100).
pub fn read_unit_cpu_weight(unit: &str) -> Option<u64> {
    // "--" so a unit name starting with '-' can't be parsed as a flag
    let out = systemctl_user(&["show", "-p", "CPUWeight", "--value", "--", unit])?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Apply a throttle to a unit. `quota_percent` 0 = no hard cap.
/// `--runtime` scopes the change to this boot — exactly the lifetime we want.
pub fn throttle_unit(unit: &str, weight: u32, quota_percent: u32) -> bool {
    let weight_prop = format!("CPUWeight={weight}");
    let mut args = vec![
        "set-property",
        "--runtime",
        "--",
        unit,
        weight_prop.as_str(),
    ];
    let quota_prop;
    if quota_percent > 0 {
        quota_prop = format!("CPUQuota={quota_percent}%");
        args.push(quota_prop.as_str());
    }
    systemctl_user(&args)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Undo a throttle. Restores the recorded original weight, or resets the
/// property to its unset default (empty assignment) when none was recorded;
/// always clears any quota we may have set.
pub fn restore_unit(unit: &str, original_weight: Option<u64>) -> bool {
    let weight_prop = match original_weight {
        Some(w) => format!("CPUWeight={w}"),
        None => "CPUWeight=".to_string(),
    };
    systemctl_user(&[
        "set-property",
        "--runtime",
        "--",
        unit,
        weight_prop.as_str(),
        "CPUQuota=",
    ])
    .map(|o| o.status.success())
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_scope_is_throttleable() {
        assert_eq!(
            unit_from_cgroup_path(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/app-firefox-1234.scope"
            ),
            Some("app-firefox-1234.scope".into())
        );
        assert_eq!(
            unit_from_cgroup_path(
                "/user.slice/user-1000.slice/user@1000.service/app.slice/foo.service"
            ),
            Some("foo.service".into())
        );
    }

    #[test]
    fn session_scope_and_user_manager_are_rejected() {
        // Login session scope = the whole desktop
        assert_eq!(
            unit_from_cgroup_path(
                "/user.slice/user-1000.slice/user@1000.service/session.slice/session-2.scope"
            ),
            None
        );
        // Directly under user@ (no intermediate slice)
        assert_eq!(
            unit_from_cgroup_path("/user.slice/user-1000.slice/user@1000.service/init.scope"),
            None
        );
    }

    #[test]
    fn system_units_and_bare_paths_are_rejected() {
        assert_eq!(unit_from_cgroup_path("/system.slice/sshd.service"), None);
        assert_eq!(unit_from_cgroup_path("/"), None);
        // Slice (not scope/service) as leaf
        assert_eq!(
            unit_from_cgroup_path("/user.slice/user-1000.slice/user@1000.service/app.slice"),
            None
        );
    }
}
