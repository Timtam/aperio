//! External-link opening.
//!
//! Event / task descriptions come from untrusted sources — external
//! EWS meeting invitations in particular — so any URL we hand to the
//! operating system is validated **here, in trusted Rust**, before it
//! reaches the default browser. Only `http`, `https` and `mailto` are
//! allowed; everything else (`file:`, custom app schemes, UNC paths,
//! …) is refused. The frontend does its own filtering for UX, but
//! this command is the security boundary that actually matters: it's
//! invokable with any string, so it can't trust its caller.

use super::{CommandError, CommandResult};

/// Maximum URL length we'll hand off. Real links sit well under this;
/// a multi-kilobyte "url" is a sign of garbage or an attack, so we
/// reject it rather than pass it to the shell.
const MAX_URL_LEN: usize = 2048;

fn invalid(msg: impl Into<String>) -> CommandError {
    CommandError {
        code: "invalid_input",
        message: msg.into(),
    }
}

/// Validate that `url` is a safe `http` / `https` / `mailto` link.
/// Returns the trimmed URL on success. Pulled out from the command so
/// it can be unit-tested without touching the OS.
fn validate_external_url(url: &str) -> Result<&str, CommandError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(invalid("empty url"));
    }
    if url.len() > MAX_URL_LEN {
        return Err(invalid("url too long"));
    }
    // No control characters or whitespace anywhere — a real URL has
    // none, and they're a classic smuggling trick (embedded CR/LF, or
    // a space that hides a second scheme from a naive parser).
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(invalid("url contains control or whitespace characters"));
    }
    // Scheme = everything before the first ':' , compared case-insensitively.
    let scheme = match url.split_once(':') {
        Some((s, _)) => s.to_ascii_lowercase(),
        None => return Err(invalid("url has no scheme")),
    };
    match scheme.as_str() {
        "http" | "https" => {
            // `scheme` is ASCII, so its byte length matches the
            // original's scheme span — slice right after it.
            let after = &url[scheme.len()..];
            if !after.starts_with("://") {
                return Err(invalid("http(s) url must use '://'"));
            }
            if after.len() <= "://".len() {
                return Err(invalid("http(s) url has no host"));
            }
        }
        "mailto" => {
            if url.len() <= "mailto:".len() {
                return Err(invalid("mailto: has no address"));
            }
        }
        other => {
            return Err(invalid(format!(
                "scheme '{other}' is not allowed (only http, https, mailto)"
            )));
        }
    }
    Ok(url)
}

/// Open a validated external URL in the OS default handler (browser /
/// mail client). Never navigates the app's own webview. Refuses any
/// scheme outside `http` / `https` / `mailto`.
#[tauri::command]
pub fn open_external_url(url: String) -> CommandResult<()> {
    let validated = validate_external_url(&url)?;
    opener::open(validated).map_err(|e| CommandError {
        code: "internal",
        message: format!("could not open url: {e}"),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_http_https_mailto() {
        for ok in [
            "http://example.com",
            "https://example.com/path?q=1#frag",
            "https://example.com",
            "HTTPS://Example.com",
            "mailto:user@example.com",
            "mailto:user@example.com?subject=Hi",
        ] {
            assert!(validate_external_url(ok).is_ok(), "should accept {ok}");
        }
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(
            validate_external_url("  https://example.com  ").unwrap(),
            "https://example.com",
        );
    }

    #[test]
    fn rejects_disallowed_schemes() {
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>",
            "ftp://example.com",
            "smb://server/share",
            "vbscript:msgbox",
            "custom-app://do-something",
            "\\\\server\\share",
        ] {
            let err = validate_external_url(bad).unwrap_err();
            assert_eq!(err.code, "invalid_input", "should reject {bad}");
        }
    }

    #[test]
    fn rejects_control_and_whitespace_smuggling() {
        for bad in [
            "https://example.com\nfile:///etc/passwd",
            "https://exa mple.com",
            "https://example.com\r\nHost: evil",
            "http://example.com\u{0000}",
        ] {
            assert!(validate_external_url(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_empty_and_hostless() {
        for bad in ["", "   ", "http://", "https://", "mailto:", "noscheme"] {
            assert!(validate_external_url(bad).is_err(), "should reject {bad:?}");
        }
    }
}
