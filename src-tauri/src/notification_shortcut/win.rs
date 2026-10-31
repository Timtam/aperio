//! The Windows side of the Start menu shortcut: reading and writing a `.lnk`
//! that carries System.AppUserModel.ID, through the shell's own COM objects
//! (IShellLinkW, IPersistFile, IPropertyStore), the way Tauri's NSIS
//! installer writes its shortcut. Never called on the main thread: the
//! window's event loop initialises OLE there.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::core::{Interface, Result, HSTRING};
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantChangeType, PROPVARIANT, PVCHF_DEFAULT,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, STGM_READ,
    STGM_READWRITE,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, SHGetKnownFolderPath, ShellLink, KF_FLAG_CREATE,
    KF_FLAG_DONT_VERIFY,
};

use super::{classify, internal, LinkInfo, NotificationShortcut};
use crate::commands::{CommandError, CommandResult};

/// COM for the calling thread, for one operation, undone when dropped.
/// Created before the first COM object, so every object is released
/// before `CoUninitialize`.
struct Apartment {
    owned: bool,
}

impl Apartment {
    fn enter() -> Result<Self> {
        // SAFETY: plain FFI; a successful call is balanced in Drop.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        // Already a multi-threaded apartment: the shell link works in
        // either, and this call must not be balanced.
        if hr == RPC_E_CHANGED_MODE {
            return Ok(Self { owned: false });
        }
        // S_OK and S_FALSE (already initialised) both need balancing.
        hr.ok()?;
        Ok(Self { owned: true })
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.owned {
            // SAFETY: balances the successful CoInitializeEx in `enter`.
            unsafe { CoUninitialize() };
        }
    }
}

/// `<the user's Start menu Programs folder>\{product_name}.lnk`. With
/// `create_folder` the folder is created if missing (with the attributes
/// Windows gives it); without, the path is returned even when it is
/// missing, and the link then reads as absent.
pub fn link_path(product_name: &str, create_folder: bool) -> Result<PathBuf> {
    let flags = if create_folder {
        KF_FLAG_CREATE
    } else {
        KF_FLAG_DONT_VERIFY
    };
    // SAFETY: the returned buffer is ours, read once and freed here.
    let dir = unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_Programs, flags, None)?;
        let dir = OsString::from_wide(raw.as_wide());
        CoTaskMemFree(Some(raw.0 as *const _));
        dir
    };
    Ok(PathBuf::from(dir).join(format!("{product_name}.lnk")))
}

/// The shortcut at `link`, or `None` when there is none (one stat, no COM).
pub fn read_link(link: &Path) -> Result<Option<LinkInfo>> {
    if !link.exists() {
        return Ok(None);
    }
    let _com = Apartment::enter()?;
    // SAFETY: COM calls on an initialised apartment; every buffer outlives
    // the call that fills it.
    unsafe {
        let shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let loaded = shell
            .cast::<IPersistFile>()?
            .Load(&HSTRING::from(link), STGM_READ);
        if loaded.is_err() {
            // Not a shortcut the shell can read (cut off by an interrupted
            // save, another file of that name): not ours, so foreign. Setting
            // up replaces it; nothing else touches it. (An empty file the
            // shell reads as an empty link, which is foreign too.)
            return Ok(Some(LinkInfo {
                target: PathBuf::new(),
                aumid: None,
            }));
        }
        // Not MAX_PATH: a cut-off path would read as another exe and be
        // rewritten at every start.
        let mut buf = vec![0u16; 32_768];
        shell.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let target = PathBuf::from(OsString::from_wide(&buf[..len]));
        // Asked for after Load: before it, the store is the empty link's.
        let store: IPropertyStore = shell.cast()?;
        let value = store.GetValue(&PKEY_AppUserModel_ID)?;
        let aumid = (!value.is_empty()).then(|| value.to_string());
        Ok(Some(LinkInfo { target, aumid }))
    }
}

