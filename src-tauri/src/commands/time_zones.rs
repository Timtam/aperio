//! The host's tzdata release, for the desktop's startup check.
//!
//! The world zone list (names, search, offsets and groups) comes from the
//! core through WebAssembly (`src/wasm/coreRules.ts`), which carries the
//! zones since stage 8. The host's own zones (reminders, adapters) come from
//! the same pinned tzdata; [`tzdata_version`] lets the desktop check at
//! startup that both were built from one release (decision 104a).

/// The IANA tzdata release the host's zones were built from, such as
/// "2026f". The desktop compares it with its WebAssembly module's at
/// startup and refuses to start on a difference (decision 104a).
#[tauri::command]
pub fn tzdata_version() -> String {
    cal_core::tzdata_version().to_string()
}
