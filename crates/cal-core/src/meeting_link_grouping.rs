//! Grouping a provider's meeting with the appointment it belongs to
//! (`DESIGN-event-groups.md`, Stufe 4).
//!
//! A videoconference account contributes a read-only calendar of its own
//! meetings. Most of those meetings also have a calendar entry — Aperio's own,
//! or the invitation Outlook wrote — so they appear twice, and
//! [`crate::without_duplicate_meetings`] drops the meeting row to hide it. That
//! works most of the time, and when it does not, a meeting the user really has
//! simply vanishes with nothing to say so.
//!
//! A group does the same job honestly: both rows stay, folding shows one, the
//! mark says "2×", and a divergence becomes visible instead of being discarded.
//! That last one is the case the filter can never handle: when an appointment
//! is moved and its meeting is not, the join URL still matches, so the filter
//! goes on hiding the meeting exactly when the two have stopped agreeing.
//!
//! # Why this may group by itself when nothing else may
//!
//! Stage 3 refuses to group automatically, and the reason stands: an office
//! full of "Team meeting" at 10:00 would produce groups nobody asked for. That
//! reason is about GUESSING — about treating a resemblance as evidence.
//!
//! A meeting and its calendar entry are not related by resemblance. They carry
//! the same JOIN URL, issued by the provider, and that is an identity. Aperio
//! writes the event's own title into the meeting it creates, so title equality
//! says almost nothing, and the times drift apart precisely when an event is
//! moved — which is when the two most need to be seen as one.
//!
//! Grouping on an identity is a different proposition from grouping on a
//! likeness. Everything here rests on that distinction, so the rules below are
//! strict about staying on the identity's side of it.
//!
//! # Folding two spellings of one link
//!
//! The identity has to survive being written down differently: case in the
//! scheme and host, surrounding space, trailing slashes on the path. That is
//! [`normalize_join_url`], and it uses a real URL parser rather than a
//! hand-rolled fold — because the fold also has to drop a default port, resolve
//! dot segments, and convert an internationalised host to punycode.
//! `münchen.example.com` and `xn--mnchen-3ya.example.com` are ONE host, and a
//! fold that answered otherwise would fail silently: a group that used to be
//! offered simply stops being offered.
//!
//! What it does NOT fold is the query string. A meeting's id and password live
//! there, and two links that differ in the query are two different meetings.

use serde::{Deserialize, Serialize};

use crate::group_suggestion::is_meeting_calendar;
use crate::meeting_events::join_url_of;
use crate::{EventGroup, SuggestionDecline};

/// Fold a join URL to what two spellings of the same link agree on.
///
/// Only what is safe: case in the scheme and host, surrounding space, trailing
/// slashes on the path, a default port, dot segments, and the punycode form of
/// an internationalised host. NOT the query string.
///
/// A string that will not parse comes back trimmed rather than mangled —
/// inventing a normalisation for something that is not a URL would be guessing,
/// and two identical strings still match without any help.
///
/// Every answer here is pinned by `tests/fixtures/normalizeJoinUrl.json`, whose
/// expectations were MEASURED from the TypeScript this replaced rather than
/// chosen.
pub fn normalize_join_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let Ok(parsed) = url::Url::parse(trimmed) else {
        return trimmed.to_string();
    };
    let path = parsed.path().trim_end_matches('/');
    let host = parsed.host_str().unwrap_or_default();
    let port = match parsed.port() {
        // `port()` is already None for the scheme's default, so anything here
        // is a port that genuinely tells the two links apart.
        Some(port) => format!(":{port}"),
        None => String::new(),
    };
    let query = match parsed.query() {
        Some(query) => format!("?{query}"),
        None => String::new(),
    };
    // `scheme()` has no colon; the JavaScript this replaces read `protocol`,
    // which does. Same string either way, spelled out so it stays that way.
    format!("{}://{host}{port}{path}{query}", parsed.scheme())
}

/// The least a row needs for this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct LinkableEvent {
    pub calendar_id: String,
    /// The id this row would be GROUPED under — the series master, resolved by
    /// the caller.
    pub series_id: String,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Whether the row belongs to a recurring series.
    #[serde(default)]
    pub recurs: bool,
}