/// Point the shortcut at `link` at `exe` and give it `aumid`. The one place
/// a shortcut is written. With `keep_existing` (a shortcut of ours) the
/// existing one is opened rather than replaced, so what the user set on it
/// (a shortcut key, how it opens) stays; one the shell cannot read is
/// written afresh. Without it (a foreign shortcut being replaced) nothing
/// of the old one carries over: not its arguments, not its icon.
pub fn write_link(link: &Path, exe: &Path, aumid: &str, keep_existing: bool) -> Result<()> {
    let _com = Apartment::enter()?;
    let path = HSTRING::from(link);
    // SAFETY: COM calls on an initialised apartment; the strings and the
    // PROPVARIANT outlive the calls that read them.
    unsafe {
        let mut shell: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let mut file: IPersistFile = shell.cast()?;
        if keep_existing && link.exists() && file.Load(&path, STGM_READWRITE).is_err() {
            shell = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            file = shell.cast()?;
        }
        shell.SetPath(&HSTRING::from(exe))?;
        if let Some(dir) = exe.parent() {
            shell.SetWorkingDirectory(&HSTRING::from(dir))?;
        }
        let store: IPropertyStore = shell.cast()?;
        store.SetValue(&PKEY_AppUserModel_ID, &lpwstr(aumid)?)?;
        store.Commit()?;
        file.Save(&path, true)?;
    }
    Ok(())
}

/// `value` as a VT_LPWSTR, the type the shell reads System.AppUserModel.ID
/// as. `PROPVARIANT::from(&str)` is a VT_BSTR.
fn lpwstr(value: &str) -> Result<PROPVARIANT> {
    let source = PROPVARIANT::from(value);
    let mut out = PROPVARIANT::default();
    // SAFETY: both point at live PROPVARIANTs; `out` owns the result and
    // clears it when dropped.
    unsafe { PropVariantChangeType(&mut out, &source, PVCHF_DEFAULT, VT_LPWSTR)? };
    Ok(out)
}

/// Where the shortcut stands.
pub fn status_at(link: &Path, exe: &Path, aumid: &str, dev: bool) -> Result<NotificationShortcut> {
    if dev {
        return Ok(NotificationShortcut::DevBuild);
    }
    Ok(classify(read_link(link)?.as_ref(), exe, aumid, dev))
}

fn dev_build_refusal() -> CommandError {
    CommandError {
        code: "unsupported",
        message: "a development build's notifications show as Windows PowerShell; \
                  there is nothing to set up"
            .into(),
    }
}

/// Set up: create the shortcut, or point ours or replace a foreign one.
/// Read back afterwards; anything but `SetUp` is an error.
pub fn create_at(
    link: &Path,
    exe: &Path,
    aumid: &str,
    dev: bool,
) -> CommandResult<NotificationShortcut> {
    if dev {
        return Err(dev_build_refusal());
    }
    if let Some(dir) = link.parent() {
        std::fs::create_dir_all(dir).map_err(internal)?;
    }
    let ours = matches!(
        status_at(link, exe, aumid, dev).map_err(internal)?,
        NotificationShortcut::SetUp | NotificationShortcut::PointsElsewhere { .. }
    );
    write_link(link, exe, aumid, ours).map_err(internal)?;
    let status = status_at(link, exe, aumid, dev).map_err(internal)?;
    if status != NotificationShortcut::SetUp {
        return Err(internal(format!(
            "the shortcut was written but reads back as {status:?}"
        )));
    }
    Ok(status)
}

