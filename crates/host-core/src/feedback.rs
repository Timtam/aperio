//! The in-app feedback form: what it sends, how, and where to.
//!
//! Both hosts show the same form (Settings → Feedback, and "Send logs" in the
//! logs area, which opens it with the logs ticked) and call in here for
//! everything but the widgets: whether this build can send at all, the system
//! line, the log text, the checks on the input, and the sending.
//!
//! Submissions go to Static Forms, which mails them to the maintainer; Aperio
//! sends no mail itself. The form there is guarded by ALTCHA, a proof-of-work
//! captcha: the app fetches a challenge, finds the number whose SHA-256 with
//! the challenge's salt matches, and sends that answer along. There is nothing
//! to look at and nothing to tick, so it is the same for sighted and blind
//! users, and it needs no web view.
//!
//! What leaves the device, and only when the user presses send:
//!  - the message the user typed;
//!  - the e-mail address, if the user gave one, as the reply-to;
//!  - the system line ([`system_line`]), which the form shows;
//!  - on request, the logs: every log file, redacted, as many of the newest
//!    lines as fit zipped in [`LOGS_ZIP_BUDGET`] (at most
//!    [`LOGS_TEXT_CEILING`] of text). The form shows that very text first
//!    ([`logs`]), and the text it showed is what is sent, redacted once more on
//!    the way out.
//!
//! The key the form submits with is baked into the build
//! ([`builtin_oauth::feedback_form_key`]). A build without one reports
//! [`FeedbackInfo::available`] false, and the form says so up front.

use std::io::{Cursor, Write};
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How large the zipped logs may be: as many of the newest lines go as fit in
/// this (Toni, 2026-11-16), leaving the rest of [`REQUEST_MAX_BYTES`] for the
/// message and the framing. Logs zip to about a fifth or less, so this is some
/// 20 MB of text and often all 14 days.
pub const LOGS_ZIP_BUDGET: usize = 4_000_000;

/// The most log text the core reads at all, whatever would fit: the text is
/// held, redacted, zipped and shown in full, on phones too, so it needs a
/// bound of its own.
pub const LOGS_TEXT_CEILING: usize = 32 * 1024 * 1024;

/// What Static Forms accepts for one whole request is 4.5 MB. Checked here
/// before sending, with room for the multipart framing, so an oversized message
/// is named as such instead of arriving as a refused connection.
const REQUEST_MAX_BYTES: usize = 4_400_000;

/// Room the captcha answer takes in a body: base64 of a JSON holding two
/// 64-character hex strings, a salt and a number, a few hundred bytes. Kept
/// generous, because it only decides whether to ask for a challenge at all.
const TOKEN_ALLOWANCE: usize = 4096;

const SUBMIT_URL: &str = "https://api.staticforms.dev/submit";
const CHALLENGE_URL: &str = "https://www.staticforms.dev/api/altcha/challenge";

/// ALTCHA's own default when a challenge names no limit.
const DEFAULT_MAX_NUMBER: u64 = 1_000_000;

/// A challenge asking for more than this is not solved: a misbehaving server
/// must not be able to keep a phone's CPU busy for minutes.
const MAX_NUMBER_CEILING: u64 = 10_000_000;

const LOGS_FILE_NAME: &str = "aperio-logs.zip";
const LOGS_ENTRY_NAME: &str = "aperio-logs.txt";

/// What a host knows about itself that the core cannot read on its own.
///
/// Every field is optional. The desktop passes nothing: its version is the
/// core's, and the core reads the operating system's version itself. The phone
/// passes its store version and build number, the system version React Native
/// reports, and on Android the device model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct HostDetails {
    /// The app's own version where it is not the core's (the phone app's).
    #[serde(default)]
    pub app_version: Option<String>,
    /// The app's build number.
    #[serde(default)]
    pub build: Option<String>,
    /// The operating system's version, when the host knows it.
    #[serde(default)]
    pub os_version: Option<String>,
    /// The device model, where the platform names one.
    #[serde(default)]
    pub device: Option<String>,
}

/// What the form needs before the user types anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct FeedbackInfo {
    /// Whether this build can send at all: it carries the form's key.
    pub available: bool,
    /// What goes along with every message, shown in the form.
    pub system: String,
}

/// One message, as the form hands it over.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct FeedbackDraft {
    pub message: String,
    /// Where a reply can reach the user. Optional.
    #[serde(default)]
    pub email: Option<String>,
    /// The log text the form showed (from [`logs`]), or `None` to send none.
    #[serde(default)]
    pub logs: Option<String>,
}

/// How a send ended, for the form to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FeedbackOutcome {
    Sent,
    Failed { reason: FeedbackFailure },
}

/// Why a message was not sent. Each one is a sentence in the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FeedbackFailure {
    /// This build carries no key for the form.
    Unavailable,
    /// The message is empty or only whitespace.
    EmptyMessage,
    /// An e-mail address was given and does not look like one.
    InvalidEmail,
    /// The whole request would be over the service's limit.
    TooLarge,
    /// The service could not be reached.
    Network,
    /// The service is taking no more messages right now (rate limit, shared by
    /// every Aperio user).
    Busy,
    /// The service answered with an error of its own.
    Service,
    /// The service refused the message (key, captcha, form switched off).
    Rejected,
}

impl From<Result<(), FeedbackFailure>> for FeedbackOutcome {
    fn from(result: Result<(), FeedbackFailure>) -> Self {
        match result {
            Ok(()) => FeedbackOutcome::Sent,
            Err(reason) => FeedbackOutcome::Failed { reason },
        }
    }
}

/// Whether this build can send, and the line that goes with every message.
pub fn info(details: &HostDetails) -> FeedbackInfo {
    FeedbackInfo {
        available: builtin_oauth::feedback_form_key().is_some(),
        system: system_line(details),
    }
}

/// The log text a message would carry: every log file, redacted, then as many
/// of the newest whole lines as fit zipped in [`LOGS_ZIP_BUDGET`]. The form
/// shows exactly this before sending, and sending leaves it as it is:
/// redacting and fitting it again change nothing ([`prepare_logs`]).
///
/// Reads, redacts and zips up to [`LOGS_TEXT_CEILING`] of text: seconds on a
/// phone, so a host calls it off its interface thread.
pub fn logs(logs_dir: &Path) -> String {
    logs_within(logs_dir, LOGS_TEXT_CEILING, LOGS_ZIP_BUDGET)
}

fn logs_within(logs_dir: &Path, ceiling: usize, budget: usize) -> String {
    // `collect` caps before it redacts, and redacting can lengthen a line
    // (`a@b.de` becomes `[redacted-email]`), so the cap comes again after it.
    let text = capped(
        &crate::logging::collect(logs_dir, true, Some(ceiling)),
        ceiling,
    );
    match fit_zipped(text, budget) {
        Ok((text, _)) => text,
        Err(err) => {
            tracing::warn!(error = %err, "feedback: zipping the logs failed");
            String::new()
        }
    }
}