/// A meeting row and the appointment it belongs to, as positions in the input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct MeetingLinkPair {
    /// The row from the meetings calendar.
    pub meeting: usize,
    /// The ordinary calendar entry for the same meeting.
    pub event: usize,
    /// The join URL both carry — the identity this rests on.
    pub join_url: String,
}

/// Everything [`find_meeting_link_pairs`] needs, in one JSON value.
#[derive(Debug, Clone, Deserialize)]
pub struct MeetingLinkInput {
    pub events: Vec<LinkableEvent>,
    #[serde(default)]
    pub groups: Vec<EventGroup>,
    #[serde(default)]
    pub declines: Vec<SuggestionDecline>,
}

/// One join URL's rows, in the order they arrived.
///
/// Insertion order is part of the answer: the first entry of a bucket is the
/// one a pair names, and the caller's order is the order the user sees. A hash
/// map would have made that depend on nothing at all.
#[derive(Default)]
struct Bucket {
    meetings: Vec<usize>,
    entries: Vec<usize>,
    seen_meetings: std::collections::HashSet<(String, String)>,
    seen_entries: std::collections::HashSet<(String, String)>,
}

fn member_key(event: &LinkableEvent) -> (String, String) {
    (event.calendar_id.clone(), event.series_id.clone())
}

fn pair_key<'a>(
    a: (&'a str, &'a str),
    b: (&'a str, &'a str),
) -> ((&'a str, &'a str), (&'a str, &'a str)) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

