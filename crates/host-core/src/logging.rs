//! File-backed logging core (§ Diagnostics) — the platform-agnostic half.
//!
//! A daily-rolling, non-blocking file sink under `<data_dir>/logs/` behind a
//! *reloadable* `EnvFilter`, plus the read / export / redaction helpers that
//! back the in-app log viewer. The one platform-specific piece is installing
//! the GLOBAL `tracing` subscriber: the desktop calls `.init()` once at startup;
//! the mobile cal-ffi Host installs it once per process behind a guard. So this
//! module exposes the *builders* (filter layer + handle, file appender + guard,
//! logs-dir prep) and each platform assembles + installs its own subscriber —
//! the same orchestrator(core)/scheduler(install) discipline used elsewhere.
//!
//! Privacy: the export is exportable, so [`collect`] runs a redaction pass that
//! scrubs e-mail addresses and token-like strings as a safety net. The primary
//! guarantee remains the codebase convention of never logging secrets/PII. The
//! redaction lives HERE only — never duplicated per platform.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::{filter::EnvFilter, reload, Registry};

/// Level used when neither the persisted pref nor `RUST_LOG` apply, and the
/// fallback for any unparseable directive (a bad value must never silence
/// logging entirely).
pub const DEFAULT_LEVEL: &str = "info";

/// Rotated log files older than this are pruned on startup.
pub const MAX_LOG_AGE_DAYS: u64 = 14;

/// Base log filename; daily rotation appends `.YYYY-MM-DD`. Shared so the
/// appender naming + the `log_files` filter agree across platforms.
pub const LOG_FILE_PREFIX: &str = "aperio.log";

/// Crash file name (`aperio.log.crash`). Shares the `aperio.log` prefix so
/// [`log_files`] picks it up — a crash therefore rides the normal log export
/// the user sends, no special-casing needed.
pub const CRASH_FILE: &str = "aperio.log.crash";

/// Device-local `user_prefs` key holding the chosen log level (`error`…`trace`).
/// NOT on the sync whitelist — one device's debug session shouldn't crank up
/// logging on another. Shared so the desktop command + the mobile facade read
/// and write the exact same key (a divergent key would break the restart seed).
pub const PREF_LOG_LEVEL: &str = "logging.level";

/// The reloadable-filter handle type, returned by [`build_reloadable_filter`]
/// and consumed by [`reload_filter`]. Each platform stores it in its own
/// long-lived state (desktop `LogState`, mobile a process-global).
pub type ReloadHandle = reload::Handle<EnvFilter, Registry>;

// ── Subscriber builders (the platform assembles + installs from these) ────────

/// Create `<data_dir>/logs`, prune files older than [`MAX_LOG_AGE_DAYS`], and
/// return the logs dir.
pub fn prepare_logs_dir(data_dir: &Path) -> PathBuf {
    let logs_dir = data_dir.join("logs");
    let _ = fs::create_dir_all(&logs_dir);
    prune_old_logs(&logs_dir, MAX_LOG_AGE_DAYS);
    logs_dir
}

/// The startup filter directive: `RUST_LOG` (dev/CI) or [`DEFAULT_LEVEL`].
pub fn initial_directive() -> String {
    std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LEVEL.to_string())
}

/// Build the reloadable `EnvFilter` layer + its handle from an initial directive
/// (falls back to [`DEFAULT_LEVEL`] on a bad directive). The caller `.with()`s
/// the layer onto a `tracing_subscriber::registry()` and keeps the handle for
/// live verbosity changes via [`reload_filter`].
pub fn build_reloadable_filter(
    initial: &str,
) -> (reload::Layer<EnvFilter, Registry>, ReloadHandle) {
    let filter = EnvFilter::try_new(initial).unwrap_or_else(|_| EnvFilter::new(DEFAULT_LEVEL));
    reload::Layer::new(filter)
}

/// The non-blocking, daily-rolling file writer under `logs_dir` + its
/// `WorkerGuard`. The caller MUST keep the guard alive for the process lifetime
/// (dropping it stops the background flush thread).
pub fn build_file_appender(logs_dir: &Path) -> (NonBlocking, WorkerGuard) {
    let appender = tracing_appender::rolling::daily(logs_dir, LOG_FILE_PREFIX);
    tracing_appender::non_blocking(appender)
}

/// Swap the active filter directive on a live reload handle (e.g. `"info"`,
/// `"debug"`). An invalid directive falls back to [`DEFAULT_LEVEL`] rather than
/// silencing logs.
pub fn reload_filter(handle: &ReloadHandle, directive: &str) {
    let filter = EnvFilter::try_new(directive).unwrap_or_else(|_| EnvFilter::new(DEFAULT_LEVEL));
    // Reload can only fail if the subscriber was dropped, which never happens
    // while the app is running — ignore the error.
    let _ = handle.reload(filter);
}

// ── Crash report (shared formatting + write; the hook itself is per-platform) ─