/// Remove ours. A foreign shortcut stays (`conflict`); none is fine.
pub fn remove_at(
    link: &Path,
    exe: &Path,
    aumid: &str,
    dev: bool,
) -> CommandResult<NotificationShortcut> {
    match status_at(link, exe, aumid, dev).map_err(internal)? {
        NotificationShortcut::NotSetUp => Ok(NotificationShortcut::NotSetUp),
        NotificationShortcut::Foreign => Err(CommandError {
            code: "conflict",
            message: "this Start menu entry was not made by Aperio for notifications, \
                      so it stays"
                .into(),
        }),
        NotificationShortcut::DevBuild | NotificationShortcut::Unsupported => {
            Err(dev_build_refusal())
        }
        NotificationShortcut::SetUp | NotificationShortcut::PointsElsewhere { .. } => {
            std::fs::remove_file(link).map_err(internal)?;
            Ok(NotificationShortcut::NotSetUp)
        }
    }
}

/// At start: point ours at this exe when it points elsewhere. Creates
/// nothing, touches no foreign shortcut, and leaves everything alone in a
/// development build. A failed rewrite is logged and the status stays.
pub fn refresh_at(link: &Path, exe: &Path, aumid: &str, dev: bool) -> Result<NotificationShortcut> {
    let status = status_at(link, exe, aumid, dev)?;
    let NotificationShortcut::PointsElsewhere { target } = &status else {
        return Ok(status);
    };
    match write_link(link, exe, aumid, true) {
        Ok(()) => {
            tracing::info!(
                target: "aperio::notifications",
                from = %target,
                to = %exe.display(),
                "pointed the start menu shortcut at this copy"
            );
            status_at(link, exe, aumid, dev)
        }
        Err(err) => {
            tracing::warn!(
                target: "aperio::notifications",
                ?err,
                "could not point the start menu shortcut at this copy"
            );
            Ok(status)
        }
    }
}

#[cfg(test)]
mod tests {
    //! Round trips through real `.lnk` files in a temporary folder: in
    //! process, no Start menu, no registry, no notification.

    use super::*;

    const ID: &str = "com.aperio.app.test";

    struct Fixture {
        _dir: tempfile::TempDir,
        link: PathBuf,
        here: PathBuf,
        other: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("temp dir");
        let here = dir.path().join("here").join("aperio.exe");
        let other = dir.path().join("other").join("aperio.exe");
        for exe in [&here, &other] {
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(exe, b"").unwrap();
        }
        let link = dir.path().join("Programs").join("Aperio.lnk");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        Fixture {
            _dir: dir,
            link,
            here,
            other,
        }
    }

    /// A shortcut without an app id, the way a user makes one.
    fn write_plain_link(link: &Path, exe: &Path) {
        let _com = Apartment::enter().unwrap();
        unsafe {
            let shell: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            shell.SetPath(&HSTRING::from(exe)).unwrap();
            shell
                .cast::<IPersistFile>()
                .unwrap()
                .Save(&HSTRING::from(link), true)
                .unwrap();
        }
    }

    fn set_hotkey(link: &Path, hotkey: u16) {
        let _com = Apartment::enter().unwrap();
        unsafe {
            let shell: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            let file: IPersistFile = shell.cast().unwrap();
            file.Load(&HSTRING::from(link), STGM_READWRITE).unwrap();
            shell.SetHotkey(hotkey).unwrap();
            file.Save(&HSTRING::from(link), true).unwrap();
        }
    }

    fn hotkey(link: &Path) -> u16 {
        let _com = Apartment::enter().unwrap();
        unsafe {
            let shell: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            shell
                .cast::<IPersistFile>()
                .unwrap()
                .Load(&HSTRING::from(link), STGM_READ)
                .unwrap();
            shell.GetHotkey().unwrap()
        }
    }

    #[test]
    fn the_app_id_is_written_as_the_string_type_the_shell_reads() {
        assert_eq!(lpwstr("x").unwrap().vt(), VT_LPWSTR);
    }

    #[test]
    fn a_written_shortcut_reads_back_with_its_target_and_app_id() {
        let f = fixture();
        write_link(&f.link, &f.here, ID, true).unwrap();
        let info = read_link(&f.link).unwrap().expect("a shortcut");
        assert!(super::super::same_exe(&info.target, &f.here), "{info:?}");
        assert_eq!(info.aumid.as_deref(), Some(ID));
    }

