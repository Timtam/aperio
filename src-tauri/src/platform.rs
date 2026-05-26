//! Platform-specific startup integration.
//!
//! Right now this only contains the Windows AUMID dance Aperio needs
//! because it ships as a portable .exe rather than through an MSI/NSIS
//! installer. Without the AUMID setup Windows would show
//! "Windows PowerShell" (or the binary's path) as the source of every
//! toast notification — see DESIGN.md §14.3.
//!
//! What we do under Windows:
//!   1. **Register the AUMID in HKCU** — `HKCU\Software\Classes\
//!      AppUserModelId\<aumid>` with a `DisplayName` value. Windows
//!      reads this to pick the human-readable source name and icon.
//!      User-scope, no admin rights, no installer needed.
//!   2. **Pin the AUMID to the process** —
//!      `SetCurrentProcessExplicitAppUserModelID` makes every toast
//!      this process emits use that AUMID, which Windows then matches
//!      against the registry entry above.
//!
//! macOS and Linux don't need anything similar at this stage — macOS
//! uses the bundle identifier from the .app, Linux's libnotify is
//! tolerant of unregistered senders. Future iterations can grow stubs
//! for desktop-entry files etc. when needed.

/// Hard-coded AUMID. The format
/// `<CompanyName>.<ProductName>.<SubProduct>.<VersionInformation>` is
/// Microsoft's convention; we keep it short since there's no parent
/// suite. Same value goes into the registry and the process pin so the
/// two sides agree.
///
/// Gated on `cfg(windows)` because the only consumer
/// (`windows_impl::setup`) is too — leaving it unconditional makes
/// clippy on Linux / macOS flag it as dead code.
#[cfg(windows)]
pub const APP_USER_MODEL_ID: &str = "Aperio.Calendar";

/// Display name surfaced in the toast banner. Plain — Windows shows
/// this verbatim and does no further localisation, so we keep it
/// neutral. Same `cfg(windows)` reasoning as `APP_USER_MODEL_ID`.
#[cfg(windows)]
pub const APP_DISPLAY_NAME: &str = "Aperio";

/// Run platform-specific startup. Errors are logged but never
/// propagated — toast labelling is a polish item, not a hard
/// requirement to bring up the rest of the app.
pub fn setup() {
    #[cfg(windows)]
    {
        if let Err(err) = windows_impl::setup(APP_USER_MODEL_ID, APP_DISPLAY_NAME) {
            tracing::warn!(?err, "windows AUMID setup failed");
        }
    }
}

#[cfg(windows)]
mod windows_impl {
    use ::windows::core::HSTRING;
    use ::windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    pub fn setup(aumid: &str, display_name: &str) -> anyhow::Result<()> {
        register_aumid_in_registry(aumid, display_name)?;
        set_process_aumid(aumid)?;
        Ok(())
    }

    /// Write the AUMID entry to HKCU. Idempotent — create_subkey
    /// opens the key when it already exists. We only set DisplayName
    /// here; icon support arrives once Aperio has a packaged .ico.
    fn register_aumid_in_registry(aumid: &str, display_name: &str) -> anyhow::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let subkey = format!("Software\\Classes\\AppUserModelId\\{aumid}");
        let (key, _disposition) = hkcu.create_subkey(&subkey)?;
        key.set_value("DisplayName", &display_name)?;
        // ShowInSettings=1 makes the entry appear in Windows' "Notifications &
        // actions" settings — important so the user can disable Aperio
        // notifications without finding the right .lnk.
        key.set_value("ShowInSettings", &1u32)?;
        Ok(())
    }

    /// Pin the AUMID to the current process. Any toast we emit
    /// afterwards is delivered under this AUMID, which Windows then
    /// matches against the registry entry written above.
    fn set_process_aumid(aumid: &str) -> anyhow::Result<()> {
        let wide: HSTRING = aumid.into();
        // SAFETY: `wide` is a valid wide-string and lives until the
        // call returns; the API copies what it needs.
        unsafe {
            SetCurrentProcessExplicitAppUserModelID(&wide)?;
        }
        Ok(())
    }
}