/// The newest whole lines of `text` whose zip fits in `budget` bytes, and that
/// zip. Text whose zip already fits comes back unchanged, which is what keeps
/// a send from changing the text [`logs`] showed. Each round shrinks the text
/// by the ratio the last zip measured, a little more to be safe; after a few
/// rounds the last zip is returned whatever its size, and the request's own
/// size check names it.
fn fit_zipped(text: String, budget: usize) -> zip::result::ZipResult<(String, Vec<u8>)> {
    let mut text = text;
    let mut zip = zip_logs(&text)?;
    for _ in 0..8 {
        if zip.len() <= budget || text.is_empty() {
            break;
        }
        let ratio = budget as f64 / zip.len() as f64;
        let target = ((text.len() as f64) * ratio * 0.97) as usize;
        text = capped(&text, target.min(text.len().saturating_sub(1)));
        zip = zip_logs(&text)?;
    }
    Ok((text, zip))
}

/// The line that stands where older lines were cut, the same words
/// `logging::collect` uses.
const TRUNCATED: &str = "[… earlier log lines truncated …]\n";

/// `text` when it fits in `max` bytes; otherwise [`TRUNCATED`] and the newest
/// whole lines that fit with it. A single line longer than that is cut at a
/// character boundary.
fn capped(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let room = max.saturating_sub(TRUNCATED.len());
    let mut cut = text.len() - room;
    while !text.is_char_boundary(cut) {
        cut += 1;
    }
    let tail = &text[cut..];
    let at_line_start = text.as_bytes()[cut - 1] == b'\n';
    let tail = match tail.find('\n') {
        Some(i) if !at_line_start && i + 1 < tail.len() => &tail[i + 1..],
        _ => tail,
    };
    format!("{TRUNCATED}{tail}")
}

/// `Aperio 0.2.0 (app 1.0.0, build 70) · iOS 18.1 · aarch64`, with whatever
/// the host knows. Language-neutral on purpose: the reader is whoever answers
/// the feedback, whatever language the sender's app is in.
pub fn system_line(details: &HostDetails) -> String {
    let given = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    let mut app = format!("Aperio {}", env!("CARGO_PKG_VERSION"));
    let extras: Vec<String> = [
        given(&details.app_version).map(|v| format!("app {v}")),
        given(&details.build).map(|b| format!("build {b}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !extras.is_empty() {
        app.push_str(&format!(" ({})", extras.join(", ")));
    }

    // A phone passes its system version; the desktop reads a whole label
    // itself, name included, because "Windows 11 Pro" is not "Windows" plus
    // a version number.
    let os = os_name(std::env::consts::OS);
    let os = match given(&details.os_version) {
        Some(version) => format!("{os} {version}"),
        None => own_os_label().unwrap_or_else(|| os.to_string()),
    };

    let mut parts = vec![app, os, std::env::consts::ARCH.to_string()];
    if let Some(device) = given(&details.device) {
        parts.push(device);
    }
    parts.join(" · ")
}

fn os_name(os: &str) -> &str {
    match os {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        "ios" => "iOS",
        "android" => "Android",
        other => other,
    }
}

/// The operating system as the desktop names it itself, e.g. `Windows 11 Pro
/// 25H2 (build 26220.9836)`, `macOS 15.6.1 (build 24G90)`, `Ubuntu 24.04.1
/// LTS`. `None` where the core cannot read it; on the phone the host passes
/// the version instead ([`HostDetails::os_version`]).
#[cfg(windows)]
fn own_os_label() -> Option<String> {
    Some(windows_label(&windows_facts()))
}

#[cfg(target_os = "macos")]
fn own_os_label() -> Option<String> {
    let out = std::process::Command::new("sw_vers").output().ok()?;
    macos_label(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(target_os = "linux")]
fn own_os_label() -> Option<String> {
    // os-release(5): /etc first, /usr/lib only when /etc has none.
    let text = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .ok()?;
    os_release_pretty_name(&text)
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn own_os_label() -> Option<String> {
    None
}

/// What the Windows label is made of. Plain data, so every platform tests the
/// rule ([`windows_label`]) and only the reading is Windows-only.
#[derive(Debug, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
struct WindowsFacts {
    /// `ProductName` from the registry, e.g. "Windows 10 Pro" (also on 11).
    product_name: Option<String>,
    /// `DisplayVersion`, e.g. "25H2"; exists since 20H2.
    display_version: Option<String>,
    /// `ReleaseId`, e.g. "1909"; frozen at "2009" since 20H2.
    release_id: Option<String>,
    build: u32,
    /// The update build revision, the number after the build's dot.
    ubr: u32,
    is_server: bool,
    /// A compatibility layer fakes the version the process sees.
    compat_mode: bool,
}

/// `Windows 11 Pro 25H2 (build 26220.9836)`.
///
/// Windows 11 still reports kernel version 10.0, and Microsoft keeps "Windows
/// 10" in `ProductName` for compatibility. What tells them apart is the build:
/// a workstation at 22000 or later is Windows 11. A server is never renamed:
/// Windows Server 2025 shares its build, 26100, with Windows 11 24H2.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_label(facts: &WindowsFacts) -> String {
    let clean = |v: &Option<String>| -> Option<String> {
        v.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let mut name = clean(&facts.product_name).unwrap_or_else(|| "Windows".to_string());
    if !facts.is_server
        && facts.build >= 22000
        && (name == "Windows 10" || name.starts_with("Windows 10 "))
    {
        name = format!("Windows 11{}", &name["Windows 10".len()..]);
    }
    if let Some(release) = clean(&facts.display_version).or_else(|| clean(&facts.release_id)) {
        name.push(' ');
        name.push_str(&release);
    }
    let mut details = match (facts.build, facts.ubr) {
        (0, _) => Vec::new(),
        (build, 0) => vec![format!("build {build}")],
        (build, ubr) => vec![format!("build {build}.{ubr}")],
    };
    if facts.compat_mode {
        details.push("compatibility mode".to_string());
    }
    if details.is_empty() {
        name
    } else {
        format!("{name} ({})", details.join(", "))
    }
}

/// Read what [`windows_label`] needs: the kernel's version through
/// `RtlGetVersion`, which needs no manifest, and the rest from
/// `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion`, in its 64-bit view
/// (the 32-bit one can name another edition).
#[cfg(windows)]
fn windows_facts() -> WindowsFacts {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            KEY_READ | KEY_WOW64_64KEY,
        )
        .ok();
    let text = |name: &str| -> Option<String> { key.as_ref()?.get_value(name).ok() };
    let kernel = windows_version::OsVersion::current();
    // Aperio needs Windows 10, so a major version below 10 is a compatibility
    // layer. It fakes the kernel's version, ProductName and CurrentBuildNumber,
    // but not CurrentBuild, DisplayVersion or UBR (measured, 2026-11-16).
    let compat_mode = kernel.major < 10;
    let build = if compat_mode {
        text("CurrentBuild")
            .and_then(|b| b.trim().parse().ok())
            .unwrap_or(kernel.build)
    } else {
        kernel.build
    };
    WindowsFacts {
        product_name: if compat_mode {
            None
        } else {
            text("ProductName")
        },
        display_version: text("DisplayVersion"),
        release_id: text("ReleaseId"),
        build,
        ubr: key
            .as_ref()
            .and_then(|k| k.get_value::<u32, _>("UBR").ok())
            .unwrap_or(0),
        is_server: windows_version::is_server(),
        compat_mode,
    }
}

/// `macOS 15.6.1 (build 24G90)` from plain `sw_vers` output.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_label(sw_vers: &str) -> Option<String> {
    let field = |key: &str| {
        sw_vers.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            let v = v.trim();
            (k.trim() == key && !v.is_empty()).then_some(v)
        })
    };
    let version = field("ProductVersion")?;
    let name = field("ProductName").unwrap_or("macOS");
    Some(match field("BuildVersion") {
        Some(build) => format!("{name} {version} (build {build})"),
        None => format!("{name} {version}"),
    })
}

