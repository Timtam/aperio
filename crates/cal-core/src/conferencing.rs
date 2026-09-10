//! Find the online meeting in an event — whoever created it.
//!
//! An event with a conference is an event with a conference, whether Aperio
//! made it, or Outlook, or eM Client, or the Webex Scheduler, or someone
//! forwarding an invitation from a company that uses a different tool. The
//! "Join" affordance has to work for all of them, so detection is deliberately
//! independent of anything Aperio writes.
//!
//! ## Why URLs and not text
//!
//! The obvious approach is to read the invitation the way a human does —
//! "Meeting number (access code): …", "Meeting-Kennnummer: …". It does not
//! survive contact with reality. Webex localises its invitation template across
//! more than 26 languages, site administrators can edit those templates, and
//! Cisco publishes no machine-readable version of any of them. Building on that
//! would make the Join button depend on Cisco's prose in a language nobody
//! chose.
//!
//! URLs do not have that problem. `j.php?MTID=` looks the same in German as in
//! English. So does a SIP address, and so does the DTMF sequence in a `tel:`
//! link — which, usefully, carries the meeting number and the password in a
//! form no translation touches:
//!
//! ```text
//! tel:+1-555-0100,,*01*25503113955%23626114%23*01*
//!                     └ meeting number ┘ └ pw ┘
//! ```
//!
//! ## What the standards offer, honestly
//!
//! RFC 7986 §5.11 defines `CONFERENCE;VALUE=URI` for exactly this, and
//! essentially nobody emits it. Google writes `X-GOOGLE-CONFERENCE`, Exchange
//! writes `X-MICROSOFT-SKYPETEAMSMEETINGURL`, and both are vendor extensions
//! for their own product. A real Webex invitation carries none of them: its
//! whole property set is the RFC 5545 basics plus `X-ALT-DESC`. Microsoft has
//! the same gap and fills it the same way — its Outlook add-in model matches a
//! body template the vendor registers, i.e. Microsoft also falls back to text.
//!
//! So the layering is: use a structured field when a provider gives one (the
//! adapter passes it in), and otherwise scan the fields every calendar has.
//! Aperio emits `CONFERENCE` on its own writes regardless, because being the
//! only standards-compliant client is still better than being one more that is
//! not.

use serde::{Deserialize, Serialize};

/// Which service a join link belongs to.
///
/// Used to label the affordance ("Join the Webex meeting") and to decide which
/// extra details are worth pulling out. `Other` is a first-class answer: a link
/// we cannot classify is still a link worth offering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
// camelCase, matching `i18n_key` below rather than the snake_case this used to
// emit. The enum had TWO spellings of itself — `google_meet` on the wire and
// `googleMeet` for the lookup — and the frontends' own type used the second.
// Nothing consumed the serialised form, so aligning them cost nothing; leaving
// them apart would have meant every i18n lookup missing the day the frontends
// started reading this.
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ConferenceProvider {
    Webex,
    Teams,
    Zoom,
    GoogleMeet,
    Jitsi,
    GoToMeeting,
    BigBlueButton,
    Whereby,
    Dfnconf,
    Other,
}

impl ConferenceProvider {
    /// A stable key for i18n lookup, so the UI names the provider in the user's
    /// language rather than hard-coding English.
    pub fn i18n_key(self) -> &'static str {
        match self {
            ConferenceProvider::Webex => "webex",
            ConferenceProvider::Teams => "teams",
            ConferenceProvider::Zoom => "zoom",
            ConferenceProvider::GoogleMeet => "googleMeet",
            ConferenceProvider::Jitsi => "jitsi",
            ConferenceProvider::GoToMeeting => "goToMeeting",
            ConferenceProvider::BigBlueButton => "bigBlueButton",
            ConferenceProvider::Whereby => "whereby",
            ConferenceProvider::Dfnconf => "dfnconf",
            ConferenceProvider::Other => "other",
        }
    }
}

/// Where a link was found. Kept so the UI can be honest about confidence and so
/// a bug report says which field to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum ConferenceSource {
    /// A provider's own structured field — Google `conferenceData`, Graph
    /// `onlineMeeting`. The adapter passed it in; nothing was guessed.
    ProviderField,
    /// RFC 7986 `CONFERENCE`.
    IcalendarConference,
    /// A vendor X-property (`X-GOOGLE-CONFERENCE`, `X-MICROSOFT-SKYPETEAMS…`).
    VendorProperty,
    /// The event's location field.
    Location,
    /// The event's description.
    Description,
}

/// An online meeting found in an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ConferenceLink {
    pub join_url: String,
    pub provider: ConferenceProvider,
    pub source: ConferenceSource,
    /// The meeting number / access code, when it could be recovered without
    /// reading prose — from the SIP address or the DTMF sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_number: Option<String>,
    /// The numeric password, from the DTMF sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// A SIP address for a video system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sip_address: Option<String>,
    /// A dial-in number, as a `tel:` URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone: Option<String>,
    /// The `label: value` lines the invitation itself puts next to the link,
    /// with the labels exactly as they were written.
    ///
    /// This is how the details survive without a dictionary: a real Webex
    /// invitation from an Exchange organiser carries no `tel:` and no `sip:`
    /// at all, and its meeting id and password sit behind prose labels —
    /// "Besprechungs-ID", "Meeting number (access code)", whatever the sender's
    /// Webex site is set to. Rather than learning those, the label is treated
    /// as DATA and handed on verbatim. A screen reader then reads
    /// "Besprechungs-ID, 27401156686" in the language the invitation actually
    /// arrived in, and Aperio never needed to know what the words mean.
    #[serde(default)]
    pub labelled_details: Vec<ConferenceDetail>,
}

/// One `label: value` line, with the label exactly as the invitation wrote it.
///
/// A named pair rather than a tuple. Tuples are fine inside Rust and awkward
/// everywhere else: they serialise as bare arrays, so a reader has to know that
/// the first slot is the label, and UniFFI records cannot carry them at all.
/// The frontends' own `ConferenceDetail` is this shape already.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ConferenceDetail {
    /// The sender's word, verbatim — this is DATA, not a key to look up.
    pub label: String,
    pub value: String,
}