/// The (meeting, appointment) pairs that should become groups.
///
/// One window's rows, the groups behind them, and the refusals the user has
/// already made. Only a view has all of a window's rows in hand, which is why
/// the caller supplies them rather than an adapter.
///
/// A link is only acted on when it identifies exactly ONE meeting and exactly
/// ONE appointment. A standing meeting room reused by two unrelated
/// appointments makes the identity ambiguous — and an ambiguous identity is a
/// resemblance again. Then nothing happens: a wrong group is worse than none,
/// because it still looks authoritative.
///
/// # Counting appointments, not rows
///
/// **A series counts once.** A recurring appointment renders one row per day,
/// so a week's rows hold five of it. Counting rows would make every recurring
/// meeting permanently ambiguous — and it would depend on how wide a range
/// happened to be open, which is not a property of the data. Membership is
/// keyed by the series master, so that is what is counted.
///
/// **A group counts once.** Copies of one appointment in several calendars each
/// carry the same join URL — that is what a forwarded invitation does. Counting
/// them separately would refuse exactly the case this feature exists for: an
/// appointment the user has ALREADY declared to be one thing. "Which
/// appointment is this meeting?" has one answer there, and the group is that
/// answer. The meeting joins the group rather than starting a second one.
///
/// A claimant OUTSIDE the group still makes it ambiguous, and still stops
/// everything. The rule is not "ignore the extras", it is "count appointments,
/// not rows".
///
/// # What a refusal does, and what it does not survive
///
/// `declines` is what stops this being a daily nuisance. Taking a member out of
/// a group, or dissolving one, writes exactly those marks, and they sync — so a
/// pair the user has pulled apart on any device stays apart on all of them.
///
/// A refusal is matched as the PAIR it names, against every copy of the
/// appointment there is rather than only the ones in view: ungrouping writes
/// one mark per member the event left at that moment, so a copy added
/// afterwards carries none of its own, and asking the rows on screen would make
/// the refusal invisible whenever that younger copy is the one being drawn.
///
/// A mark stores two ids, and for an UNGROUPED pair neither can be repaired:
/// healing needs a membership row and a signature, and a bare mark has neither.
/// So when a provider re-mints the appointment's id — a bootstrap, a move,
/// Exchange unasked — the mark stops matching and the pair is offered once
/// more. The user refuses once more, and the new mark names the new id.
///
/// Reading the mark one-sidedly, by its stable meeting half, was tried and
/// reverted. The argument for it — every mark that can speak about a pairing
/// names the meeting — is true and useless: the rule needs the converse, that
/// every mark NAMING the meeting speaks about its pairing, and that is false.
/// Ungrouping writes a mark between the departing event and every member it
/// left, so a meeting merely present in that group gets named by a statement
/// about somebody else; and a name-and-time refusal can name a meeting row too.
/// Either would have blacklisted the meeting from ever pairing again, globally
/// and permanently, over a refusal the user never made about it.
///
/// One repeated offer after an id change is the cheaper wrong.
pub fn find_meeting_link_pairs(
    events: &[LinkableEvent],
    groups: &[EventGroup],
    declines: &[SuggestionDecline],
) -> Vec<MeetingLinkPair> {
    let mut grouped: std::collections::HashMap<(String, String), &EventGroup> =
        std::collections::HashMap::new();
    for group in groups {
        for member in &group.members {
            grouped.insert((member.calendar_id.clone(), member.event_id.clone()), group);
        }
    }
    let declined: std::collections::HashSet<((&str, &str), (&str, &str))> = declines
        .iter()
        .filter(|d| d.is_declined())
        .map(|d| {
            pair_key(
                (d.calendar_a.as_str(), d.event_a.as_str()),
                (d.calendar_b.as_str(), d.event_b.as_str()),
            )
        })
        .collect();

    /// Which appointment the row belongs to: its group when it has one, else
    /// itself. This is the identity the counting is about.
    fn appointment_of(
        grouped: &std::collections::HashMap<(String, String), &EventGroup>,
        event: &LinkableEvent,
    ) -> String {
        match grouped.get(&member_key(event)) {
            Some(group) => group.id.clone(),
            None => format!("alone:{}\n{}", event.calendar_id, event.series_id),
        }
    }

    // Insertion-ordered, because the first entry of a bucket is the one a pair
    // names and the caller's order is the order the user sees.
    let mut order: Vec<String> = Vec::new();
    let mut by_url: std::collections::HashMap<String, Bucket> = std::collections::HashMap::new();
    for (i, event) in events.iter().enumerate() {
        let Some(raw) = join_url_of(event.location.as_deref(), event.description.as_deref()) else {
            continue;
        };
        let url = normalize_join_url(&raw);
        if url.is_empty() {
            continue;
        }
        let bucket = by_url.entry(url.clone()).or_insert_with(|| {
            order.push(url.clone());
            Bucket::default()
        });
        let key = member_key(event);
        if is_meeting_calendar(&event.calendar_id) {
            if bucket.seen_meetings.insert(key) {
                bucket.meetings.push(i);
            }
        } else if bucket.seen_entries.insert(key) {
            bucket.entries.push(i);
        }
    }

    let mut out = Vec::new();
    for join_url in order {
        let bucket = &by_url[&join_url];
        if bucket.meetings.len() != 1 {
            continue;
        }
        if bucket.entries.is_empty() {
            continue;
        }
        // One appointment, however many rows say so.
        let appointments: std::collections::HashSet<String> = bucket
            .entries
            .iter()
            .map(|&i| appointment_of(&grouped, &events[i]))
            .collect();
        if appointments.len() != 1 {
            continue;
        }
        // A recurring appointment is left alone, and that is not caution.
        //
        // A group's members are SERIES. A provider that lists a recurring
        // meeting as one row per occurrence (Webex) has no series for one to
        // name, and a provider whose meeting does NOT recur while the
        // appointment does has a meeting that genuinely is not there on most of
        // the days. Either way the group would claim a copy that does not exist
        // on the day being read, on every day but one. The duplicate filter
        // goes on hiding it, exactly as before, and grouping by hand is still
        // there for whoever wants it.
        if bucket.entries.iter().any(|&i| events[i].recurs) {
            continue;
        }
        let meeting = &events[bucket.meetings[0]];
        let event = &events[bucket.entries[0]];
        let meeting_group = grouped.get(&member_key(meeting));
        let appointment_group = grouped.get(&member_key(event));
        // A meeting that is ALREADY in a group is finished business, whatever
        // the entry in front of us is.
        //
        // Same group: nothing to do. Different group: a merge, which only the
        // user can ask for. And — the case this rule exists for — the entry
        // ungrouped while the meeting is grouped: the meeting already belongs
        // to an appointment, so a second one claiming the same link means the
        // link identifies two appointments, which is the ambiguity this whole
        // function refuses. Letting it through would have merged them, and
        // WHICH ones got merged would have depended on nothing but the
        // calendars that happened to be switched on — the count above only ever
        // sees the rows in view.
        if meeting_group.is_some() {
            continue;
        }
        // ONE meeting per account per appointment, and this is load-bearing.
        //
        // A provider may list a recurring meeting as one row per occurrence,
        // each with an id of its own — Webex does, and its list response
        // carries no series id for us to collapse them by. Without this rule
        // the day view would hand over a different meeting id every morning,
        // each one a new member: the group would grow by one a day, forever,
        // and the count on the row would climb with it. An appointment has one
        // meeting per account; once this calendar is represented in the group,
        // the job is done.
        if appointment_group.is_some_and(|group| {
            group
                .members
                .iter()
                .any(|m| m.calendar_id == meeting.calendar_id)
        }) {
            continue;
        }
        // Already refused — as the PAIR the mark names, against every copy of
        // the appointment there is rather than only the ones in view.
        let a = (meeting.calendar_id.as_str(), meeting.series_id.as_str());
        let refused = match appointment_group {
            Some(group) => group.members.iter().any(|m| {
                declined.contains(&pair_key(a, (m.calendar_id.as_str(), m.event_id.as_str())))
            }),
            None => bucket.entries.iter().any(|&i| {
                let entry = &events[i];
                declined.contains(&pair_key(
                    a,
                    (entry.calendar_id.as_str(), entry.series_id.as_str()),
                ))
            }),
        };
        if refused {
            continue;
        }
        out.push(MeetingLinkPair {
            meeting: bucket.meetings[0],
            event: bucket.entries[0],
            join_url,
        });
    }
    out
}

