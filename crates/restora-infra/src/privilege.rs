//! Checks whether the current process has the privilege level raw disk
//! access requires, and produces a clear, actionable message when it
//! doesn't — rather than letting the OS's own permission-denied error
//! (often cryptic, especially on Windows) be the first thing a user
//! sees.
//!
//! This is a check, not an elevator: nothing here re-launches the
//! process with elevated rights. Actually doing that — relaunching via
//! `sudo`/`pkexec` on Unix, or via a UAC prompt through `ShellExecuteW`'s
//! `"runas"` verb on Windows — is a legitimate next step, but it changes
//! how the whole application starts up (a real UI needs to decide
//! whether to prompt-and-relaunch immediately at startup or lazily when
//! the person actually picks a physical device), which is a product
//! decision for the Tauri shell, not something `restora-infra` should
//! decide unilaterally. What's implemented here is the part that's
//! genuinely OS-boundary logic: "am I allowed to do this," accurately
//! per platform.

/// What the check found, plus a ready-to-display explanation for the
/// common case ("no, and here's what to do about it").
#[derive(Debug, Clone)]
pub struct PrivilegeStatus {
    pub is_elevated: bool,
    /// A human-readable next step, populated when `is_elevated` is
    /// false. Phrased for whoever's actually running this — a WSL/Linux
    /// user sees a `sudo` instruction, a Windows user sees a "Run as
    /// Administrator" instruction — not a generic "access denied."
    pub hint: Option<String>,
}

#[cfg(unix)]
pub fn check_privilege() -> PrivilegeStatus {
    // On Unix, raw block device access is gated by the device file's own
    // permissions — in practice this means "are you root" (uid 0) for
    // any device you don't already have explicit group access to (e.g.
    // via the `disk` group on some distros). Checking the effective UID
    // is the honest, portable baseline; a more elaborate version could
    // additionally check group membership against the specific device's
    // owning group, but that varies enough across distros that "are you
    // root" is the reliable floor.
    let is_elevated = nix::unistd::geteuid().is_root();
    PrivilegeStatus {
        is_elevated,
        hint: if is_elevated {
            None
        } else {
            Some(
                "Raw disk access requires root. Re-run this command with `sudo`, \
                 e.g. `sudo restora-cli scan /dev/sdb`."
                    .to_string(),
            )
        },
    }
}

#[cfg(windows)]
pub fn check_privilege() -> PrivilegeStatus {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: standard Win32 token-inspection sequence — open the
    // current process's own access token (read-only query rights),
    // query its elevation state, close the handle. Every call's return
    // value is checked; on any failure we conservatively report
    // "not elevated" rather than assume success.
    let is_elevated = unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return PrivilegeStatus {
                is_elevated: false,
                hint: Some(windows_hint()),
            };
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned_len,
        )
        .is_ok();

        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    };

    PrivilegeStatus {
        is_elevated,
        hint: if is_elevated { None } else { Some(windows_hint()) },
    }
}

#[cfg(windows)]
fn windows_hint() -> String {
    "Raw disk access requires Administrator privileges. Right-click the app \
     (or your terminal) and choose \"Run as Administrator\", then try again."
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_hint_whenever_not_elevated() {
        let status = check_privilege();
        // Whatever the actual result (this sandbox runs as root, so
        // is_elevated should be true here — but the invariant this test
        // checks holds either way): a hint is present if and only if
        // not elevated.
        assert_eq!(status.hint.is_none(), status.is_elevated);
    }
}
