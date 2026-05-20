//! Authorization headers shared by every CalDAV request.
//!
//! Both Basic (RFC 7617) and Bearer (RFC 6750) flow through here so
//! the rest of the adapter doesn't have to care which one the user
//! configured. Bearer is mainly there for future OAuth-style
//! servers; today it is unused but harmless to keep wired up.

use base64::Engine;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

use crate::config::{AuthKind, Credentials};
use crate::error::{CaldavError, CaldavResult};

/// Build a one-entry `HeaderMap` with the Authorization header for
/// the supplied credentials. Returns a fresh map each call so
/// callers can extend it with content-type / depth without
/// mutating shared state.
pub fn auth_header(credentials: &Credentials) -> CaldavResult<HeaderMap> {
    let mut headers = HeaderMap::new();
    let value = match credentials.config.auth_kind {
        AuthKind::Basic => {
            let token = format!(
                "{}:{}",
                credentials.config.username, credentials.secret
            );
            let encoded =
                base64::engine::general_purpose::STANDARD.encode(token.as_bytes());
            HeaderValue::from_str(&format!("Basic {encoded}"))
                .map_err(|e| CaldavError::Config(e.to_string()))?
        }
        AuthKind::Bearer => HeaderValue::from_str(&format!(
            "Bearer {}",
            credentials.secret
        ))
        .map_err(|e| CaldavError::Config(e.to_string()))?,
    };
    headers.insert(AUTHORIZATION, value);
    Ok(headers)
}