/// Pair from JSON, and answer with JSON.
///
/// The door both frontends come through; the marshalling lives here so the two
/// cannot drift in what they accept.
pub fn find_meeting_link_pairs_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: MeetingLinkInput = serde_json::from_str(input_json)?;
    let found = find_meeting_link_pairs(&input.events, &input.groups, &input.declines);
    serde_json::to_string(&found)
}

/// The fold, against the table MEASURED from the TypeScript it replaced.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::Value;

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/normalizeJoinUrl.json"
    ));

    #[test]
    fn every_case_in_the_contract_holds() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let cases = doc["cases"].as_array().expect("cases is an array");

        // Anti-silence: the file must carry the row the parser is here for.
        // Named rather than counted \u2014 a floor would be today's row count, and
        // adding one is the change it should survive.
        assert!(
            cases
                .iter()
                .any(|c| c["url"].as_str() == Some("https://m\u{fc}nchen.example.com/meet/x")),
            "the contract lost the internationalised-host row, which is the one \
             case a hand-rolled fold cannot answer",
        );

        for case in cases {
            let url = case["url"].as_str().expect("every case has a url");
            let want = case["normalized"]
                .as_str()
                .expect("every case has an answer");
            assert_eq!(
                normalize_join_url(url),
                want,
                "{:?}: {}",
                url,
                case["note"].as_str().unwrap_or(""),
            );
        }
    }

    /// Folding again does not move a link the DETECTOR can produce.
    ///
    /// Full idempotence is neither needed nor true, and pretending otherwise
    /// would be a test about nothing. The fold is applied ONCE, to a URL the
    /// conference detector just returned, and that detector only ever answers
    /// with `http`/`https`. Feed it the one row here that is not a join URL at
    /// all — `mailto:someone@example.com`, which folds to
    /// `mailto://someone@example.com` — and a second pass reads `someone` as
    /// userinfo and drops it. The TypeScript this replaces did exactly the
    /// same; the case is in the table so the shape is recorded rather than
    /// discovered.
    ///
    /// What has to hold is that a bucket key is stable, so this checks the
    /// rows that can actually become one.
    #[test]
    fn folding_a_real_join_url_twice_changes_nothing() {
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let mut checked = 0;
        for case in doc["cases"].as_array().expect("cases is an array") {
            let once = normalize_join_url(case["url"].as_str().unwrap_or(""));
            if !once.starts_with("http://") && !once.starts_with("https://") {
                continue;
            }
            assert_eq!(normalize_join_url(&once), once, "not stable: {once:?}");
            checked += 1;
        }
        assert!(
            checked >= 10,
            "the table stopped holding real join URLs, so this proves nothing",
        );
    }
}
