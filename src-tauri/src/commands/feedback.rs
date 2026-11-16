//! Tauri commands behind Settings → Feedback.
//!
//! Everything but the widgets is `host_core::feedback`, shared with the phone:
//! these commands only add what the desktop host knows, which is where its
//! logs live. The desktop passes no [`HostDetails`]: its version is the core's,
//! and the core reads the Windows, macOS or Linux version itself.

use host_core::feedback::{self, FeedbackDraft, FeedbackInfo, FeedbackOutcome, HostDetails};
use tauri::State;

use super::CommandResult;
use crate::logging::LogState;

/// Whether this build can send, and the system line the form shows.
#[tauri::command]
pub fn feedback_info() -> CommandResult<FeedbackInfo> {
    Ok(feedback::info(&HostDetails::default()))
}

/// The redacted log text a message would carry, for the form to show before
/// the user decides. The form hands this very text back in the draft.
#[tauri::command]
pub fn feedback_logs(log_state: State<'_, LogState>) -> CommandResult<String> {
    Ok(feedback::logs(&log_state.logs_dir))
}

/// Send one message. Every ending is an outcome the form has a sentence for,
/// so this never returns an error.
#[tauri::command]
pub async fn send_feedback(draft: FeedbackDraft) -> CommandResult<FeedbackOutcome> {
    Ok(feedback::send(&draft, &HostDetails::default()).await)
}