/// `PRETTY_NAME` from an os-release file, without its quotes.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn os_release_pretty_name(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = line.trim().strip_prefix("PRETTY_NAME=")?;
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Send one message. Never panics and never returns an error type: every way
/// it can end is a [`FeedbackOutcome`] the form has a sentence for.
pub async fn send(draft: &FeedbackDraft, details: &HostDetails) -> FeedbackOutcome {
    let Some(key) = builtin_oauth::feedback_form_key() else {
        return FeedbackOutcome::Failed {
            reason: FeedbackFailure::Unavailable,
        };
    };
    let service = Service {
        challenge_url: CHALLENGE_URL,
        submit_url: SUBMIT_URL,
        key,
    };
    service.send(draft, details).await.into()
}

/// Where a message goes. Split from [`send`] so the tests can point it at a
/// local server with a key of their own.
struct Service<'a> {
    challenge_url: &'a str,
    submit_url: &'a str,
    key: &'a str,
}

impl Service<'_> {
    async fn send(
        &self,
        draft: &FeedbackDraft,
        details: &HostDetails,
    ) -> Result<(), FeedbackFailure> {
        let message = draft.message.trim();
        if message.is_empty() {
            return Err(FeedbackFailure::EmptyMessage);
        }
        let email = match draft.email.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(email) if looks_like_email(email) => Some(email),
            Some(_) => return Err(FeedbackFailure::InvalidEmail),
        };
        let system = system_line(details);
        let attachment = match draft.logs.clone() {
            None => None,
            // Redacting and zipping up to 32 MB takes a while: off the async
            // worker, like the captcha's search.
            Some(text) => Some(
                tokio::task::spawn_blocking(move || {
                    prepare_logs(&text, LOGS_TEXT_CEILING, LOGS_ZIP_BUDGET).map(|(_, zip)| zip)
                })
                .await
                .map_err(|err| err.to_string())
                .and_then(|zipped| zipped.map_err(|err| err.to_string()))
                .map_err(|err| {
                    tracing::warn!(error = %err, "feedback: zipping the logs failed");
                    FeedbackFailure::Service
                })?,
            ),
        };

        let fields = Fields {
            subject: format!("Aperio feedback · {system}"),
            message: message.to_string(),
            reply_to: email.map(str::to_string),
            system,
        };

        // Too large is known before any challenge: each one counts against a
        // limit all Aperio users share, and solving it costs the device work.
        let (_, without_token) = self.body(&fields, "", attachment.as_deref());
        if without_token.len() + TOKEN_ALLOWANCE > REQUEST_MAX_BYTES {
            return Err(FeedbackFailure::TooLarge);
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("Aperio/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| {
                tracing::warn!(error = %err.without_url(), "feedback: no HTTP client");
                FeedbackFailure::Network
            })?;

        // A token that aged out between fetching and sending (a slow upload on
        // a phone) is the one refusal a second try can fix: one more, with a
        // fresh challenge, and no more than that.
        match self.attempt(&client, &fields, attachment.as_deref()).await {
            Err(Refusal::CaptchaExpired) => {
                tracing::info!("feedback: captcha token expired, trying once more");
                self.attempt(&client, &fields, attachment.as_deref())
                    .await
                    .map_err(Refusal::into_failure)
            }
            other => other.map_err(Refusal::into_failure),
        }?;

        tracing::info!(
            logs = attachment.as_ref().map_or(0, Vec::len),
            "feedback: sent"
        );
        Ok(())
    }

    async fn attempt(
        &self,
        client: &reqwest::Client,
        fields: &Fields,
        attachment: Option<&[u8]>,
    ) -> Result<(), Refusal> {
        let challenge = self.challenge(client).await?;
        let token = tokio::task::spawn_blocking(move || {
            let number = solve(&challenge)?;
            Some(altcha_token(&challenge, number))
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            tracing::warn!("feedback: the captcha challenge has no solution in range");
            Refusal::Failure(FeedbackFailure::Service)
        })?;

        let (content_type, body) = self.body(fields, &token, attachment);
        if body.len() > REQUEST_MAX_BYTES {
            return Err(Refusal::Failure(FeedbackFailure::TooLarge));
        }

        let response = client
            .post(self.submit_url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .header(reqwest::header::ACCEPT, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(error = %err.without_url(), "feedback: submit failed");
                Refusal::Failure(FeedbackFailure::Network)
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        submit_verdict(status, &text)
    }

    /// The request body and its content type: JSON without an attachment,
    /// multipart with one.
    fn body(&self, fields: &Fields, token: &str, attachment: Option<&[u8]>) -> (String, Vec<u8>) {
        match attachment {
            None => (
                "application/json".to_string(),
                fields.json(self.key, token).into_bytes(),
            ),
            Some(zip) => fields.multipart(self.key, token, zip),
        }
    }

    /// Fetch one ALTCHA challenge. The key travels in the query, as Static
    /// Forms documents it, so no error here may print the URL.
    async fn challenge(&self, client: &reqwest::Client) -> Result<Challenge, Refusal> {
        let response = client
            .get(self.challenge_url)
            .query(&[("apiKey", self.key)])
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|err| {
                tracing::warn!(error = %err.without_url(), "feedback: challenge request failed");
                Refusal::Failure(FeedbackFailure::Network)
            })?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        match status {
            200..=299 => serde_json::from_str::<Challenge>(&text).map_err(|err| {
                tracing::warn!(error = %err, "feedback: the captcha challenge did not parse");
                Refusal::Failure(FeedbackFailure::Service)
            }),
            429 => {
                tracing::warn!("feedback: captcha challenges are rate limited right now");
                Err(Refusal::Failure(FeedbackFailure::Busy))
            }
            500..=599 => {
                tracing::warn!(status, "feedback: the challenge endpoint failed");
                Err(Refusal::Failure(FeedbackFailure::Service))
            }
            _ => {
                tracing::warn!(status, "feedback: the challenge endpoint refused");
                Err(Refusal::Failure(FeedbackFailure::Rejected))
            }
        }
    }
}

/// How one attempt ended, before deciding on a second.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    CaptchaExpired,
    Failure(FeedbackFailure),
}

impl Refusal {
    fn into_failure(self) -> FeedbackFailure {
        match self {
            Refusal::CaptchaExpired => FeedbackFailure::Rejected,
            Refusal::Failure(failure) => failure,
        }
    }
}

