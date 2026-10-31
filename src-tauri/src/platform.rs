//! Platform-specific startup integration.
//!
//! Under Windows Aperio's notifications go out under the Tauri
//! identifier (`com.aperio.app`): tauri-plugin-notification passes it
//! explicitly to `CreateToastNotifierWithId` (plugin 2.3.3
//! `desktop.rs:195-206`, tauri-winrt-notification 0.7.2 `lib.rs:711`).
//! Windows shows them only when a Start menu shortcut carries that id
//! (System.AppUserModel.ID); without one it stores every toast and shows
//! none, not as a banner, not in the notification center. A
//! `HKCU\Software\Classes\AppUserModelId` key does not help (measured on
//! Windows 11 26220, 2026-10-31). Aperio is portable and creates that
//! shortcut only when asked: see `notification_shortcut` and DESIGN.md
//! §14.3.
//!
//! What runs here, before the first window:
//!   1. **Pin the identifier to the process**
//!      (`SetCurrentProcessExplicitAppUserModelID`). It does not decide a
//!      toast's id; it groups the window on the taskbar with the Start
//!      menu shortcut. Not in a development build, whose toasts carry
//!      PowerShell's id and whose window should not join the real pin.
//!   2. **Remove the old registration** of `Aperio.Calendar`, an id Aperio
//!      registered and pinned until 2026-10 on the premise that toasts
//!      inherit the process id. They don't, and its `ShowInSettings` entry
//!      could list a second "Aperio" in Windows' notification settings.
//!
//! macOS and Linux need nothing similar: macOS uses the bundle
//! identifier, Linux's notification daemons accept any sender.

/// Whether tauri-plugin-notification sends this build's toasts under
/// PowerShell's id rather than the identifier: when the exe sits in a
/// cargo `target\debug` or `target\release` folder. A verbatim port of
/// plugin 2.3.3 `desktop.rs:195-206`, case-sensitive like it, so Aperio
/// and the plugin agree about which builds those are. Read the plugin's
/// rule again when it is updated.
#[cfg(any(windows, test))]
pub(crate) fn toasts_use_powershell_id(exe_dir: &str) -> bool {
    use std::path::MAIN_SEPARATOR as SEP;
    exe_dir.ends_with(&format!("{SEP}target{SEP}debug"))
        || exe_dir.ends_with(&format!("{SEP}target{SEP}release"))
}

/// Whether this process is a development build in the plugin's sense
/// (see [`toasts_use_powershell_id`]). The exe path is read the way the
/// plugin reads it.
#[cfg(windows)]
pub(crate) fn is_dev_build() -> bool {
    tauri::utils::platform::current_exe()
        .ok()
        .and_then(|exe| {
            exe.parent()
                .map(|dir| toasts_use_powershell_id(&dir.display().to_string()))
        })
        .unwrap_or(false)
}

/// Run platform-specific startup. `identifier` is the Tauri identifier,
/// the id the notification plugin sends toasts under. Errors are logged
/// but never propagated: none of this is needed to bring up the rest of
/// the app.
pub fn setup(identifier: &str) {
    #[cfg(windows)]
    {
        if let Err(err) = windows_impl::remove_legacy_registration() {
            tracing::warn!(?err, "removing the old Aperio.Calendar registration failed");
        }
        if !is_dev_build() {
            if let Err(err) = windows_impl::set_process_aumid(identifier) {
                tracing::warn!(?err, identifier, "pinning the app id to the process failed");
            }
        }
    }
    #[cfg(not(windows))]
    let _ = identifier;
}

#[cfg(windows)]
mod windows_impl {
    use ::windows::core::HSTRING;
    use ::windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    /// The key Aperio wrote for `Aperio.Calendar` until 2026-10.
    const LEGACY_KEY: &str = r"Software\Classes\AppUserModelId\Aperio.Calendar";

    /// Delete the old registration. Absent is the normal case.
    pub fn remove_legacy_registration() -> std::io::Result<()> {
        match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(LEGACY_KEY) {
            Ok(()) => {
                tracing::info!("removed the old Aperio.Calendar registration");
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    }

    /// Pin `aumid` to the current process, for taskbar grouping.
    pub fn set_process_aumid(aumid: &str) -> ::windows::core::Result<()> {
        let wide: HSTRING = aumid.into();
        // SAFETY: `wide` is a valid wide-string and lives until the
        // call returns; the API copies what it needs.
        unsafe { SetCurrentProcessExplicitAppUserModelID(&wide) }
    }
}

#[cfg(test)]
mod tests {
    use super::toasts_use_powershell_id;
    use std::path::MAIN_SEPARATOR_STR;

    fn dir(parts: &[&str]) -> String {
        parts.join(MAIN_SEPARATOR_STR)
    }

    #[test]
    fn only_a_cargo_debug_or_release_folder_counts_as_a_development_build() {
        assert!(toasts_use_powershell_id(&dir(&[
            "C:", "src", "target", "debug"
        ])));
        assert!(toasts_use_powershell_id(&dir(&[
            "C:", "src", "target", "release"
        ])));
        // A test binary: one level deeper.
        assert!(!toasts_use_powershell_id(&dir(&[
            "C:", "src", "target", "debug", "deps"
        ])));
        // Case-sensitive, like the plugin.
        assert!(!toasts_use_powershell_id(&dir(&[
            "C:", "src", "Target", "Debug"
        ])));
        // A cross-compiled build folder is a real copy to the plugin.
        assert!(!toasts_use_powershell_id(&dir(&[
            "C:",
            "src",
            "target",
            "x86_64-pc-windows-msvc",
            "release",
        ])));
        // The portable folder.
        assert!(!toasts_use_powershell_id(&dir(&[
            "D:",
            "aperio-windows-x86_64"
        ])));
    }
}