    #[test]
    fn the_status_names_each_kind_of_shortcut() {
        let f = fixture();
        assert_eq!(
            status_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::NotSetUp
        );
        write_link(&f.link, &f.here, ID, true).unwrap();
        assert_eq!(
            status_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::SetUp
        );
        assert!(matches!(
            status_at(&f.link, &f.other, ID, false).unwrap(),
            NotificationShortcut::PointsElsewhere { .. }
        ));
        write_plain_link(&f.link, &f.here);
        assert_eq!(
            status_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::Foreign
        );
    }

    #[test]
    fn the_start_points_a_moved_shortcut_at_this_copy_and_keeps_its_shortcut_key() {
        let f = fixture();
        write_link(&f.link, &f.other, ID, true).unwrap();
        // Ctrl+Alt+A.
        set_hotkey(&f.link, 0x0641);
        assert_eq!(
            refresh_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::SetUp
        );
        assert_eq!(hotkey(&f.link), 0x0641);
    }

    #[test]
    fn the_start_creates_no_shortcut() {
        let f = fixture();
        assert_eq!(
            refresh_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::NotSetUp
        );
        assert!(!f.link.exists());
    }

    #[test]
    fn the_start_leaves_a_foreign_shortcut_as_it_is() {
        let f = fixture();
        write_plain_link(&f.link, &f.other);
        let before = std::fs::read(&f.link).unwrap();
        assert_eq!(
            refresh_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::Foreign
        );
        assert_eq!(std::fs::read(&f.link).unwrap(), before);
    }

    #[test]
    fn a_development_build_repairs_nothing_and_sets_up_nothing() {
        let f = fixture();
        write_link(&f.link, &f.other, ID, true).unwrap();
        let before = std::fs::read(&f.link).unwrap();
        assert_eq!(
            refresh_at(&f.link, &f.here, ID, true).unwrap(),
            NotificationShortcut::DevBuild
        );
        assert_eq!(std::fs::read(&f.link).unwrap(), before);
        assert_eq!(
            create_at(&f.link, &f.here, ID, true).unwrap_err().code,
            "unsupported"
        );
    }

    #[test]
    fn setting_up_replaces_a_foreign_shortcut_with_nothing_of_it_left() {
        let f = fixture();
        write_plain_link(&f.link, &f.other);
        set_hotkey(&f.link, 0x0641);
        assert_eq!(
            create_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::SetUp
        );
        assert_eq!(hotkey(&f.link), 0);
    }

    #[test]
    fn a_file_the_shell_cannot_read_is_foreign_and_setting_up_replaces_it() {
        let f = fixture();
        // Not a shell link at all (an empty file the shell reads as an
        // empty link, so it would not test this).
        std::fs::write(&f.link, b"this is not a shortcut").unwrap();
        assert_eq!(
            status_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::Foreign
        );
        assert_eq!(
            refresh_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::Foreign
        );
        assert_eq!(std::fs::read(&f.link).unwrap(), b"this is not a shortcut");
        assert_eq!(
            create_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::SetUp
        );
    }

    #[test]
    fn removing_deletes_ours_keeps_a_foreign_one_and_is_fine_with_none() {
        let f = fixture();
        write_link(&f.link, &f.here, ID, true).unwrap();
        assert_eq!(
            remove_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::NotSetUp
        );
        assert!(!f.link.exists());
        assert_eq!(
            remove_at(&f.link, &f.here, ID, false).unwrap(),
            NotificationShortcut::NotSetUp
        );
        write_plain_link(&f.link, &f.here);
        assert_eq!(
            remove_at(&f.link, &f.here, ID, false).unwrap_err().code,
            "conflict"
        );
        assert!(f.link.exists());
    }
}