/// Static Forms' answer to a submit. The status decides; the body's `code`
/// only tells an expired captcha from every other refusal.
fn submit_verdict(status: u16, body: &str) -> Result<(), Refusal> {
    #[derive(Deserialize, Default)]
    struct Answer {
        #[serde(default)]
        success: Option<bool>,
        #[serde(default)]
        code: Option<String>,
    }
    let answer: Answer = serde_json::from_str(body).unwrap_or_default();
    let code = answer.code.as_deref().unwrap_or("");
    match status {
        200..=299 if answer.success != Some(false) => Ok(()),
        200..=299 => {
            tracing::warn!(status, code, "feedback: the service said no");
            Err(Refusal::Failure(FeedbackFailure::Rejected))
        }
        400 | 403 if code == "captcha_expired_or_reused" => Err(Refusal::CaptchaExpired),
        413 => {
            tracing::warn!("feedback: the request was too large");
            Err(Refusal::Failure(FeedbackFailure::TooLarge))
        }
        429 => {
            tracing::warn!(code, "feedback: rate limited");
            Err(Refusal::Failure(FeedbackFailure::Busy))
        }
        500..=599 => {
            tracing::warn!(status, code, "feedback: the service failed");
            Err(Refusal::Failure(FeedbackFailure::Service))
        }
        _ => {
            tracing::warn!(status, code, "feedback: the service refused");
            Err(Refusal::Failure(FeedbackFailure::Rejected))
        }
    }
}

/// The fields of one submission, besides the key and the captcha token.
struct Fields {
    subject: String,
    message: String,
    reply_to: Option<String>,
    system: String,
}

