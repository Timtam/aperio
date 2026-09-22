//! Why a read was refused, in a word both surfaces can translate.
//!
//! The reading twin of [`crate::WriteRefusal`], for the same reason: the
//! refusal crosses an adapter error, the host's error type, a Tauri command or
//! the phone's native module, and every one of them carries only text. So the
//! message starts with a token from this enum, and the surfaces look the
//! sentence up in their own locale files:
//!
//! ```text
//! token-refused: users search (projects)
//! ```
//!
//! The token is the rule; the detail behind the colon names what the provider
//! wanted, in the provider's own words, so the sentence can say which
//! permission to grant without the surface knowing anything about the provider.

use serde::{Deserialize, Serialize};

/// The reason a read did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ReadRefusal {
    /// The provider refused the account's token for this read. Either the
    /// token lacks the permission the read needs, or it is no longer valid,
    /// and the provider does not say which — Vikunja answers both with the
    /// same 401. The detail names the permission the read needs.
    TokenRefused,
}

impl ReadRefusal {
    /// The token that starts the message.
    pub const fn token(self) -> &'static str {
        match self {
            Self::TokenRefused => "token-refused",
        }
    }

    /// The message an adapter hands over: the token, then the detail.
    pub fn message(self, detail: &str) -> String {
        format!("{}: {detail}", self.token())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_is_the_serialized_name() {
        // The surfaces match the token they read from the generated type, so
        // the two spellings must be the same word.
        let serialized = serde_json::to_string(&ReadRefusal::TokenRefused).unwrap();
        assert_eq!(
            serialized,
            format!("\"{}\"", ReadRefusal::TokenRefused.token())
        );
        assert_eq!(
            ReadRefusal::TokenRefused.message("users search (projects)"),
            "token-refused: users search (projects)"
        );
    }
}