/// Format a self-contained crash record (the body the panic hook writes).
pub fn format_crash_report(
    when: &str,
    version: &str,
    thread: &str,
    location: &str,
    message: &str,
    backtrace: &str,
) -> String {
    format!(
        "\n==== CRASH {when} ====\n\
         aperio {version} | thread '{thread}'\n\
         panicked at {location}: {message}\n\
         {backtrace}\n\
         ==== end crash ====\n"
    )
}

/// Synchronously append a crash report to `<logs_dir>/aperio.log.crash`.
/// Synchronous on purpose: a `panic = "abort"` build dies the instant the hook
/// returns, so the non-blocking file sink would never flush.
pub fn write_crash_report(logs_dir: &Path, report: &str) {
    let crash_path = logs_dir.join(CRASH_FILE);
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&crash_path)
        .and_then(|mut f| f.write_all(report.as_bytes()));
}

// ── Read / export ─────────────────────────────────────────────────────────────

/// Last `max_lines` lines of the newest log file, for the in-app viewer.
pub fn recent_lines(logs_dir: &Path, max_lines: usize) -> String {
    let Some(path) = newest_log_file(logs_dir) else {
        return String::new();
    };
    let Ok(content) = fs::read_to_string(&path) else {
        return String::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

/// Concatenate every log file (oldest → newest) into one export bundle.
/// `redact` runs the PII/token scrub. `max_bytes` caps the result to its
/// most-recent N bytes (used for the clipboard / share path so a huge trace
/// bundle doesn't choke the channel); the full file export passes `None`.
pub fn collect(logs_dir: &Path, redact: bool, max_bytes: Option<usize>) -> String {
    let mut files = log_files(logs_dir);
    files.sort();
    let mut out = String::new();
    for path in files {
        if let Ok(content) = fs::read_to_string(&path) {
            out.push_str(&content);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    if let Some(max) = max_bytes {
        if out.len() > max {
            let mut cut = out.len() - max;
            while !out.is_char_boundary(cut) {
                cut += 1;
            }
            // Start at the next whole line so the cap never splits one.
            let tail = &out[cut..];
            let tail = tail.find('\n').map(|i| &tail[i + 1..]).unwrap_or(tail);
            out = format!("[… earlier log lines truncated …]\n{tail}");
        }
    }
    if redact {
        redact_text(&out)
    } else {
        out
    }
}

/// Best-effort clear: remove the rotated files. The file the appender holds
/// open (today's) is skipped — on Windows the open handle blocks removal, and
/// on Unix unlinking it would orphan in-flight writes; it ages out via the
/// retention prune.
pub fn clear(logs_dir: &Path) {
    let active = newest_log_file(logs_dir);
    for path in log_files(logs_dir) {
        if Some(&path) == active.as_ref() {
            continue;
        }
        let _ = fs::remove_file(&path);
    }
}

/// Prune log files older than `max_age_days`. Called on startup via
/// [`prepare_logs_dir`].
pub fn prune_old_logs(logs_dir: &Path, max_age_days: u64) {
    let Some(cutoff) =
        SystemTime::now().checked_sub(Duration::from_secs(max_age_days * 24 * 60 * 60))
    else {
        return;
    };
    for path in log_files(logs_dir) {
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if let Ok(modified) = meta.modified() {
            if modified < cutoff {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

fn log_files(logs_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(logs_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                // Match only our own files: exactly the prefix, or prefix
                // followed by the rotation separator `.`. A stray
                // `aperio.log_backup` the user dropped in here is NOT ours —
                // otherwise prune_old_logs could delete their file.
                n == LOG_FILE_PREFIX
                    || (n.starts_with(LOG_FILE_PREFIX)
                        && n.as_bytes().get(LOG_FILE_PREFIX.len()) == Some(&b'.'))
            })
        })
        .collect()
}

/// Newest log file by modification time (the one currently being written).
fn newest_log_file(logs_dir: &Path) -> Option<PathBuf> {
    log_files(logs_dir)
        .into_iter()
        .filter_map(|p| {
            let m = fs::metadata(&p).ok()?.modified().ok()?;
            Some((p, m))
        })
        .max_by_key(|(_, m)| *m)
        .map(|(p, _)| p)
}

/// Scrub the obvious secret/PII shapes from exportable text. Defense-in-depth
/// only — the primary guarantee is the no-secrets/no-PII logging convention.
/// This catches: credentials embedded in URLs (`scheme://user:pass@host`),
/// e-mail addresses, JWTs, and long token runs. It does NOT catch free-form
/// names / phone numbers / short provider keys with hyphens.
fn redact_text(input: &str) -> String {
    static URL_CREDS: OnceLock<Regex> = OnceLock::new();
    static EMAIL: OnceLock<Regex> = OnceLock::new();
    static JWT: OnceLock<Regex> = OnceLock::new();
    static TOKEN: OnceLock<Regex> = OnceLock::new();
    let url = URL_CREDS.get_or_init(|| {
        // scheme://user:password@host → scheme://[redacted-credentials]@host
        Regex::new(r"([A-Za-z][A-Za-z0-9+.\-]*://)[^\s:/@]+:[^\s/@]+@")
            .expect("static url-creds regex")
    });
    let email = EMAIL.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}").expect("static email regex")
    });
    let jwt = JWT.get_or_init(|| {
        // header.payload.signature — base64url with dots/hyphens, which the
        // contiguous-run token regex below would miss.
        Regex::new(r"eyJ[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+\.[A-Za-z0-9_\-]+")
            .expect("static jwt regex")
    });
    // 32+ contiguous base64/hex/underscore run (no hyphens, so UUIDs — safe,
    // useful ids — survive). Catches API keys / refresh / session tokens.
    let token = TOKEN.get_or_init(|| Regex::new(r"[A-Za-z0-9_]{32,}").expect("static token regex"));
    let s = url.replace_all(input, "${1}[redacted-credentials]@");
    let s = email.replace_all(&s, "[redacted-email]");
    let s = jwt.replace_all(&s, "[redacted-token]");
    token.replace_all(&s, "[redacted-token]").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    #[test]
    fn collect_concatenates_oldest_first() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "aperio.log.2026-06-01", "older\n");
        write(tmp.path(), "aperio.log.2026-06-02", "newer\n");
        // A non-log file must be ignored.
        write(tmp.path(), "notes.txt", "ignore me\n");
        let out = collect(tmp.path(), false, None);
        assert_eq!(out, "older\nnewer\n");
    }

    #[test]
    fn crash_file_is_included_in_collect() {
        // A panic record written to `aperio.log.crash` must ride the normal
        // log export — the prefix + '.' rule in `log_files` matches it — so a
        // user's "send logs" carries the crash without special-casing.
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "aperio.log.2026-06-01", "normal log line\n");
        write(
            tmp.path(),
            CRASH_FILE,
            "==== CRASH ====\npanicked at x: boom\n",
        );
        let out = collect(tmp.path(), false, None);
        assert!(out.contains("normal log line"));
        assert!(out.contains("panicked at x: boom"));
    }

    #[test]
    fn log_files_filter_does_not_match_lookalikes() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "aperio.log.2026-06-01", "ours\n");
        // Lookalikes a user might drop here — must NOT be treated as ours
        // (prune would otherwise delete them).
        write(tmp.path(), "aperio.log_backup", "theirs\n");
        write(tmp.path(), "aperio.logs", "theirs\n");
        let out = collect(tmp.path(), false, None);
        assert_eq!(out, "ours\n");
    }

    #[test]
    fn collect_max_bytes_tails_and_marks_truncation() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "aperio.log.2026-06-01", "l1\nl2\nl3\nl4\nl5\n");
        let out = collect(tmp.path(), false, Some(6));
        assert!(out.starts_with("[… earlier log lines truncated …]\n"));
        assert!(out.contains("l5"));
        assert!(!out.contains("l1"));
    }

    #[test]
    fn recent_lines_tails_the_newest_file() {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "aperio.log.2026-06-01", "a\nb\nc\nd\ne\n");
        let out = recent_lines(tmp.path(), 2);
        assert_eq!(out, "d\ne");
    }

    #[test]
    fn redaction_scrubs_emails_and_tokens_but_keeps_uuids() {
        let input =
            "user alice@example.com token ya29ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd id 550e8400-e29b-41d4-a716-446655440000";
        let out = redact_text(input);
        assert!(!out.contains("alice@example.com"));
        assert!(out.contains("[redacted-email]"));
        assert!(!out.contains("ya29ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcd"));
        assert!(out.contains("[redacted-token]"));
        // UUID (hyphen-broken, ≤32-char runs) survives — safe + useful.
        assert!(out.contains("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn redaction_scrubs_url_credentials_and_jwts() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fwpMeJ";
        let input = format!("GET https://alice:s3cr3t@dav.example.com/cal jwt={jwt}");
        let out = redact_text(&input);
        // URL userinfo gone, but the scheme + host stay for debugging.
        assert!(!out.contains("s3cr3t"));
        assert!(out.contains("https://[redacted-credentials]@dav.example.com"));
        // The whole JWT (dotted base64url) is redacted, not just one segment.
        assert!(!out.contains(jwt));
        assert!(out.contains("[redacted-token]"));
    }

    #[test]
    fn clear_keeps_the_active_file_and_drops_the_rest() {
        let tmp = TempDir::new().unwrap();
        let old = write(tmp.path(), "aperio.log.2026-06-01", "old\n");
        // Make the second file newer so it's detected as active.
        std::thread::sleep(std::time::Duration::from_millis(10));
        let active = write(tmp.path(), "aperio.log.2026-06-02", "active\n");
        clear(tmp.path());
        assert!(!old.exists(), "rotated file should be removed");
        assert!(active.exists(), "active (newest) file should be kept");
    }

    #[test]
    fn format_crash_report_contains_the_key_fields() {
        let r = format_crash_report(
            "2026-06-19T00:00:00Z",
            "1.2.3",
            "main",
            "src/x.rs:1:1",
            "boom",
            "<bt>",
        );
        assert!(r.contains("CRASH 2026-06-19T00:00:00Z"));
        assert!(r.contains("aperio 1.2.3 | thread 'main'"));
        assert!(r.contains("panicked at src/x.rs:1:1: boom"));
    }
}