impl Fields {
    /// Without an attachment: the documented JSON body.
    fn json(&self, key: &str, token: &str) -> String {
        let mut body = serde_json::Map::new();
        for (name, value) in self.pairs(key, token) {
            body.insert(
                name.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }
        serde_json::Value::Object(body).to_string()
    }

    /// With the zipped logs: `multipart/form-data`, built by hand because it
    /// is four text parts and one file, and reqwest's multipart support would
    /// pull in a MIME-guessing crate for a type this code already knows.
    fn multipart(&self, key: &str, token: &str, zip: &[u8]) -> (String, Vec<u8>) {
        let boundary = format!("aperio-{}", uuid::Uuid::new_v4().simple());
        let mut body = Vec::new();
        for (name, value) in self.pairs(key, token) {
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\
                     Content-Type: text/plain; charset=utf-8\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(value.as_bytes());
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"logs\"; \
                 filename=\"{LOGS_FILE_NAME}\"\r\nContent-Type: application/zip\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(zip);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    fn pairs<'a>(&'a self, key: &'a str, token: &'a str) -> Vec<(&'static str, &'a str)> {
        let mut pairs = vec![
            ("apiKey", key),
            ("altchaToken", token),
            ("subject", self.subject.as_str()),
            ("message", self.message.as_str()),
            ("system", self.system.as_str()),
        ];
        if let Some(reply_to) = self.reply_to.as_deref() {
            pairs.push(("replyTo", reply_to));
        }
        pairs
    }
}

/// Deliberately loose: one `@`, something before it, a dot somewhere after it,
/// no spaces. The service checks the rest, and a reply simply bounces.
fn looks_like_email(email: &str) -> bool {
    let Some((local, domain)) = email.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !email.chars().any(char::is_whitespace)
}

/// The zip a send attaches, and the text in it. The text a host hands back is
/// the text it showed, but the host is not trusted to have kept to the rules:
/// redact it again, cap it and fit it, in the order [`logs`] does. For the
/// text `logs` gave, the text comes out unchanged.
fn prepare_logs(
    text: &str,
    ceiling: usize,
    budget: usize,
) -> zip::result::ZipResult<(String, Vec<u8>)> {
    let text = capped(&crate::logging::redact_text(text), ceiling);
    fit_zipped(text, budget)
}

fn zip_logs(text: &str) -> zip::result::ZipResult<Vec<u8>> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file(LOGS_ENTRY_NAME, options)?;
    writer.write_all(text.as_bytes())?;
    Ok(writer.finish()?.into_inner())
}

/// An ALTCHA challenge (protocol v1), as the challenge endpoint returns it.
#[derive(Debug, Clone, Deserialize)]
struct Challenge {
    algorithm: String,
    challenge: String,
    #[serde(default, alias = "maxNumber")]
    maxnumber: Option<u64>,
    salt: String,
    signature: String,
}

/// The number `n` with `SHA-256(salt + n)` equal to the challenge, if one is in
/// range. Only SHA-256: Static Forms issues it, and the other two ALTCHA
/// algorithms are not worth a code path nobody calls.
fn solve(challenge: &Challenge) -> Option<u64> {
    if !challenge.algorithm.eq_ignore_ascii_case("SHA-256") {
        tracing::warn!(algorithm = %challenge.algorithm, "feedback: unsupported captcha algorithm");
        return None;
    }
    let target = decode_hex_32(&challenge.challenge)?;
    let max = challenge
        .maxnumber
        .unwrap_or(DEFAULT_MAX_NUMBER)
        .min(MAX_NUMBER_CEILING);
    use std::fmt::Write as _;
    let mut digits = String::with_capacity(20);
    for n in 0..=max {
        digits.clear();
        let _ = write!(digits, "{n}");
        let mut hasher = Sha256::new();
        hasher.update(challenge.salt.as_bytes());
        hasher.update(digits.as_bytes());
        if hasher.finalize().as_slice() == target {
            return Some(n);
        }
    }
    None
}

/// The answer the service expects in `altchaToken`: the challenge's own fields
/// plus the number, as base64 of their JSON.
fn altcha_token(challenge: &Challenge, number: u64) -> String {
    let payload = serde_json::json!({
        "algorithm": challenge.algorithm,
        "challenge": challenge.challenge,
        "number": number,
        "salt": challenge.salt,
        "signature": challenge.signature,
    });
    base64::engine::general_purpose::STANDARD.encode(payload.to_string())
}

fn decode_hex_32(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim().as_bytes();
    if hex.len() != 64 {
        return None;
    }
    let nibble = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = (nibble(hex[2 * i])? << 4) | nibble(hex[2 * i + 1])?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn challenge_for(salt: &str, number: u64, maxnumber: Option<u64>) -> Challenge {
        let digest = Sha256::digest(format!("{salt}{number}").as_bytes());
        Challenge {
            algorithm: "SHA-256".into(),
            challenge: hex(&digest),
            maxnumber,
            salt: salt.into(),
            signature: "sig-from-the-server".into(),
        }
    }

    fn challenge_json(salt: &str, number: u64) -> String {
        let c = challenge_for(salt, number, Some(100_000));
        serde_json::json!({
            "algorithm": c.algorithm,
            "challenge": c.challenge,
            "maxnumber": 100_000,
            "salt": c.salt,
            "signature": c.signature,
        })
        .to_string()
    }

    fn draft(message: &str) -> FeedbackDraft {
        FeedbackDraft {
            message: message.into(),
            email: None,
            logs: None,
        }
    }

    // ── ALTCHA ──────────────────────────────────────────────────────────────

    #[test]
    fn the_solver_finds_the_number_the_challenge_was_made_from() {
        let c = challenge_for("abc123?expires=1700000000", 4242, None);
        assert_eq!(solve(&c), Some(4242));
    }

    #[test]
    fn the_solver_accepts_an_upper_case_challenge() {
        let mut c = challenge_for("salt", 7, None);
        c.challenge = c.challenge.to_ascii_uppercase();
        assert_eq!(solve(&c), Some(7));
    }

    #[test]
    fn the_solver_stops_at_the_challenges_limit() {
        let c = challenge_for("salt", 501, Some(500));
        assert_eq!(solve(&c), None, "501 is past the challenge's maxnumber");
        let c = challenge_for("salt", 500, Some(500));
        assert_eq!(solve(&c), Some(500), "the limit itself is in range");
    }

    #[test]
    fn the_solver_never_goes_past_its_own_ceiling() {
        let c = challenge_for("salt", 1, Some(u64::MAX));
        assert_eq!(solve(&c), Some(1));
        // The ceiling itself is what stops a hostile maxnumber; pin its size so
        // raising it is a decision, not a drive-by.
        assert_eq!(MAX_NUMBER_CEILING, 10_000_000);
    }

    #[test]
    fn the_solver_refuses_other_algorithms_and_bad_hex() {
        let mut c = challenge_for("salt", 3, None);
        c.algorithm = "SHA-512".into();
        assert_eq!(solve(&c), None);
        let mut c = challenge_for("salt", 3, None);
        c.challenge = "zz".repeat(32);
        assert_eq!(solve(&c), None);
        let mut c = challenge_for("salt", 3, None);
        c.challenge.pop();
        assert_eq!(solve(&c), None);
    }

    #[test]
    fn the_token_is_base64_json_of_the_challenge_and_the_number() {
        let c = challenge_for("s", 12, None);
        let token = altcha_token(&c, 12);
        let raw = base64::engine::general_purpose::STANDARD
            .decode(token)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(v["algorithm"], "SHA-256");
        assert_eq!(v["challenge"], c.challenge.as_str());
        assert_eq!(v["number"], 12);
        assert_eq!(v["salt"], "s");
        assert_eq!(v["signature"], "sig-from-the-server");
        assert_eq!(v.as_object().unwrap().len(), 5);
    }

    #[test]
    fn a_challenge_parses_with_either_spelling_of_its_limit() {
        let lower: Challenge = serde_json::from_str(
            r#"{"algorithm":"SHA-256","challenge":"00","maxnumber":5,"salt":"s","signature":"x"}"#,
        )
        .unwrap();
        assert_eq!(lower.maxnumber, Some(5));
        let camel: Challenge = serde_json::from_str(
            r#"{"algorithm":"SHA-256","challenge":"00","maxNumber":6,"salt":"s","signature":"x"}"#,
        )
        .unwrap();
        assert_eq!(camel.maxnumber, Some(6));
        let none: Challenge = serde_json::from_str(
            r#"{"algorithm":"SHA-256","challenge":"00","salt":"s","signature":"x"}"#,
        )
        .unwrap();
        assert_eq!(none.maxnumber, None);
    }

    // ── System line ─────────────────────────────────────────────────────────

    #[test]
    fn the_system_line_names_version_system_and_architecture() {
        let line = system_line(&HostDetails::default());
        let parts: Vec<&str> = line.split(" · ").collect();
        assert_eq!(parts.len(), 3, "{line}");
        assert_eq!(parts[0], format!("Aperio {}", env!("CARGO_PKG_VERSION")));
        assert!(!parts[1].trim().is_empty(), "{line}");
        assert_eq!(parts[2], std::env::consts::ARCH);
    }

    /// The label this machine reads for itself, checked on every CI system:
    /// Windows (a server there), macOS and Linux each read their own way.
    #[test]
    fn this_machine_names_itself() {
        let label = own_os_label();
        let line = system_line(&HostDetails::default());
        // Seen with --nocapture: what each system really reads.
        println!("this machine: {line:?}");
        if let Some(label) = &label {
            assert_eq!(
                line.split(" · ").nth(1),
                Some(label.as_str()),
                "the line carries the label as read"
            );
        }
        if cfg!(windows) {
            let label = label.expect("Windows always has a label");
            assert!(label.starts_with("Windows"), "{label}");
            assert!(label.contains("(build "), "{label}");
            assert!(
                !label.contains("10.0."),
                "never the kernel's version: {label}"
            );
        } else if cfg!(target_os = "macos") {
            let label = label.expect("sw_vers answers on macOS");
            assert!(label.starts_with("macOS "), "{label}");
            assert!(label.chars().any(|c| c.is_ascii_digit()), "{label}");
        } else if cfg!(target_os = "linux") {
            // A container may lack os-release; then the line says "Linux".
            if let Some(label) = label {
                assert!(!label.trim().is_empty());
            }
        }
    }

    fn windows(
        name: Option<&str>,
        display: Option<&str>,
        release: Option<&str>,
        build: u32,
        ubr: u32,
        is_server: bool,
    ) -> WindowsFacts {
        WindowsFacts {
            product_name: name.map(Into::into),
            display_version: display.map(Into::into),
            release_id: release.map(Into::into),
            build,
            ubr,
            is_server,
            compat_mode: false,
        }
    }

    #[test]
    fn windows_11_is_named_by_its_build_not_its_product_name() {
        // Toni's machine, read 2026-11-16: the registry still says "Windows 10".
        assert_eq!(
            windows_label(&windows(
                Some("Windows 10 Pro"),
                Some("25H2"),
                Some("2009"),
                26220,
                9836,
                false
            )),
            "Windows 11 Pro 25H2 (build 26220.9836)"
        );
        // 22000 is Windows 11's first build, and counts.
        assert_eq!(
            windows_label(&windows(
                Some("Windows 10 Home"),
                Some("21H2"),
                None,
                22000,
                1,
                false
            )),
            "Windows 11 Home 21H2 (build 22000.1)"
        );
        assert_eq!(
            windows_label(&windows(Some("Windows 10"), None, None, 22631, 0, false)),
            "Windows 11 (build 22631)"
        );
    }

    #[test]
    fn windows_10_stays_windows_10() {
        assert_eq!(
            windows_label(&windows(
                Some("Windows 10 Pro"),
                Some("22H2"),
                Some("2009"),
                19045,
                7850,
                false
            )),
            "Windows 10 Pro 22H2 (build 19045.7850)"
        );
        // Before 20H2 there is no DisplayVersion; ReleaseId names the update.
        assert_eq!(
            windows_label(&windows(
                Some("Windows 10 Pro"),
                None,
                Some("1909"),
                18363,
                418,
                false
            )),
            "Windows 10 Pro 1909 (build 18363.418)"
        );
        // Only "Windows 10" itself is renamed, not a name that merely starts so.
        assert_eq!(
            windows_label(&windows(Some("Windows 100"), None, None, 26100, 0, false)),
            "Windows 100 (build 26100)"
        );
    }

    #[test]
    fn a_server_is_never_renamed_windows_11() {
        // Server 2025 shares build 26100 with Windows 11 24H2.
        assert_eq!(
            windows_label(&windows(
                Some("Windows Server 2025 Standard"),
                Some("24H2"),
                Some("2009"),
                26100,
                4061,
                true
            )),
            "Windows Server 2025 Standard 24H2 (build 26100.4061)"
        );
        assert_eq!(
            windows_label(&windows(Some("Windows 10 Pro"), None, None, 26100, 0, true)),
            "Windows 10 Pro (build 26100)"
        );
    }

    #[test]
    fn missing_or_faked_facts_still_name_the_build() {
        assert_eq!(
            windows_label(&WindowsFacts {
                build: 26220,
                ubr: 9836,
                ..Default::default()
            }),
            "Windows (build 26220.9836)"
        );
        assert_eq!(
            windows_label(&WindowsFacts {
                product_name: Some("  ".into()),
                display_version: Some(String::new()),
                release_id: Some("2009".into()),
                build: 26220,
                ..Default::default()
            }),
            "Windows 2009 (build 26220)"
        );
        assert_eq!(
            windows_label(&WindowsFacts {
                display_version: Some("25H2".into()),
                build: 26220,
                ubr: 9836,
                compat_mode: true,
                ..Default::default()
            }),
            "Windows 25H2 (build 26220.9836, compatibility mode)"
        );
        assert_eq!(windows_label(&WindowsFacts::default()), "Windows");
    }

    #[test]
    fn macos_reads_name_version_and_build_from_sw_vers() {
        assert_eq!(
            macos_label(
                "ProductName:\t\tmacOS\nProductVersion:\t\t15.6.1\nBuildVersion:\t\t24G90\n"
            )
            .as_deref(),
            Some("macOS 15.6.1 (build 24G90)")
        );
        assert_eq!(
            macos_label("ProductVersion: 15.1\n").as_deref(),
            Some("macOS 15.1")
        );
        assert_eq!(
            macos_label("ProductName: macOS\n"),
            None,
            "no version, no label"
        );
        assert_eq!(macos_label(""), None);
    }

    #[test]
    fn the_system_line_carries_what_the_host_knows() {
        let line = system_line(&HostDetails {
            app_version: Some("1.0.0".into()),
            build: Some("70".into()),
            os_version: Some("18.1".into()),
            device: Some("Pixel 8".into()),
        });
        let os = os_name(std::env::consts::OS);
        assert_eq!(
            line,
            format!(
                "Aperio {} (app 1.0.0, build 70) · {os} 18.1 · {} · Pixel 8",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH
            )
        );
    }

    #[test]
    fn blank_host_details_count_as_absent() {
        let blank = system_line(&HostDetails {
            app_version: Some("  ".into()),
            build: Some(String::new()),
            os_version: None,
            device: Some(" ".into()),
        });
        assert_eq!(blank, system_line(&HostDetails::default()));
        assert!(!blank.contains("()"));
    }

    #[test]
    fn os_names_read_as_their_makers_write_them() {
        assert_eq!(os_name("windows"), "Windows");
        assert_eq!(os_name("macos"), "macOS");
        assert_eq!(os_name("linux"), "Linux");
        assert_eq!(os_name("ios"), "iOS");
        assert_eq!(os_name("android"), "Android");
        assert_eq!(os_name("freebsd"), "freebsd");
    }

    #[test]
    fn os_release_yields_its_pretty_name() {
        let text = "NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nID=ubuntu\n";
        assert_eq!(
            os_release_pretty_name(text).as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(os_release_pretty_name("PRETTY_NAME=\n"), None);
        assert_eq!(os_release_pretty_name("NAME=x\n"), None);
    }

    // ── Input checks ────────────────────────────────────────────────────────

    #[test]
    fn email_addresses_are_checked_loosely() {
        for good in ["a@b.de", "first.last+tag@sub.example.org"] {
            assert!(looks_like_email(good), "{good}");
        }
        for bad in [
            "", "plain", "@b.de", "a@", "a@b", "a@.de", "a@b.", "a b@c.de", "a@b@c.de",
        ] {
            assert!(!looks_like_email(bad), "{bad}");
        }
    }

    /// What a send makes of a text, with small bounds so the tests run fast.
    fn prepared(text: &str, ceiling: usize, budget: usize) -> String {
        prepare_logs(text, ceiling, budget).unwrap().0
    }

    /// Log lines that zip about as badly as real ones with ids in them.
    fn varied_lines(count: usize) -> String {
        (0..count)
            .map(|i| {
                // UUID-like pieces: runs under 32 characters, which the
                // redaction keeps, as it keeps real ids.
                let h = hex(&Sha256::digest(i.to_string().as_bytes()));
                let id = format!("{}-{}-{}", &h[0..8], &h[8..20], &h[20..28]);
                format!("2026-11-16T18:00:00Z  INFO aperio: a@b.de synced {id}\n")
            })
            .collect()
    }

    #[test]
    fn the_logs_are_capped_to_the_newest_bytes_and_redacted() {
        let ceiling = 64 * 1024;
        let mut text = "x".repeat(ceiling);
        text.push_str("\nme@example.org says hi\n");
        let out = prepared(&text, ceiling, 1_000_000);
        assert!(out.len() <= ceiling, "capped");
        assert!(
            out.ends_with("[redacted-email] says hi\n"),
            "newest kept, redacted"
        );
        assert!(!out.contains("me@example.org"));
    }

    #[test]
    fn capping_never_splits_a_character() {
        // One line of two-byte characters, cut where the arithmetic lands
        // inside one: the cut has to move to the next character.
        let max = 64 * 1024;
        let mut text = "ä".repeat(max / 2 + 1);
        text.push_str("öx");
        assert_eq!((text.len() - (max - TRUNCATED.len())) % 2, 1);
        let out = capped(&text, max);
        assert!(out.ends_with("öx"));
        assert!(out.starts_with(TRUNCATED));
        assert!(out[TRUNCATED.len()..].starts_with('ä'));
        assert!(out.len() <= max);
    }

    #[test]
    fn capping_keeps_whole_lines_and_says_what_it_cut() {
        // Room for the marker and 8 bytes; an older line longer than that.
        let max = TRUNCATED.len() + 8;
        let old = format!("{}\n", "x".repeat(40));
        // The cut lands right after a newline: that line is whole and stays.
        assert_eq!(
            capped(&format!("{old}bbb\nccc\n"), max),
            format!("{TRUNCATED}bbb\nccc\n")
        );
        // The cut lands inside a line: that line goes.
        assert_eq!(
            capped(&format!("{old}bbbbb\ncc\n"), max),
            format!("{TRUNCATED}cc\n")
        );
        // Text that fits is left alone.
        assert_eq!(capped("short\n", max), "short\n");
    }

    #[test]
    fn what_the_form_shows_is_what_is_sent() {
        // More than fits, with addresses that grow when redacted: the form
        // shows the newest lines that fit zipped, and sending changes nothing,
        // neither the text nor the zip it makes.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("aperio.log.2026-11-16"),
            varied_lines(3_000),
        )
        .unwrap();
        let (ceiling, budget) = (1024 * 1024, 20_000);

        let shown = logs_within(dir.path(), ceiling, budget);
        assert!(shown.starts_with(TRUNCATED), "older lines were cut");
        assert!(!shown.contains("a@b.de"));
        let zip = zip_logs(&shown).unwrap();
        assert!(zip.len() <= budget, "{}", zip.len());

        let (sent, sent_zip) = prepare_logs(&shown, ceiling, budget).unwrap();
        assert_eq!(sent, shown, "sending changes nothing");
        assert_eq!(sent_zip, zip);
    }

    #[test]
    fn a_send_caps_after_redacting() {
        // Under the ceiling as handed back, over it once redacted: a cap taken
        // before the redaction would let the text grow past it.
        let ceiling = 4 * 1024;
        let text = "a@b.de\n".repeat(ceiling / 7);
        assert!(text.len() <= ceiling);
        let sent = prepared(&text, ceiling, 1_000_000);
        assert!(sent.len() <= ceiling, "{}", sent.len());
        assert!(sent.starts_with(TRUNCATED));
    }

    #[test]
    fn the_shown_text_is_capped_after_redacting() {
        let dir = tempfile::tempdir().unwrap();
        let ceiling = 4 * 1024;
        std::fs::write(
            dir.path().join("aperio.log.2026-11-16"),
            "a@b.de\n".repeat(ceiling / 7),
        )
        .unwrap();
        let shown = logs_within(dir.path(), ceiling, 1_000_000);
        assert!(shown.len() <= ceiling, "{}", shown.len());
    }

    #[test]
    fn as_much_as_fits_goes_along() {
        let text = crate::logging::redact_text(&varied_lines(3_000));
        let budget = 20_000;
        let (fitted, zip) = fit_zipped(text.clone(), budget).unwrap();
        assert!(zip.len() <= budget);
        // Not a token sample: the shrink aims just under the budget.
        assert!(zip.len() * 10 >= budget * 8, "{} of {budget}", zip.len());
        assert!(
            text.ends_with(fitted.strip_prefix(TRUNCATED).unwrap()),
            "the newest lines"
        );
        // Text whose zip fits is left as it is.
        let (small, _) = fit_zipped("one line\n".to_string(), budget).unwrap();
        assert_eq!(small, "one line\n");
    }

    #[test]
    fn redacting_twice_changes_nothing() {
        let once = crate::logging::redact_text("a@b.de token_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdef0123");
        assert_eq!(crate::logging::redact_text(&once), once);
    }

    #[test]
    fn the_zip_holds_the_log_text_under_its_name() {
        let zip = zip_logs("line one\nline two\n").unwrap();
        let mut archive = zip::ZipArchive::new(Cursor::new(zip)).unwrap();
        assert_eq!(archive.len(), 1);
        let mut entry = archive.by_name(LOGS_ENTRY_NAME).unwrap();
        let mut back = String::new();
        entry.read_to_string(&mut back).unwrap();
        assert_eq!(back, "line one\nline two\n");
    }

    #[test]
    fn the_logs_text_is_the_redacted_newest_tail_of_every_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("aperio.log.2026-11-15"),
            "old me@example.org\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("aperio.log.2026-11-16"), "new line\n").unwrap();
        let text = logs(dir.path());
        assert!(text.contains("old [redacted-email]"));
        assert!(text.contains("new line"));
        assert!(text.find("old").unwrap() < text.find("new").unwrap());
    }

