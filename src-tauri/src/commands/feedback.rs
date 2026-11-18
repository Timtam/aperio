//! Tauri commands behind Settings → Feedback.
//!
//! Everything but the widgets is `host_core::feedback`, shared with the phone:
//! these commands only add what the desktop host knows, which is where its
//! logs live. The desktop passes no [`HostDetails`]: its version is the core's,
//! and the core reads the Windows, macOS or Linux version itself.

use host_core::feedback::{
    self, FeedbackDraft, FeedbackFailure, FeedbackInfo, FeedbackOutcome, HostDetails,
};
use tauri::State;

use super::{CommandError, CommandResult};
use crate::logging::LogState;

/// Whether this build can send, and the system line the form shows.
#[tauri::command]
pub fn feedback_info() -> CommandResult<FeedbackInfo> {
    Ok(feedback::info(&HostDetails::default()))
}

/// The redacted log text a message would carry, for the form to show before
/// the user decides. The form hands this very text back in the draft.
///
/// Reading, redacting and zipping up to 32 MB takes a moment: on a blocking
/// thread, never on the one a synchronous command would run on.
#[tauri::command]
pub async fn feedback_logs(log_state: State<'_, LogState>) -> CommandResult<String> {
    let dir = log_state.logs_dir.clone();
    tauri::async_runtime::spawn_blocking(move || feedback::logs(&dir))
        .await
        .map_err(|err| CommandError {
            code: "internal",
            message: err.to_string(),
        })
}

/// What the send would refuse at once, without the network: the form asks
/// before it shows its "sending" dialog. `None` when the draft can go.
#[tauri::command]
pub fn feedback_check(draft: FeedbackDraft) -> CommandResult<Option<FeedbackFailure>> {
    Ok(feedback::check(&draft))
}

/// Send one message. Every ending is an outcome the form has a sentence for,
/// so this never returns an error.
#[tauri::command]
pub async fn send_feedback(draft: FeedbackDraft) -> CommandResult<FeedbackOutcome> {
    Ok(feedback::send(&draft, &HostDetails::default()).await)
}
