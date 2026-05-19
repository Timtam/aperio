//! Color types for calendars, task lists, and color labels.
//!
//! See `DESIGN.md` section 6.5 (container colors) and section 8
//! (global color labels).

use serde::{Deserialize, Serialize};

/// Container color for calendars and task lists.
///
/// `source` distinguishes provider-supplied colors from user overrides —
/// relevant for writing changes back to the provider's API (section 6.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerColor {
    /// Hex color value including the leading `#`, e.g. `#4285f4`.
    pub hex: String,
    pub source: ColorSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorSource {
    /// Delivered by the provider via its API.
    Native,
    /// Overridden by the user inside the app.
    Custom,
}

impl ContainerColor {
    pub fn native(hex: impl Into<String>) -> Self {
        Self {
            hex: hex.into(),
            source: ColorSource::Native,
        }
    }

    pub fn custom(hex: impl Into<String>) -> Self {
        Self {
            hex: hex.into(),
            source: ColorSource::Custom,
        }
    }
}

/// Stable reference to a global color label (section 8).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ColorLabelId(pub String);

impl ColorLabelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