    // ── Submit verdicts ─────────────────────────────────────────────────────

    #[test]
    fn verdicts_follow_the_status() {
        assert_eq!(
            submit_verdict(200, r#"{"success":true,"message":"ok"}"#),
            Ok(())
        );
        assert_eq!(submit_verdict(200, "plain text"), Ok(()));
        assert_eq!(
            submit_verdict(200, r#"{"success":false}"#),
            Err(Refusal::Failure(FeedbackFailure::Rejected))
        );
        assert_eq!(
            submit_verdict(413, r#"{"error":{"code":"413"}}"#),
            Err(Refusal::Failure(FeedbackFailure::TooLarge))
        );
        assert_eq!(
            submit_verdict(429, r#"{"error":"Rate limit exceeded."}"#),
            Err(Refusal::Failure(FeedbackFailure::Busy))
        );
        assert_eq!(
            submit_verdict(503, ""),
            Err(Refusal::Failure(FeedbackFailure::Service))
        );
        assert_eq!(
            submit_verdict(401, r#"{"success":false,"error":"Invalid API key"}"#),
            Err(Refusal::Failure(FeedbackFailure::Rejected))
        );
        assert_eq!(
            submit_verdict(403, r#"{"success":false,"code":"captcha_failed"}"#),
            Err(Refusal::Failure(FeedbackFailure::Rejected))
        );
    }

    #[test]
    fn only_an_expired_captcha_asks_for_another_try() {
        for status in [400, 403] {
            assert_eq!(
                submit_verdict(
                    status,
                    r#"{"success":false,"code":"captcha_expired_or_reused"}"#
                ),
                Err(Refusal::CaptchaExpired)
            );
        }
        assert_eq!(
            submit_verdict(
                429,
                r#"{"success":false,"code":"captcha_expired_or_reused"}"#
            ),
            Err(Refusal::Failure(FeedbackFailure::Busy)),
            "a rate limit stays a rate limit whatever code it carries"
        );
    }

    #[test]
    fn outcomes_travel_tagged() {
        assert_eq!(
            serde_json::to_value(FeedbackOutcome::Sent).unwrap(),
            serde_json::json!({"outcome": "sent"})
        );
        assert_eq!(
            serde_json::to_value(FeedbackOutcome::from(Err(FeedbackFailure::InvalidEmail)))
                .unwrap(),
            serde_json::json!({"outcome": "failed", "reason": "invalid_email"})
        );
    }

    // ── Sending, against a local server ─────────────────────────────────────

    struct Local {
        server: mockito::ServerGuard,
    }

    impl Local {
        async fn new() -> Self {
            Self {
                server: mockito::Server::new_async().await,
            }
        }

        fn challenge_url(&self) -> String {
            format!("{}/challenge", self.server.url())
        }

        fn submit_url(&self) -> String {
            format!("{}/submit", self.server.url())
        }

        async fn send(&self, draft: &FeedbackDraft) -> Result<(), FeedbackFailure> {
            let challenge = self.challenge_url();
            let submit = self.submit_url();
            Service {
                challenge_url: &challenge,
                submit_url: &submit,
                key: "test-key",
            }
            .send(draft, &HostDetails::default())
            .await
        }
    }

    fn challenge_mock(server: &mut mockito::ServerGuard, number: u64) -> mockito::Mock {
        server
            .mock("GET", "/challenge")
            .match_query(mockito::Matcher::UrlEncoded(
                "apiKey".into(),
                "test-key".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(challenge_json("salt?expires=1", number))
    }

    fn token_with_number(number: u64) -> String {
        altcha_token(
            &challenge_for("salt?expires=1", number, Some(100_000)),
            number,
        )
    }

    #[tokio::test]
    async fn a_message_without_logs_goes_as_json_with_a_solved_captcha() {
        let mut local = Local::new().await;
        let challenge = challenge_mock(&mut local.server, 321).create_async().await;
        let submit = local
            .server
            .mock("POST", "/submit")
            .match_header("content-type", "application/json")
            .match_header("accept", "application/json")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "apiKey": "test-key",
                "altchaToken": token_with_number(321),
                "message": "Hallo, äöü",
                "replyTo": "me@example.org",
                "system": system_line(&HostDetails::default()),
            })))
            .with_status(200)
            .with_body(r#"{"success":true,"message":"Form submission received"}"#)
            .create_async()
            .await;

        let result = local
            .send(&FeedbackDraft {
                message: "  Hallo, äöü \n".into(),
                email: Some(" me@example.org ".into()),
                logs: None,
            })
            .await;

        assert_eq!(result, Ok(()));
        challenge.assert_async().await;
        submit.assert_async().await;
    }

    #[tokio::test]
    async fn a_blank_address_sends_no_reply_to() {
        let mut local = Local::new().await;
        let _challenge = challenge_mock(&mut local.server, 1).create_async().await;
        // mockito has no "body lacks" matcher, so the server answers a body
        // that names a reply-to with a refusal the send would report.
        let submit = local
            .server
            .mock("POST", "/submit")
            .with_status(200)
            .with_body_from_request(|request| {
                let body = request.utf8_lossy_body().unwrap_or_default();
                if body.contains("replyTo") {
                    br#"{"success":false}"#.to_vec()
                } else {
                    br#"{"success":true}"#.to_vec()
                }
            })
            .create_async()
            .await;
        let result = local
            .send(&FeedbackDraft {
                message: "x".into(),
                email: Some("   ".into()),
                logs: None,
            })
            .await;
        assert_eq!(result, Ok(()));
        submit.assert_async().await;
    }

    #[tokio::test]
    async fn logs_go_as_a_zip_in_a_multipart_body() {
        let mut local = Local::new().await;
        let _challenge = challenge_mock(&mut local.server, 9).create_async().await;
        let submit = local
            .server
            .mock("POST", "/submit")
            .match_header(
                "content-type",
                mockito::Matcher::Regex("^multipart/form-data; boundary=aperio-".into()),
            )
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::Regex("name=\"apiKey\"".into()),
                mockito::Matcher::Regex("name=\"altchaToken\"".into()),
                mockito::Matcher::Regex("name=\"message\"".into()),
                mockito::Matcher::Regex(
                    "name=\"logs\"; filename=\"aperio-logs.zip\"\r\nContent-Type: application/zip"
                        .into(),
                ),
            ]))
            .with_status(200)
            .with_body(r#"{"success":true}"#)
            .create_async()
            .await;

        let result = local
            .send(&FeedbackDraft {
                message: "with logs".into(),
                email: None,
                logs: Some("a line\nme@example.org\n".into()),
            })
            .await;
        assert_eq!(result, Ok(()));
        submit.assert_async().await;
    }

    #[test]
    fn the_multipart_body_carries_the_redacted_zip_intact() {
        let fields = Fields {
            subject: "Aperio feedback".into(),
            message: "Grüße".into(),
            reply_to: Some("a@b.de".into()),
            system: "sys".into(),
        };
        let (_, zip) = prepare_logs(
            "me@example.org did a thing\n",
            LOGS_TEXT_CEILING,
            LOGS_ZIP_BUDGET,
        )
        .unwrap();
        let (content_type, body) = fields.multipart("k", "t", &zip);
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        assert!(body.ends_with(format!("\r\n--{boundary}--\r\n").as_bytes()));

        let head = b"filename=\"aperio-logs.zip\"\r\nContent-Type: application/zip\r\n\r\n";
        let start = body.windows(head.len()).position(|w| w == head).unwrap() + head.len();
        let tail = format!("\r\n--{boundary}--\r\n");
        let file = &body[start..body.len() - tail.len()];
        assert_eq!(file, zip.as_slice(), "the zip arrives byte for byte");

        let mut archive = zip::ZipArchive::new(Cursor::new(file.to_vec())).unwrap();
        let mut text = String::new();
        archive
            .by_index(0)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "[redacted-email] did a thing\n");

        let as_text = String::from_utf8_lossy(&body);
        for name in [
            "apiKey",
            "altchaToken",
            "subject",
            "message",
            "system",
            "replyTo",
        ] {
            assert!(as_text.contains(&format!("name=\"{name}\"")), "{name}");
        }
        assert!(as_text.contains("Grüße"));
    }

    #[tokio::test]
    async fn an_expired_captcha_is_tried_once_more_with_a_fresh_challenge() {
        let mut local = Local::new().await;
        let challenge = challenge_mock(&mut local.server, 5)
            .expect(2)
            .create_async()
            .await;
        let submit = local
            .server
            .mock("POST", "/submit")
            .with_status(400)
            .with_body(r#"{"success":false,"code":"captcha_expired_or_reused"}"#)
            .expect(2)
            .create_async()
            .await;
        assert_eq!(
            local.send(&draft("x")).await,
            Err(FeedbackFailure::Rejected),
            "two expiries in a row are a refusal, not a loop"
        );
        challenge.assert_async().await;
        submit.assert_async().await;
    }

    #[tokio::test]
    async fn the_input_is_checked_before_anything_is_fetched() {
        let mut local = Local::new().await;
        let challenge = challenge_mock(&mut local.server, 1)
            .expect(0)
            .create_async()
            .await;
        assert_eq!(
            local.send(&draft(" \n ")).await,
            Err(FeedbackFailure::EmptyMessage)
        );
        assert_eq!(
            local
                .send(&FeedbackDraft {
                    message: "x".into(),
                    email: Some("not an address".into()),
                    logs: None,
                })
                .await,
            Err(FeedbackFailure::InvalidEmail)
        );
        challenge.assert_async().await;
    }

    #[tokio::test]
    async fn a_rate_limited_challenge_reads_as_busy() {
        let mut local = Local::new().await;
        let _challenge = local
            .server
            .mock("GET", "/challenge")
            .match_query(mockito::Matcher::Any)
            .with_status(429)
            .with_body(r#"{"error":"Rate limit exceeded. Please try again later."}"#)
            .create_async()
            .await;
        let submit = local
            .server
            .mock("POST", "/submit")
            .expect(0)
            .create_async()
            .await;
        assert_eq!(local.send(&draft("x")).await, Err(FeedbackFailure::Busy));
        submit.assert_async().await;
    }

    #[tokio::test]
    async fn a_challenge_that_does_not_parse_is_the_services_fault() {
        let mut local = Local::new().await;
        let _challenge = local
            .server
            .mock("GET", "/challenge")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body("<html>not json</html>")
            .create_async()
            .await;
        assert_eq!(local.send(&draft("x")).await, Err(FeedbackFailure::Service));
    }

    #[tokio::test]
    async fn an_unreachable_service_reads_as_network() {
        // A port nothing listens on. mockito's servers go back to a pool and
        // keep listening when dropped, so a bare listener is taken and closed.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let challenge = format!("http://127.0.0.1:{port}/challenge");
        let submit = format!("http://127.0.0.1:{port}/submit");
        let result = Service {
            challenge_url: &challenge,
            submit_url: &submit,
            key: "test-key",
        }
        .send(&draft("x"), &HostDetails::default())
        .await;
        assert_eq!(result, Err(FeedbackFailure::Network));
    }

    #[tokio::test]
    async fn an_oversized_message_is_refused_before_it_is_sent() {
        let mut local = Local::new().await;
        // Not even a challenge: those count against a limit every Aperio user
        // shares.
        let challenge = challenge_mock(&mut local.server, 2)
            .expect(0)
            .create_async()
            .await;
        let submit = local
            .server
            .mock("POST", "/submit")
            .expect(0)
            .create_async()
            .await;
        let huge = "x".repeat(REQUEST_MAX_BYTES + 1);
        assert_eq!(
            local.send(&draft(&huge)).await,
            Err(FeedbackFailure::TooLarge)
        );
        challenge.assert_async().await;
        submit.assert_async().await;
    }

    #[tokio::test]
    async fn a_build_without_a_key_says_so_and_sends_nothing() {
        if builtin_oauth::feedback_form_key().is_some() {
            // A credentialed build cannot test the keyless path through the
            // public entry; the posture is pinned in builtin-oauth instead.
            return;
        }
        assert_eq!(
            send(&draft("x"), &HostDetails::default()).await,
            FeedbackOutcome::Failed {
                reason: FeedbackFailure::Unavailable
            }
        );
        assert!(!info(&HostDetails::default()).available);
    }
}