/// The fields a detector looks at, in the order it prefers them.
///
/// Separate from `Event` so an adapter can pass what only it knows — Google's
/// `conferenceData`, Graph's `onlineMeeting`, the X-properties a CalDAV
/// adapter parsed — without `cal-core` growing a field for each provider.
#[derive(Debug, Default, Clone)]
pub struct ConferenceSources<'a> {
    /// A join URL a provider stated outright. Trusted first; nothing else can
    /// be more authoritative than the provider's own answer.
    pub provider_field: Option<&'a str>,
    /// RFC 7986 `CONFERENCE` values, in the order they appeared.
    pub icalendar_conference: &'a [&'a str],
    /// Vendor X-property values.
    pub vendor_properties: &'a [&'a str],
    pub location: Option<&'a str>,
    pub description: Option<&'a str>,
}

/// The fields a detector looks at, as a frontend hands them over.
///
/// [`ConferenceSources`] borrows, which a deserialiser cannot produce, so this
/// is its owning twin — the shape that crosses a binding.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ConferenceSourcesInput {
    pub provider_field: Option<String>,
    pub icalendar_conference: Vec<String>,
    pub vendor_properties: Vec<String>,
    pub location: Option<String>,
    pub description: Option<String>,
}

/// Detect a meeting from JSON, and answer with JSON.
///
/// The door both frontends come through — the desktop compiled into its webview
/// as WebAssembly, mobile over the UniFFI bridge. It lives HERE rather than in
/// each binding because two copies of a marshalling step is how the rule itself
/// came to exist twice, and the point of the move is to stop that happening
/// again.
///
/// JSON on both sides rather than a generated record: `cal-ffi` already carries
/// the whole task domain that way, `serde_json` is already in both graphs (this
/// crate needs it for the extras codec), and one shape for both doors means the
/// two cannot drift in what they accept.
///
/// `"null"` is the answer for "no meeting here", which is what an ordinary
/// appointment gets and therefore the commonest answer of all. A malformed
/// input is an `Err`, not a `None`: the caller built that JSON, so it is a bug
/// in the binding rather than a fact about the event.
pub fn detect_conference_json(sources_json: &str) -> Result<String, serde_json::Error> {
    let input: ConferenceSourcesInput = serde_json::from_str(sources_json)?;
    let icalendar: Vec<&str> = input
        .icalendar_conference
        .iter()
        .map(String::as_str)
        .collect();
    let vendor: Vec<&str> = input.vendor_properties.iter().map(String::as_str).collect();
    let found = detect_conference(&ConferenceSources {
        provider_field: input.provider_field.as_deref(),
        icalendar_conference: &icalendar,
        vendor_properties: &vendor,
        location: input.location.as_deref(),
        description: input.description.as_deref(),
    });
    serde_json::to_string(&found)
}

/// Find the meeting, or decide there is none.
///
/// Sources are tried in order of authority. Within one source the LONGEST
/// matching URL wins: Webex's newer join links nest a shorter-looking URL
/// inside their query string, and taking the first match would hand back a
/// fragment that does not join anything.
pub fn detect_conference(sources: &ConferenceSources<'_>) -> Option<ConferenceLink> {
    let ordered: [(ConferenceSource, Vec<&str>); 5] = [
        (
            ConferenceSource::ProviderField,
            sources.provider_field.into_iter().collect(),
        ),
        (
            ConferenceSource::IcalendarConference,
            sources.icalendar_conference.to_vec(),
        ),
        (
            ConferenceSource::VendorProperty,
            sources.vendor_properties.to_vec(),
        ),
        (
            ConferenceSource::Location,
            sources.location.into_iter().collect(),
        ),
        (
            ConferenceSource::Description,
            sources.description.into_iter().collect(),
        ),
    ];

    for (source, texts) in ordered {
        let mut best: Option<(String, ConferenceProvider)> = None;
        for text in &texts {
            for url in extract_urls(text) {
                let Some(provider) = classify(&url) else {
                    continue;
                };
                let better = match &best {
                    None => true,
                    // CHARACTERS, not bytes. `len()` is a byte count, and it
                    // let a URL with one non-ASCII character outrank a longer
                    // ASCII one — which is not what "the longest URL" means to
                    // anyone. The TypeScript twin counted UTF-16 code units,
                    // so the two picked DIFFERENT join urls for the same
                    // description, and `join_url` is the one field both hosts
                    // actually read: the Join button and the host's meeting
                    // resolution could target different links.
                    //
                    // Scalar values agree with UTF-16 for every ASCII and
                    // every BMP character, so this matches what the frontends
                    // did, and differs only for astral characters in a URL.
                    Some((current, _)) => url.chars().count() > current.chars().count(),
                };
                if better {
                    best = Some((url, provider));
                }
            }
        }
        if let Some((join_url, provider)) = best {
            // Details come from the whole source set, not just the field the
            // URL came from: the link is usually in the location while the
            // dial-in details are in the description.
            let all: Vec<&str> = [
                sources.provider_field,
                sources.location,
                sources.description,
            ]
            .into_iter()
            .flatten()
            .collect();
            let details = extract_details(&all);
            let labelled = sources
                .description
                .map(|d| labelled_lines_near(d, &join_url))
                .unwrap_or_default();
            return Some(ConferenceLink {
                join_url,
                provider,
                source,
                meeting_number: details.meeting_number,
                password: details.password,
                sip_address: details.sip_address,
                phone: details.phone,
                labelled_details: labelled,
            });
        }
    }
    None
}

