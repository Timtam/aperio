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
#[serde(rename_all = "snake_case")]
pub enum ConferenceProvider {
    Webex,
    Teams,
    Zoom,
    GoogleMeet,
    Jitsi,
    GoToMeeting,
    BigBlueButton,
    Whereby,
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
            ConferenceProvider::Other => "other",
        }
    }
}

/// Where a link was found. Kept so the UI can be honest about confidence and so
/// a bug report says which field to look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
                    Some((current, _)) => url.len() > current.len(),
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
            return Some(ConferenceLink {
                join_url,
                provider,
                source,
                meeting_number: details.meeting_number,
                password: details.password,
                sip_address: details.sip_address,
                phone: details.phone,
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
                if let Some((user, _)) = sip.trim_start_matches("sip:").split_once('@') {
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
            part.split(|c: char| !c.is_ascii_digit())
                .filter(|run| !run.is_empty())
                .max_by_key(|run| run.len())
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
}
