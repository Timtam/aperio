//! The world zone list's offsets for the desktop.
//!
//! The names, the search and where a stored zone stands come from the core
//! through WebAssembly (`src/wasm/coreRules.ts`). The offsets are asked of
//! the host: the list asks for them when it opens and when the series' start
//! changes, never while it renders, so an IPC round trip costs nothing that
//! matters. The module carries the zones too since stage 8 (wall_clock), from
//! the same pinned tzdata; [`tzdata_version`] lets the desktop check at
//! startup that both were built from one release (decision 104a).

use super::CommandResult;

/// Every listed zone's offset at the series' start and its standard offset as
/// of today, and the order of the list. See `cal_core::zone_offsets`.
#[tauri::command]
pub async fn zone_offsets(
    question: cal_core::ZoneOffsetsQuestion,
) -> CommandResult<cal_core::ZoneOffsets> {
    Ok(cal_core::zone_offsets(&question))
}

/// The IANA tzdata release the host's zones were built from, such as
/// "2026f". The desktop compares it with its WebAssembly module's at
/// startup and refuses to start on a difference (decision 104a).
#[tauri::command]
pub fn tzdata_version() -> String {
    cal_core::tzdata_version().to_string()
}