/// Pull every http(s) URL out of free text, trimming what punctuation around a
/// link would otherwise glue on.
///
/// Written by hand rather than with a regex because the trailing-character
/// cases are the entire difficulty, and they are easier to be exact about here.
/// A URL wrapped in angle brackets that keeps its `%3E`, or a link at the end
/// of a sentence that keeps the full stop, is a link that does not open.
/// The block Aperio appends to an event's description when it creates a
/// meeting for it.
///
/// Written in the shape real invitations use — a join line, then labelled
/// detail lines — because that is what every other calendar client, and this
/// app's own detector, reads. Nothing here is Aperio-specific: an attendee
/// opening the event in Outlook sees a link, a meeting number and a dial-in
/// number, not a marker.
///
/// ## One line per fact, and plain text
///
/// Every fact gets its own `Label: value` line. Not cosmetics: the removal path
/// ([`without_meeting_block`]) works line by line, so a value wrapped across
/// two lines would survive the removal and strand half a block in the event.
/// A provider's five dial-in numbers are therefore five lines, each naming its
/// own country.
///
/// It is plain text everywhere, with no HTML twin. HTML would buy clickability
/// that clients already provide for bare URLs, and would cost: `X-ALT-DESC` is
/// read by almost nothing, a second representation goes stale when another
/// client edits the first (and then an attendee dials a number that is no
/// longer right), sanitizers strip exactly the `tel:` and `sip:` anchors this
/// exists for, and unrendered markup is read out verbatim by a screen reader —
/// NVDA is silent on `<` and `>` at default settings, so `<br>Password:` is
/// announced as "br Password".
///
/// ## The labels
///
/// Supplied by the caller, already resolved: the adapter owns the words (see
/// `plugin_core::strings`), and the host has picked the language before calling
/// here. They are frozen into somebody else's calendar the moment this returns,
/// which is why the language is a decision and not a lookup.
///
/// The detector reads them back as DATA rather than matching on them, so a
/// block that says "Meeting-Kennnummer" is understood exactly as well as one
/// that says "Meeting number".
pub fn meeting_block(lines: &[(String, String)]) -> String {
    lines
        .iter()
        .map(|(label, value)| (label.trim(), value.trim()))
        .filter(|(label, value)| !label.is_empty() && !value.is_empty())
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove the block [`meeting_block`] added, leaving the user's own text.
///
/// Matches on the JOIN URL rather than on the surrounding words, for the same
/// reason the detector does: the words may have been translated, reflowed, or
/// rewritten by another client on the way through, and the URL is the only part
/// that has to survive unchanged for the meeting to work at all.
///
/// Removes the line carrying the URL, plus any immediately following labelled
/// detail lines (`Label: value`), which is where the password sits. Text the
/// user wrote before or after the block is untouched, and a description that
/// never had a block comes back unchanged.
pub fn without_meeting_block(description: &str, join_url: &str) -> String {
    if join_url.is_empty() || !description.contains(join_url) {
        return description.to_string();
    }
    let mut kept: Vec<&str> = Vec::new();
    let mut lines = description.lines().peekable();
    while let Some(line) = lines.next() {
        if !line.contains(join_url) {
            kept.push(line);
            continue;
        }
        // The URL's line goes, and so do the labelled detail lines that follow
        // it — but only while they LOOK like details, so a paragraph the user
        // wrote underneath survives.
        while lines.peek().is_some_and(|next| is_detail_line(next)) {
            lines.next();
        }
    }
    // Collapse the blank run the removal can leave behind, and trim the ends,
    // so removing a block from an otherwise empty description gives an empty
    // description rather than a stack of newlines.
    let mut out = String::new();
    let mut blank_run = 0usize;
    for line in kept {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim().to_string()
}

/// A `Label: value` line, the shape invitations use for meeting details.
///
/// Deliberately narrow: a non-empty label short enough to be a label rather
/// than a sentence that happens to contain a colon, and a non-empty value.
///
/// The VALUE may be a link. It used to be barred from containing `://`, back
/// when a block was a join line and a password and any URL below it was
/// therefore the user's own writing. A real invitation is not that: a line
/// pointing at the provider's list of global dial-in numbers is part of the
/// block, and rejecting it here would strand everything below it when the
/// meeting is removed. What still stops the walk is a blank line, which is what
/// separates the block from whatever the user wrote — and the block is always
/// appended after one.
///
/// The LABEL still must not look like a URL, so a bare link on its own line is
/// never mistaken for `scheme: //rest`.
fn is_detail_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Some((label, value)) = trimmed.split_once(':') else {
        return false;
    };
    !label.trim().is_empty()
        && label.chars().count() <= 40
        && !label.contains("://")
        // A bare link on its own line splits into `https` + `//host/…`, which
        // would otherwise read as a perfectly good label and value.
        && !value.starts_with("//")
        && !value.trim().is_empty()
}

pub fn extract_urls(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < text.len() {
        let rest = &text[i..];
        let Some(rel) = rest.find("http") else { break };
        let start = i + rel;
        let tail = &text[start..];
        if !(tail.starts_with("http://") || tail.starts_with("https://")) {
            i = start + 4;
            continue;
        }
        // A URL ends at the first character that cannot be in one.
        let mut end = start;
        for (offset, ch) in tail.char_indices() {
            if ch.is_whitespace() || ch == '<' || ch == '>' || ch == '"' || ch == '\'' {
                break;
            }
            end = start + offset + ch.len_utf8();
        }
        let mut url = &text[start..end];
        // Trailing punctuation belongs to the sentence, not the link. Brackets
        // only count as trailing when they are unbalanced, so a URL that
        // legitimately ends in `)` survives.
        loop {
            let trimmed = match url.chars().last() {
                Some(c @ ('.' | ',' | ';' | ':' | '!' | '?')) => &url[..url.len() - c.len_utf8()],
                Some(')') if url.matches('(').count() < url.matches(')').count() => {
                    &url[..url.len() - 1]
                }
                Some(']') if url.matches('[').count() < url.matches(']').count() => {
                    &url[..url.len() - 1]
                }
                _ => break,
            };
            url = trimmed;
        }
        // `<https://…>` percent-encodes its closing bracket in some mailers.
        let url = url
            .strip_suffix("%3E")
            .or_else(|| url.strip_suffix("%3e"))
            .unwrap_or(url);
        if url.len() > "https://".len() {
            out.push(url.to_string());
        }
        i = end.max(start + 1);
        let _ = bytes;
    }
    out
}

/// Decide which service a URL belongs to, or `None` when it is not a join link
/// at all.
///
/// The exclusions matter as much as the matches. Webex's dial-in help page
/// lives on the same host and carries the same `MTID` query parameter as a real
/// join link, so matching on `MTID` alone offers a "Join" button that opens a
/// page of telephone numbers.
pub fn classify(url: &str) -> Option<ConferenceProvider> {
    let lower = url.to_ascii_lowercase();
    let host = host_of(&lower)?;

    if host.ends_with("webex.com") || host.ends_with("webex.com.cn") {
        // Not joinable: the dial-in listing, the recording player, and the bare
        // site root that some invitations put in the location field.
        if lower.contains("globalcallin.php")
            || lower.contains("/recordingservice/")
            || lower.contains("/playback/")
        {
            return None;
        }
        let joinable = lower.contains("j.php?")
            || lower.contains("/meet/")
            || lower.contains("/join/")
            || lower.contains("/wbxmjs/joinservice/");
        return joinable.then_some(ConferenceProvider::Webex);
    }
    if host.ends_with("teams.microsoft.com") || host.ends_with("teams.live.com") {
        return lower
            .contains("/l/meetup-join/")
            .then_some(ConferenceProvider::Teams);
    }
    if host.ends_with("zoom.us") || host.ends_with("zoom.com") || host.ends_with("zoomgov.com") {
        return (lower.contains("/j/") || lower.contains("/my/") || lower.contains("/w/"))
            .then_some(ConferenceProvider::Zoom);
    }
    // DFNconf (the German research network's Pexip service). The join links live
    // on the bare host under /webapp/; the DOCUMENTATION lives on
    // www.conf.dfn.de, so an exact host match is what keeps a link to the manual
    // from offering a Join button. A conference id is required too — `/webapp/`
    // on its own is the app's landing page and joins nothing.
    //
    // Only the Pexip half. DFNconf also still runs Adobe Connect on
    // webconf.vc.dfn.de, but its meeting URLs have no stable path marker, so
    // matching that host would offer Join for its login page as readily as for
    // a meeting — the same trap the Webex exclusions above exist for.
    if host == "conf.dfn.de" {
        return (lower.contains("conference=") || lower.contains("/conference/"))
            .then_some(ConferenceProvider::Dfnconf);
    }
    if host == "meet.google.com" {
        return Some(ConferenceProvider::GoogleMeet);
    }
    if host == "meet.jit.si" || host.ends_with(".jit.si") {
        return Some(ConferenceProvider::Jitsi);
    }
    if host.ends_with("gotomeeting.com") || host.ends_with("goto.com") {
        return lower
            .contains("/join/")
            .then_some(ConferenceProvider::GoToMeeting);
    }
    if host.ends_with("whereby.com") {
        return Some(ConferenceProvider::Whereby);
    }
    // BigBlueButton is self-hosted, so the host tells us nothing — the join
    // path is the only stable marker.
    if lower.contains("/bigbluebutton/api/join") || lower.contains("/b/") && lower.contains("bbb") {
        return Some(ConferenceProvider::BigBlueButton);
    }
    None
}

fn host_of(lower_url: &str) -> Option<&str> {
    let after_scheme = lower_url.split_once("://")?.1;
    let host = after_scheme
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?;
    // Drop the port.
    Some(host.split(':').next().unwrap_or(host))
}

/// How many `label: value` pairs are worth carrying. A conferencing block has
/// a handful; more than this means the scan wandered into the body text.
const MAX_LABELLED: usize = 6;
/// Bounds that keep an ordinary sentence containing a colon from looking like a
/// labelled field.
const MAX_LABEL_CHARS: usize = 40;
const MAX_VALUE_CHARS: usize = 60;

/// Read the `label: value` lines that follow the join link.
///
/// Deliberately knows no labels. It takes the text from the join link onward —
/// which is where every invitation puts these — and reads each line that has
/// the shape, keeping the label exactly as written. The bounds are what stop it
/// from harvesting prose: a real label is short, a real value is short and on
/// the same line, and a line whose value is a URL is the link itself rather
/// than a detail.
fn labelled_lines_near(description: &str, join_url: &str) -> Vec<ConferenceDetail> {
    let from = description
        .find(join_url)
        .map(|at| at + join_url.len())
        .unwrap_or(0);
    let mut out: Vec<ConferenceDetail> = Vec::new();
    for line in description[from..].lines() {
        if out.len() >= MAX_LABELLED {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        let label = label.trim();
        let value = value.trim();
        if label.is_empty()
            || value.is_empty()
            || label.chars().count() > MAX_LABEL_CHARS
            || value.chars().count() > MAX_VALUE_CHARS
            // A URL is the link, not a detail — and `https` would otherwise
            // read as a label with the rest of the address as its value.
            || value.starts_with("//")
            || value.contains("://")
        {
            continue;
        }
        out.push(ConferenceDetail {
            label: label.to_string(),
            value: value.to_string(),
        });
    }
    out
}

#[derive(Default)]
struct Details {
    meeting_number: Option<String>,
    password: Option<String>,
    sip_address: Option<String>,
    phone: Option<String>,
}

/// Recover the join details WITHOUT reading prose.
///
/// Two machine-readable carriers, identical in every language:
///
/// * a SIP address, `sip:25503113955@example.webex.com`, whose user part is the
///   meeting number;
/// * the DTMF sequence in a `tel:` link,
///   `tel:+1-555-0100,,*01*25503113955%23626114%23*01*`, which is the meeting
///   number and then the numeric password, each terminated by an encoded `#`.
fn extract_details(texts: &[&str]) -> Details {
    let mut details = Details::default();
    for text in texts {
        if details.sip_address.is_none() {
            if let Some(sip) = find_uri(text, "sip:") {
                // ONE scheme, and matched without regard to case. Both halves
                // of that were wrong before, in opposite directions:
                //
                // `trim_start_matches` strips the prefix REPEATEDLY, so a
                // malformed `sip:sip:1234@host` was quietly cleaned up into a
                // meeting number. That is guessing at a URI nobody wrote, and
                // it was an accident of the combinator rather than a decision.
                //
                // And it matched case-SENSITIVELY, so `SIP:1234@host` — which
                // RFC 3986 §3.1 says is the same URI, schemes being
                // case-insensitive — yielded nothing at all.
                let body = sip
                    .get(.."sip:".len())
                    .filter(|prefix| prefix.eq_ignore_ascii_case("sip:"))
                    .map_or(sip.as_str(), |_| &sip["sip:".len()..]);
                if let Some((user, _)) = body.split_once('@') {
                    if !user.is_empty() && user.chars().all(|c| c.is_ascii_digit()) {
                        details.meeting_number = Some(user.to_string());
                    }
                }
                details.sip_address = Some(sip);
            }
        }
        if details.phone.is_none() {
            if let Some(tel) = find_uri(text, "tel:") {
                let (number, password) = parse_dtmf(&tel);
                if details.meeting_number.is_none() {
                    details.meeting_number = number;
                }
                if details.password.is_none() {
                    details.password = password;
                }
                details.phone = Some(tel);
            }
        }
    }
    details
}

fn find_uri(text: &str, scheme: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find(scheme)?;
    let tail = &text[at..];
    let mut end = tail.len();
    for (offset, ch) in tail.char_indices() {
        if ch.is_whitespace() || ch == '<' || ch == '>' || ch == '"' || ch == '\'' {
            end = offset;
            break;
        }
    }
    let uri = tail[..end].trim_end_matches(['.', ',', ';', ':', ')']);
    (uri.len() > scheme.len()).then(|| uri.to_string())
}

/// Split the `*01*<number>#<password>#*01*` DTMF payload of a dial-in link.
fn parse_dtmf(tel: &str) -> (Option<String>, Option<String>) {
    let decoded = tel
        .replace("%23", "#")
        .replace("%2A", "*")
        .replace("%2a", "*");
    let Some(payload) = decoded.split(",,").nth(1) else {
        return (None, None);
    };
    let digits: Vec<String> = payload
        .split('#')
        .filter_map(|part| {
            // The LONGEST digit run in the part, not the first: the field is
            // wrapped in a `*01*` tone marker, and taking the first run would
            // stop at "01" and never reach the number behind it.
            //
            // On a TIE the FIRST maximal run wins, and the fold says so rather
            // than leaving it to a combinator. `max_by_key` returns the LAST
            // maximal element, which is not wrong so much as unstated — and it
            // was the opposite of what the TypeScript twin did, so a payload
            // with two equally long runs in one field announced a different
            // meeting number depending on which surface a person was reading.
            // First-wins is the answer that was already shipping.
            part.split(|c: char| !c.is_ascii_digit())
                .filter(|run| !run.is_empty())
                .fold(None::<&str>, |best, run| match best {
                    Some(b) if b.len() >= run.len() => Some(b),
                    _ => Some(run),
                })
                .map(str::to_string)
        })
        .collect();
    // The tone markers contribute short runs; a real meeting number and a real
    // password are both longer, so length is what tells them apart.
    let mut useful = digits.into_iter().filter(|d| d.len() >= 4);
    (useful.next(), useful.next())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn from_description(text: &str) -> Option<ConferenceLink> {
        detect_conference(&ConferenceSources {
            description: Some(text),
            ..Default::default()
        })
    }

    #[test]
    fn a_webex_join_link_is_found_in_a_description() {
        let found = from_description(
            "Bitte tritt dem Meeting bei: https://example.webex.com/example/j.php?MTID=m1234567890abcdef",
        )
        .expect("detected");
        assert_eq!(found.provider, ConferenceProvider::Webex);
        assert_eq!(found.source, ConferenceSource::Description);
        assert!(found.join_url.ends_with("m1234567890abcdef"));
    }

    #[test]
    fn detection_does_not_depend_on_the_language_of_the_invitation() {
        // The whole point. Same link, four languages of surrounding prose.
        let url = "https://example.webex.com/example/j.php?MTID=mabc";
        for prose in [
            format!("Join meeting: {url}"),
            format!("Meeting beitreten: {url}"),
            format!("Rejoindre la réunion : {url}"),
            format!("会議に参加する: {url}"),
        ] {
            let found = from_description(&prose).expect("detected");
            assert_eq!(found.join_url, url, "failed for {prose}");
        }
    }

    #[test]
    fn the_location_field_is_a_source_because_outlook_uses_it() {
        let found = detect_conference(&ConferenceSources {
            location: Some("https://example.webex.com/example/j.php?MTID=mxyz"),
            ..Default::default()
        })
        .expect("detected");
        assert_eq!(found.source, ConferenceSource::Location);
    }

    #[test]
    fn a_provider_field_outranks_everything_scraped() {
        let found = detect_conference(&ConferenceSources {
            provider_field: Some("https://meet.google.com/abc-defg-hij"),
            location: Some("https://example.webex.com/example/j.php?MTID=m1"),
            description: Some("https://example.zoom.us/j/123"),
            ..Default::default()
        })
        .expect("detected");
        assert_eq!(found.provider, ConferenceProvider::GoogleMeet);
        assert_eq!(found.source, ConferenceSource::ProviderField);
    }

    #[test]
    fn the_bare_site_root_is_not_a_join_link() {
        // A real Webex invitation was observed with only this in its location.
        // Offering a Join button for it would open a page that joins nothing.
        assert!(detect_conference(&ConferenceSources {
            location: Some("https://example.webex.com"),
            ..Default::default()
        })
        .is_none());
    }

    #[test]
    fn the_dial_in_page_is_not_a_join_link_even_though_it_carries_an_mtid() {
        // Same host, same MTID parameter, entirely different page — this is the
        // false positive that matching on MTID alone would produce.
        assert!(from_description(
            "Global call-in numbers: https://example.webex.com/example/globalcallin.php?MTID=m99"
        )
        .is_none());
    }

    #[test]
    fn a_recording_link_is_not_a_meeting() {
        assert!(from_description(
            "Aufzeichnung: https://example.webex.com/recordingservice/sites/example/recording/abc"
        )
        .is_none());
    }

    #[test]
    fn trailing_punctuation_and_brackets_never_end_up_in_the_url() {
        // Each of these produced a dead link in a shipped client at some point.
        for (text, want) in [
            (
                "Join at https://example.webex.com/e/j.php?MTID=m1.",
                "https://example.webex.com/e/j.php?MTID=m1",
            ),
            (
                "Join at <https://example.webex.com/e/j.php?MTID=m2>",
                "https://example.webex.com/e/j.php?MTID=m2",
            ),
            (
                "Join at https://example.webex.com/e/j.php?MTID=m3%3E",
                "https://example.webex.com/e/j.php?MTID=m3",
            ),
            (
                "(see https://example.webex.com/e/j.php?MTID=m4)",
                "https://example.webex.com/e/j.php?MTID=m4",
            ),
        ] {
            assert_eq!(from_description(text).expect("detected").join_url, want);
        }
    }

    #[test]
    fn the_longest_url_in_a_source_wins() {
        // Webex's newer join link nests a shorter-looking URL in its query
        // string; taking the first match hands back something that joins
        // nothing.
        let long = "https://example.webex.com/wbxmjs/joinservice/sites/example/meeting/download/abc?siteurl=example&MTID=m1";
        let found = from_description(&format!(
            "Alt: https://example.webex.com/e/j.php?MTID=m0 Neu: {long}"
        ))
        .expect("detected");
        assert_eq!(found.join_url, long);
    }

    #[test]
    fn the_meeting_number_and_password_come_out_of_the_dtmf_not_the_prose() {
        // The carrier that no translation touches.
        let found = from_description(
            "Beitreten: https://example.webex.com/e/j.php?MTID=m1\n\
             Einwahl: tel:+49-555-0100,,*01*25503113955%23626114%23*01*",
        )
        .expect("detected");
        assert_eq!(found.meeting_number.as_deref(), Some("25503113955"));
        assert_eq!(found.password.as_deref(), Some("626114"));
        assert!(found.phone.is_some());
    }

    #[test]
    fn the_meeting_number_also_comes_out_of_a_sip_address() {
        let found = from_description(
            "Video: sip:25503113955@example.webex.com\n\
             Link: https://example.webex.com/e/j.php?MTID=m1",
        )
        .expect("detected");
        assert_eq!(found.meeting_number.as_deref(), Some("25503113955"));
        assert_eq!(
            found.sip_address.as_deref(),
            Some("sip:25503113955@example.webex.com")
        );
    }

    #[test]
    fn details_are_gathered_across_fields_not_only_where_the_link_was() {
        // The link is usually in the location and the dial-in in the body.
        let found = detect_conference(&ConferenceSources {
            location: Some("https://example.webex.com/e/j.php?MTID=m1"),
            description: Some("tel:+49-555-0100,,*01*25503113955%23626114%23*01*"),
            ..Default::default()
        })
        .expect("detected");
        assert_eq!(found.source, ConferenceSource::Location);
        assert_eq!(found.password.as_deref(), Some("626114"));
    }

    #[test]
    fn the_other_providers_are_recognised_too() {
        for (url, provider) in [
            (
                "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc",
                ConferenceProvider::Teams,
            ),
            (
                "https://example.zoom.us/j/123456789",
                ConferenceProvider::Zoom,
            ),
            (
                "https://meet.google.com/abc-defg-hij",
                ConferenceProvider::GoogleMeet,
            ),
            ("https://meet.jit.si/AperioTest", ConferenceProvider::Jitsi),
            ("https://whereby.com/aperio", ConferenceProvider::Whereby),
        ] {
            let found = from_description(&format!("Link: {url}")).expect(url);
            assert_eq!(found.provider, provider, "for {url}");
        }
    }

    /// A real German Webex invitation, as Exchange delivers it.
    ///
    /// Structure, wording and escaping are verbatim from a captured
    /// `text/calendar` part (`PRODID:Microsoft Exchange Server 2010`); the
    /// names, the site, the MTID, the meeting id and the password are invented.
    /// What it pins down, and what made it worth capturing:
    ///
    /// * `LOCATION;LANGUAGE=de-DE:` was **empty** — for this Exchange-plus-Webex
    ///   path the location carries nothing, so a detector that only looked
    ///   there would find no meeting at all;
    /// * the invitation has **no** `CONFERENCE`, no `X-WEBEX-*` and no
    ///   `X-MICROSOFT-SKYPETEAMS*` — only `X-MICROSOFT-CDO-*`, which say
    ///   nothing about conferencing;
    /// * it has **no** `tel:` and **no** `sip:`, so the machine-readable detail
    ///   carriers are absent and the id and password exist only behind German
    ///   labels — with an alphanumeric password, which no digit heuristic
    ///   would have found either.
    const GERMAN_EXCHANGE_INVITATION: &str = "                Hallo Leonie,\n\
        \n\
        wie vereinbart, hier der Regeltermin für die Abstimmung deiner Bachelorarbeit.\n\
        \n\
        Bis dahin!\n\
        \n\
        Mit lieben Grüßen.\n\
        \n\
        Toni\n\
        ________________________________\n\
        Nehmen Sie an dieser Videokonferenz teil via \
        https://example.webex.com/example/j.php?MTID=m0123456789abcdef0123456789abcdef\n\
        \n\
        Besprechungs-ID: 27401156686\n\
        Passwort: PteT3RSYi92\n";

    #[test]
    fn the_real_german_invitation_is_recognised_with_an_empty_location() {
        let found = detect_conference(&ConferenceSources {
            // Exactly as Exchange delivered it: present, and empty.
            location: Some(""),
            description: Some(GERMAN_EXCHANGE_INVITATION),
            ..Default::default()
        })
        .expect("the join link must be found");

        assert_eq!(found.provider, ConferenceProvider::Webex);
        assert_eq!(found.source, ConferenceSource::Description);
        assert_eq!(
            found.join_url,
            "https://example.webex.com/example/j.php?MTID=m0123456789abcdef0123456789abcdef"
        );
    }

    #[test]
    fn the_real_invitations_details_survive_without_knowing_any_german() {
        let found = detect_conference(&ConferenceSources {
            description: Some(GERMAN_EXCHANGE_INVITATION),
            ..Default::default()
        })
        .expect("detected");

        // No tel:, no sip: — so the machine-readable path finds nothing, and
        // saying otherwise would be the claim this fixture disproves.
        assert!(found.meeting_number.is_none());
        assert!(found.password.is_none());
        assert!(found.phone.is_none() && found.sip_address.is_none());

        // The labels carry them instead, verbatim and in the sender's language.
        assert_eq!(
            found.labelled_details,
            vec![
                ConferenceDetail {
                    label: "Besprechungs-ID".to_string(),
                    value: "27401156686".to_string(),
                },
                ConferenceDetail {
                    label: "Passwort".to_string(),
                    value: "PteT3RSYi92".to_string(),
                },
            ]
        );
    }

    #[test]
    fn the_prose_above_the_link_is_not_harvested_as_details() {
        // The greeting, the sign-off and the separator rule all precede the
        // link; only the block after it is a conferencing block.
        let found = detect_conference(&ConferenceSources {
            description: Some(GERMAN_EXCHANGE_INVITATION),
            ..Default::default()
        })
        .expect("detected");
        assert!(
            found
                .labelled_details
                .iter()
                .all(|d| d.label != "Hallo Leonie"),
            "picked up prose: {:?}",
            found.labelled_details
        );
        assert_eq!(found.labelled_details.len(), 2);
    }

    #[test]
    fn an_english_invitation_yields_english_labels_from_the_same_code() {
        // The point of treating the label as data: no branch, no dictionary.
        let found = from_description(
            "Join the meeting: https://example.webex.com/e/j.php?MTID=m1\n\
             \n\
             Meeting number (access code): 2550 311 3955\n\
             Meeting password: ocn114\n",
        )
        .expect("detected");
        assert_eq!(
            found.labelled_details,
            vec![
                ConferenceDetail {
                    label: "Meeting number (access code)".to_string(),
                    value: "2550 311 3955".to_string(),
                },
                ConferenceDetail {
                    label: "Meeting password".to_string(),
                    value: "ocn114".to_string(),
                },
            ]
        );
    }

    #[test]
    fn an_ordinary_link_is_not_mistaken_for_a_meeting() {
        for text in [
            "Agenda: https://example.com/agenda.pdf",
            "https://github.com/Timtam/aperio",
            "no links at all",
            "",
        ] {
            assert!(from_description(text).is_none(), "false positive: {text}");
        }
    }

    #[test]
    fn a_teams_link_that_is_not_a_meeting_join_is_ignored() {
        assert!(from_description("https://teams.microsoft.com/l/channel/19%3aabc").is_none());
    }

    #[test]
    fn urls_survive_multibyte_text_around_them() {
        let found =
            from_description("Grüße — beitreten: https://example.webex.com/e/j.php?MTID=mü1 …")
                .expect("detected");
        assert!(found.join_url.starts_with("https://example.webex.com"));
    }

    // ── meeting_block / without_meeting_block ───────────────────────────────

    /// The old two-line block, so the removal tests below keep asserting what
    /// they were written to assert. A real block is longer now — the join line,
    /// a meeting number, two passwords, a dial-in number per country — and the
    /// tests that care about THAT are the ones further down.
    fn meeting_block(url: &str, password: Option<&str>) -> String {
        let mut lines = vec![("Join the meeting".to_string(), url.to_string())];
        if let Some(p) = password.map(str::trim).filter(|p| !p.is_empty()) {
            lines.push(("Meeting password".to_string(), p.to_string()));
        }
        super::meeting_block(&lines)
    }

    #[test]
    fn the_block_is_readable_by_the_detector_that_reads_everyone_elses() {
        // The whole point: what Aperio writes has to come back out through the
        // same path an Outlook or eM Client invitation does.
        let url = "https://example.webex.com/example/j.php?MTID=mabc";
        let text = meeting_block(url, Some("s3cr3t"));
        let found = detect_conference(&ConferenceSources {
            description: Some(&text),
            ..Default::default()
        })
        .expect("its own block must be detectable");
        assert_eq!(found.join_url, url);
        assert!(found.labelled_details.iter().any(|d| d.value == "s3cr3t"));
    }

    #[test]
    fn a_meeting_without_a_password_gets_no_password_line() {
        let text = meeting_block("https://example.webex.com/e/j.php?MTID=m1", None);
        assert!(!text.contains("password"), "{text}");
        // An empty string is the same as none — a provider that returns "" must
        // not produce a line inviting the user to type nothing.
        assert_eq!(
            meeting_block("https://example.webex.com/e/j.php?MTID=m1", Some("  ")),
            text
        );
    }

    #[test]
    fn removing_the_block_leaves_the_users_own_text_alone() {
        let url = "https://example.webex.com/example/j.php?MTID=mabc";
        let user_text = "Bring the Q3 numbers.\n\nAgenda:\n- budget\n- hiring";
        let combined = format!("{user_text}\n\n{}", meeting_block(url, Some("s3cr3t")));
        assert_eq!(without_meeting_block(&combined, url), user_text);
    }

    #[test]
    fn removing_a_block_that_is_the_whole_description_empties_it() {
        let url = "https://example.webex.com/e/j.php?MTID=m1";
        let only = meeting_block(url, Some("pw"));
        assert_eq!(without_meeting_block(&only, url), "");
    }

    #[test]
    fn a_description_that_never_had_a_block_is_returned_unchanged() {
        let text = "Just a note.\nWith two lines.";
        assert_eq!(
            without_meeting_block(text, "https://example.webex.com/e/j.php?MTID=m1"),
            text
        );
        // And an empty URL must never match everything.
        assert_eq!(without_meeting_block(text, ""), text);
    }

    #[test]
    fn text_written_below_the_block_survives() {
        // The detail-line sweep must stop at a real sentence, or removing a
        // meeting would eat whatever the user wrote underneath it.
        let url = "https://example.webex.com/e/j.php?MTID=m1";
        let combined = format!(
            "{}\nPlease dial in five minutes early.",
            meeting_block(url, Some("pw"))
        );
        assert_eq!(
            without_meeting_block(&combined, url),
            "Please dial in five minutes early."
        );
    }

    #[test]
    fn a_block_written_by_another_client_is_removed_too() {
        // Aperio did not write this one — a colleague's Outlook did — but the
        // URL is the same meeting, and matching on the URL rather than on the
        // wording is what makes that work.
        let url = "https://example.webex.com/example/j.php?MTID=mabc";
        let foreign = format!(
            "Nehmen Sie an dieser Videokonferenz teil via {url}\n\
             Besprechungs-ID: 27401156686\n\
             Passwort: PteT3RSYi92\n\
             \n\
             Bis dahin!"
        );
        assert_eq!(without_meeting_block(&foreign, url), "Bis dahin!");
    }

    #[test]
    fn the_round_trip_is_stable() {
        // Attach, detach, attach again: a description must not accumulate.
        let url = "https://example.webex.com/e/j.php?MTID=m1";
        let user_text = "Notes.";
        let once = format!("{user_text}\n\n{}", meeting_block(url, Some("pw")));
        let stripped = without_meeting_block(&once, url);
        let twice = format!("{stripped}\n\n{}", meeting_block(url, Some("pw")));
        assert_eq!(once, twice);
    }

    #[test]
    fn dfnconf_room_links_are_recognised() {
        assert_eq!(
            classify("https://conf.dfn.de/webapp/#/?conference=97912345"),
            Some(ConferenceProvider::Dfnconf)
        );
        assert_eq!(
            classify("https://conf.dfn.de/webapp/conference/97912345"),
            Some(ConferenceProvider::Dfnconf)
        );
    }

    #[test]
    fn dfnconf_documentation_is_not_a_join_link() {
        // The manual lives on www.conf.dfn.de and the app on conf.dfn.de. A
        // colleague pasting "see the DFNconf instructions" into an invitation
        // must not produce a Join button that opens a help page.
        assert_eq!(
            classify("https://www.conf.dfn.de/dfnconf/anleitungen-und-dokumentation/"),
            None
        );
        // `/webapp/` with no conference is the landing page.
        assert_eq!(classify("https://conf.dfn.de/webapp/"), None);
        assert_eq!(classify("https://conf.dfn.de/"), None);
    }

    #[test]
    fn dfnconf_adobe_connect_half_is_left_alone() {
        // DFNconf still runs Adobe Connect on its own host, but its meeting
        // URLs carry no stable path marker — matching the host would offer Join
        // for the login page too. Deliberately unclassified rather than
        // wrongly claimed.
        assert_eq!(classify("https://webconf.vc.dfn.de/r/abc123/"), None);
    }
}

/// The contract this rule answers, as a table rather than as code.
///
/// The rule was written TWICE — here and in `shared/conferencing.ts` — and
/// nothing pinned the two to each other: each had its own hand-copied test
/// list, and six places where they disagreed sat there unobserved because the
/// two Rust callers read only `join_url` and every disagreement lived in the
/// other fields. All six would have appeared at once, silently, the moment the
/// TypeScript was deleted.
///
/// The fixture records each input, the answer this code gives, and — for the
/// rows that had two answers — what the TypeScript said and why the surviving
/// answer was chosen. It is a table a person can read, which is the point: the
/// migration's behaviour change is written down BEFORE it happens instead of
/// being discovered afterwards.
///
/// The file lives inside this crate rather than in `shared/contracts/`, where
/// its two siblings sit. Those are read from `host-core`, which never leaves
/// this repository; `cal-core` is compiled by twelve adapter repositories, and
/// an `include_str!` reaching out of the crate would break the day it moves.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/conferencing.json"
    ));

    fn opt<'a>(case: &'a Value, key: &str) -> Option<&'a str> {
        case.get(key).and_then(Value::as_str)
    }

    /// What the fixture says a field should be, as the detected link spells it.
    fn expected<'a>(expect: &'a Value, key: &str) -> Option<&'a str> {
        expect.get(key).and_then(Value::as_str)
    }

    #[test]
    fn every_case_in_the_contract_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["cases"].as_array().expect("cases is an array");

        // Anti-silence: the file must actually carry cases, and it must carry
        // the ones this test exists for. Named rather than counted — a floor
        // would be the number of rows there are today, and adding a row is
        // precisely the change it should survive.
        for must in [
            "webex-dtmf-canonical",
            "dtmf-tie-in-the-meeting-number",
            "sip-scheme-written-twice",
            "sip-scheme-in-capitals",
            "turkish-dotted-capital-i-before-a-uri",
            "no-meeting-at-all",
        ] {
            assert!(
                cases.iter().any(|c| c["name"] == must),
                "the contract has no case named {must} — either it was removed \
                 deliberately or this test is reading the wrong file",
            );
        }

        for case in cases {
            let name = case["name"].as_str().expect("every case is named");
            let found = detect_conference(&ConferenceSources {
                location: opt(case, "location"),
                description: opt(case, "description"),
                ..Default::default()
            });

            let expect = &case["expect"];
            if expect.is_null() {
                assert!(
                    found.is_none(),
                    "{name}: the contract says no meeting, but one was detected: {found:?}",
                );
                continue;
            }
            let Some(found) = found else {
                panic!("{name}: the contract expects a meeting and none was detected");
            };

            assert_eq!(
                found.join_url,
                expect["join_url"]
                    .as_str()
                    .expect("every meeting has a join url"),
                "{name}: join url",
            );
            assert_eq!(
                serde_json::to_value(found.provider).expect("a provider serialises"),
                expect["provider"],
                "{name}: provider",
            );
            assert_eq!(
                serde_json::to_value(found.source).expect("a source serialises"),
                expect["source"],
                "{name}: source",
            );

            // Absent from the fixture means absent from the answer. Spelling it
            // that way round is what makes a row that gains a field fail here
            // rather than pass in silence.
            for (key, actual) in [
                ("meeting_number", &found.meeting_number),
                ("password", &found.password),
                ("sip_address", &found.sip_address),
                ("phone", &found.phone),
            ] {
                assert_eq!(actual.as_deref(), expected(expect, key), "{name}: {key}",);
            }

            let want: Vec<ConferenceDetail> = expect
                .get("labelled_details")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .map(|row| ConferenceDetail {
                            label: row["label"].as_str().unwrap_or_default().to_string(),
                            value: row["value"].as_str().unwrap_or_default().to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(found.labelled_details, want, "{name}: labelled details");
        }
    }

    /// Every row that records a second answer says which one survived and why.
    ///
    /// Not decoration. A row whose `wasTypeScript` block loses its `chosen`
    /// note becomes a behaviour change with no reason attached, which is the
    /// state this whole exercise exists to get out of.
    #[test]
    fn every_divergence_says_why_it_was_settled() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let mut settled = Vec::new();
        for case in doc["cases"].as_array().expect("cases is an array") {
            let Some(was) = case.get("wasTypeScript") else {
                continue;
            };
            let name = case["name"].as_str().unwrap_or("<unnamed>");
            for key in ["answer", "chosen"] {
                assert!(
                    was.get(key)
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty()),
                    "{name}: its wasTypeScript block has no `{key}`",
                );
            }
            settled.push(name);
        }
        assert!(
            settled.len() >= 6,
            "only {} divergences are recorded; six were found, so rows have been \
             dropped: {settled:?}",
            settled.len(),
        );
    }
}
