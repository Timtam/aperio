//! The lines that make a CalDAV event a meeting, on a write.
//!
//! On an RFC 6638 server the organizer's copy of a meeting IS the invitation:
//! the server mails the attendees whenever that copy changes (§3.2.1), and a
//! PUT that drops ORGANIZER turns the meeting into a plain appointment, which
//! the server announces by cancelling it for everyone (§3.2.3.1, §3.2.1.3).
//! Aperio rebuilds a VEVENT from core fields, so it used to drop them on every
//! save without "Notify attendees": an iCloud meeting lost its guests, and
//! they got a cancellation (live round 5, E1).
//!
//! So a write never rebuilds these lines. It reads the server's copy and
//! carries ORGANIZER, ATTENDEE, SEQUENCE and STATUS through as the server's own
//! text, and changes the invitees only as far as the edit changed them
//! ([`cal_core::attendee::invitee_write`]):
//!
//! - the same invitees: every line verbatim (the server mails the change);
//! - invitees changed on the account's own meeting: ORGANIZER and the rows
//!   that stay verbatim, the rows removed dropped (the server cancels for
//!   them), the new ones generated (the server invites them);
//! - every invitee removed, confirmed by the host: no ORGANIZER and no rows,
//!   and the server cancels for everyone (decision 74a);
//! - someone else's meeting: the lines verbatim, and a change to its invitees
//!   is refused, because only the organizer may make one;
//! - a plain event that gets its first invitees: ORGANIZER and the rows, but
//!   only when the server schedules and the user notifies, as before.

use cal_core::attendee::{invitee_write, normalize_address, parse, InviteeWrite};
use cal_core::Event;

use crate::error::{CaldavError, CaldavResult};
use crate::ical_raw::{
    component_lines, fold, insert_after_head, line_ending, param_value, ContentLine,
};
use crate::identity::OwnIdentity;
use crate::mapping::{same_calendar_user, strip_mailto_scheme};

/// What a write knows about the account on this server.
#[derive(Debug, Clone, Default)]
pub struct WriteCtx {
    /// The account's own calendar-user addresses; `None` when the server
    /// could not be asked for them just now.
    pub identity: Option<OwnIdentity>,
    /// The server schedules (RFC 6638).
    pub schedules: bool,
    /// The `mailto:` a new meeting names as its ORGANIZER.
    pub organizer_address: Option<String>,
}

/// What a write does to a meeting's people, to be applied to the other
/// blocks of the same resource too: RFC 6638 wants every component of a
/// meeting to name the same organizer, and a guest removed from a series is
/// removed from its changed occurrences as well.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeopleChange {
    Keep,
    /// Normalised addresses that leave, and entries (`Name <address>` or a
    /// bare address) that join.
    Replace {
        removed: Vec<String>,
        added: Vec<String>,
    },
    Clear,
    /// A plain series gets its first invitees.
    Invite {
        organizer: String,
        added: Vec<String>,
    },
}

/// The planned meeting lines of one VEVENT block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockPlan {
    /// Raw text to insert after the head of the rebuilt block: ORGANIZER,
    /// ATTENDEE, SEQUENCE and STATUS, in the block's own line ending.
    pub lines: String,
    pub change: PeopleChange,
    /// The addresses the block will name as invitees, the way Aperio shows
    /// them.
    pub invitees: Vec<String>,
    /// The block names an ORGANIZER that is not the account.
    pub attendee_copy: bool,
}

/// Plan the meeting lines of `server_block`, the server's current copy of the
/// VEVENT that `edit` rewrites.
pub fn plan_block(server_block: &str, edit: &Event, ctx: &WriteCtx) -> CaldavResult<BlockPlan> {
    let ending = line_ending(server_block);
    let lines = component_lines(server_block);
    let organizer = lines.iter().find(|l| l.name == "ORGANIZER");
    let rows: Vec<&ContentLine> = lines.iter().filter(|l| l.name == "ATTENDEE").collect();
    let mut carried = String::new();
    for line in lines
        .iter()
        .filter(|l| l.name == "SEQUENCE" || l.name == "STATUS")
    {
        carried.push_str(&server_block[line.range.clone()]);
    }
    let verbatim = |kept: &[&ContentLine]| -> String {
        kept.iter()
            .map(|l| &server_block[l.range.clone()])
            .collect()
    };

    let Some(organizer) = organizer else {
        return Ok(plan_plain(server_block, &rows, edit, ctx, ending, carried));
    };

    let identity = ctx.identity.as_ref().ok_or_else(|| {
        CaldavError::Network(
            "the account's own addresses on this server could not be read; nothing was saved"
                .into(),
        )
    })?;
    let own_meeting = identity.names(&organizer.value, organizer.param("EMAIL"));
    // The organizer's row and the account's own row are not invitees (67a).
    let is_host = |row: &ContentLine| {
        same_calendar_user(&organizer.value, &row.value)
            || (own_meeting && identity.names(&row.value, row.param("EMAIL")))
    };
    let guests: Vec<&ContentLine> = rows.iter().copied().filter(|r| !is_host(r)).collect();
    let current: Vec<String> = guests.iter().map(|r| shown_address(r)).collect();
    // On someone else's meeting the account is one of the guests, so its own
    // address stays in the list on both sides.
    let now = if own_meeting {
        edit_invitees(edit, identity)
    } else {
        edit_invitees(edit, &OwnIdentity::default())
    };
    let decision = invitee_write(edit, &now, &current);

    if !own_meeting {
        if decision != InviteeWrite::Keep {
            return Err(CaldavError::Forbidden(
                "reply-only-invitation: only the organizer can change who is invited".into(),
            ));
        }
        let mut text = verbatim(&[organizer]);
        text.push_str(&verbatim(&rows));
        text.push_str(&carried);
        return Ok(BlockPlan {
            lines: text,
            change: PeopleChange::Keep,
            invitees: current,
            attendee_copy: true,
        });
    }

    match decision {
        InviteeWrite::Keep => {
            let mut text = verbatim(&[organizer]);
            text.push_str(&verbatim(&rows));
            text.push_str(&carried);
            Ok(BlockPlan {
                lines: text,
                change: PeopleChange::Keep,
                invitees: current,
                attendee_copy: false,
            })
        }
        InviteeWrite::Clear => Ok(BlockPlan {
            lines: carried,
            change: PeopleChange::Clear,
            invitees: Vec::new(),
            attendee_copy: false,
        }),
        InviteeWrite::Replace => {
            let now_addresses: Vec<String> =
                now.iter().map(|e| normalize_address(&parse(e).1)).collect();
            let kept: Vec<&ContentLine> = rows
                .iter()
                .copied()
                .filter(|r| {
                    is_host(r) || now_addresses.contains(&normalize_address(&shown_address(r)))
                })
                .collect();
            let removed: Vec<String> = current
                .iter()
                .map(|a| normalize_address(a))
                .filter(|a| !now_addresses.contains(a))
                .collect();
            let current_normalised: Vec<String> =
                current.iter().map(|a| normalize_address(a)).collect();
            let added: Vec<String> = now
                .iter()
                .filter(|e| !current_normalised.contains(&normalize_address(&parse(e).1)))
                .cloned()
                .collect();
            let mut text = verbatim(&[organizer]);
            text.push_str(&verbatim(&kept));
            for entry in &added {
                text.push_str(&attendee_line(entry, ending));
            }
            text.push_str(&carried);
            Ok(BlockPlan {
                lines: text,
                change: PeopleChange::Replace { removed, added },
                invitees: now.iter().map(|e| parse(e).1).collect(),
                attendee_copy: false,
            })
        }
    }
}

/// A block without ORGANIZER: a plain appointment, or rows a client stored as
/// data. Its first invitees make it a meeting only when the server schedules
/// and the user notifies.
fn plan_plain(
    server_block: &str,
    rows: &[&ContentLine],
    edit: &Event,
    ctx: &WriteCtx,
    ending: &str,
    carried: String,
) -> BlockPlan {
    let own = ctx.identity.clone().unwrap_or_default();
    let current: Vec<String> = rows.iter().map(|r| shown_address(r)).collect();
    let now = edit_invitees(edit, &own);
    let decision = invitee_write(edit, &now, &current);
    let mut text = String::new();
    let change;
    let invitees;
    match decision {
        InviteeWrite::Keep => {
            for row in rows {
                text.push_str(&server_block[row.range.clone()]);
            }
            change = PeopleChange::Keep;
            invitees = current;
        }
        InviteeWrite::Clear => {
            change = PeopleChange::Clear;
            invitees = Vec::new();
        }
        InviteeWrite::Replace if rows.is_empty() => {
            match ctx.organizer_address.as_deref() {
                Some(organizer) if ctx.schedules && edit.send_invitations => {
                    text.push_str(&fold(&format!("ORGANIZER:{organizer}"), ending));
                    for entry in &now {
                        text.push_str(&attendee_line(entry, ending));
                    }
                    change = PeopleChange::Invite {
                        organizer: organizer.to_string(),
                        added: now.clone(),
                    };
                    invitees = now.iter().map(|e| parse(e).1).collect();
                }
                // Invitees are stored only as a meeting the server schedules
                // (TODO: keep them as data on a server that does not).
                _ => {
                    change = PeopleChange::Keep;
                    invitees = Vec::new();
                }
            }
        }
        InviteeWrite::Replace => {
            let now_addresses: Vec<String> =
                now.iter().map(|e| normalize_address(&parse(e).1)).collect();
            let current_normalised: Vec<String> =
                current.iter().map(|a| normalize_address(a)).collect();
            for row in rows {
                if now_addresses.contains(&normalize_address(&shown_address(row))) {
                    text.push_str(&server_block[row.range.clone()]);
                }
            }
            let added: Vec<String> = now
                .iter()
                .filter(|e| !current_normalised.contains(&normalize_address(&parse(e).1)))
                .cloned()
                .collect();
            for entry in &added {
                text.push_str(&attendee_line(entry, ending));
            }
            let removed = current_normalised
                .into_iter()
                .filter(|a| !now_addresses.contains(a))
                .collect();
            change = PeopleChange::Replace { removed, added };
            invitees = now.iter().map(|e| parse(e).1).collect();
        }
    }
    text.push_str(&carried);
    BlockPlan {
        lines: text,
        change,
        invitees,
        attendee_copy: false,
    }
}

/// `block`, another VEVENT of the same resource (a changed occurrence), with
/// `change` applied to its own meeting lines. Everything else stays as the
/// server wrote it.
pub fn apply_to_block(block: &str, change: &PeopleChange) -> String {
    let lines = component_lines(block);
    let organizer = lines.iter().find(|l| l.name == "ORGANIZER");
    let rows: Vec<&ContentLine> = lines.iter().filter(|l| l.name == "ATTENDEE").collect();
    let ending = line_ending(block);
    match change {
        PeopleChange::Keep => block.to_string(),
        PeopleChange::Clear => {
            let mut drop: Vec<std::ops::Range<usize>> =
                rows.iter().map(|r| r.range.clone()).collect();
            if let Some(organizer) = organizer {
                drop.push(organizer.range.clone());
            }
            crate::ical_raw::without_ranges(block, &drop)
        }
        PeopleChange::Replace { removed, added } => {
            let is_host = |row: &ContentLine| {
                organizer.is_some_and(|o| same_calendar_user(&o.value, &row.value))
            };
            let drop: Vec<std::ops::Range<usize>> = rows
                .iter()
                .filter(|r| !is_host(r) && removed.contains(&normalize_address(&shown_address(r))))
                .map(|r| r.range.clone())
                .collect();
            let present: Vec<String> = rows
                .iter()
                .map(|r| normalize_address(&shown_address(r)))
                .collect();
            let new_rows: String = added
                .iter()
                .filter(|e| !present.contains(&normalize_address(&parse(e).1)))
                .map(|e| attendee_line(e, ending))
                .collect();
            let trimmed = crate::ical_raw::without_ranges(block, &drop);
            insert_after_head(&trimmed, &new_rows)
        }
        PeopleChange::Invite {
            organizer: org,
            added,
        } => {
            if organizer.is_some() {
                return block.to_string();
            }
            let mut text = fold(&format!("ORGANIZER:{org}"), ending);
            for entry in added {
                text.push_str(&attendee_line(entry, ending));
            }
            insert_after_head(block, &text)
        }
    }
}

/// The edit's invitees, without the account's own addresses: the account is
/// the organizer of its own meeting and never its own guest (67a).
fn edit_invitees(edit: &Event, own: &OwnIdentity) -> Vec<String> {
    invitees_of(&edit.attendees, own)
}

/// The entries of `attendees` that name somebody other than the account.
pub fn invitees_of(attendees: &[String], own: &OwnIdentity) -> Vec<String> {
    attendees
        .iter()
        .filter(|entry| {
            let address = parse(entry).1;
            !normalize_address(&address).is_empty() && !own.names(&address, None)
        })
        .cloned()
        .collect()
}

/// The address a row names, the way the read side shows it: a `mailto:`
/// without its scheme, else the EMAIL parameter, else the value.
fn shown_address(row: &ContentLine) -> String {
    if let Some(address) = strip_mailto_scheme(row.value.trim()) {
        return address.trim().to_string();
    }
    row.param("EMAIL")
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| row.value.trim().to_string())
}

/// A new invitee's row: required, not yet answered, asked to answer.
pub fn attendee_line(entry: &str, ending: &str) -> String {
    let (name, address) = parse(entry);
    let mut line = String::from("ATTENDEE");
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        line.push_str(";CN=");
        line.push_str(&param_value(name.trim()));
    }
    line.push_str(";ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:");
    line.push_str(address.trim());
    fold(&line, ending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::tests::{icloud_identity, ICLOUD_MEETING};

    fn master_block() -> &'static str {
        let from = ICLOUD_MEETING.find("BEGIN:VEVENT").unwrap();
        let to = ICLOUD_MEETING.find("END:VEVENT\r\n").unwrap() + "END:VEVENT\r\n".len();
        &ICLOUD_MEETING[from..to]
    }

    fn ctx() -> WriteCtx {
        WriteCtx {
            identity: Some(icloud_identity()),
            schedules: true,
            organizer_address: Some("mailto:toni@example.org".into()),
        }
    }

    fn read() -> Event {
        crate::mapping::parse_calendar_data_as(ICLOUD_MEETING, "cal", None, &icloud_identity())
            .unwrap()
            .remove(0)
    }

    const CHAIR_ROW: &str = "ATTENDEE;CN=Toni Barth;CUTYPE=INDIVIDUAL;PARTSTAT=ACCEPTED;EMAIL=toni@example\r\n .org;ROLE=CHAIR:/aB1/principal/\r\n";
    const BOB_ROW: &str = "ATTENDEE;CN=Bob Guest;CUTYPE=INDIVIDUAL;EMAIL=bob@example.net;SCHEDULE-STATUS=\r\n 1.1:mailto:bob@example.net\r\n";
    const ORGANIZER_LINE: &str =
        "ORGANIZER;CN=Toni Barth;EMAIL=toni@example.org:/aB1/principal/\r\n";

    /// E1: a title edit keeps every meeting line as the server wrote it.
    #[test]
    fn the_same_guests_keep_every_line_byte_for_byte() {
        let mut edit = read();
        edit.title = "Aperio R6 renamed".into();
        let plan = plan_block(master_block(), &edit, &ctx()).unwrap();
        assert_eq!(plan.change, PeopleChange::Keep);
        assert_eq!(
            plan.lines,
            format!("{ORGANIZER_LINE}{CHAIR_ROW}{BOB_ROW}SEQUENCE:0\r\n")
        );
        assert!(!plan.attendee_copy);
    }

    #[test]
    fn a_new_guest_is_added_and_the_others_stay_verbatim() {
        let mut edit = read();
        edit.attendees.push("Doe, Jane <jane@example.net>".into());
        let plan = plan_block(master_block(), &edit, &ctx()).unwrap();
        assert_eq!(
            plan.lines,
            format!(
                "{ORGANIZER_LINE}{CHAIR_ROW}{BOB_ROW}ATTENDEE;CN=\"Doe, Jane\";ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRU\r\n E:mailto:jane@example.net\r\nSEQUENCE:0\r\n"
            )
        );
        assert_eq!(
            plan.change,
            PeopleChange::Replace {
                removed: vec![],
                added: vec!["Doe, Jane <jane@example.net>".into()]
            }
        );
    }

    #[test]
    fn a_removed_guest_loses_only_their_row() {
        let mut edit = read();
        edit.attendees = vec!["carol@example.net".into()];
        let plan = plan_block(master_block(), &edit, &ctx()).unwrap();
        assert!(plan
            .lines
            .starts_with(&format!("{ORGANIZER_LINE}{CHAIR_ROW}ATTENDEE;")));
        assert!(!plan.lines.contains("bob@example.net"), "{}", plan.lines);
        assert_eq!(
            plan.change,
            PeopleChange::Replace {
                removed: vec!["bob@example.net".into()],
                added: vec!["carol@example.net".into()]
            }
        );
    }

    /// 74a: every guest removed, confirmed by the host, drops the meeting
    /// lines; the server then cancels for everyone. Without the host's word
    /// an empty list keeps them.
    #[test]
    fn removing_every_guest_needs_the_hosts_word() {
        let mut edit = read();
        edit.attendees.clear();
        let plan = plan_block(master_block(), &edit, &ctx()).unwrap();
        assert_eq!(plan.change, PeopleChange::Keep);
        edit.clear_attendees = true;
        let plan = plan_block(master_block(), &edit, &ctx()).unwrap();
        assert_eq!(plan.change, PeopleChange::Clear);
        assert_eq!(plan.lines, "SEQUENCE:0\r\n");
    }

    #[test]
    fn someone_elses_meeting_keeps_its_lines_and_refuses_a_new_guest() {
        let other = WriteCtx {
            identity: Some(OwnIdentity::from_hrefs(
                &["mailto:bob@example.net".into()],
                &url::Url::parse("https://p42-caldav.icloud.com/9/principal/").unwrap(),
            )),
            ..ctx()
        };
        let mut edit = read();
        edit.title = "mine now".into();
        // Bob's own copy: Bob is the guest, Toni the organizer.
        edit.attendees = vec!["Bob Guest <bob@example.net>".into()];
        let plan = plan_block(master_block(), &edit, &other).unwrap();
        assert!(plan.attendee_copy);
        assert_eq!(
            plan.lines,
            format!("{ORGANIZER_LINE}{CHAIR_ROW}{BOB_ROW}SEQUENCE:0\r\n")
        );
        edit.attendees.push("eve@example.net".into());
        let err = plan_block(master_block(), &edit, &other).unwrap_err();
        assert!(matches!(err, CaldavError::Forbidden(_)), "{err:?}");
    }

    #[test]
    fn unknown_own_addresses_refuse_a_meeting() {
        let edit = read();
        let err = plan_block(
            master_block(),
            &edit,
            &WriteCtx {
                identity: None,
                ..ctx()
            },
        )
        .unwrap_err();
        assert!(matches!(err, CaldavError::Network(_)), "{err:?}");
    }

    #[test]
    fn a_plain_event_gets_its_first_guests_only_when_notifying() {
        let plain = "BEGIN:VEVENT\r\nUID:p\r\nSUMMARY:x\r\nDTSTART:20261109T150000Z\r\nDTEND:20261109T160000Z\r\nEND:VEVENT\r\n";
        let mut edit = read();
        edit.attendees = vec!["bob@example.net".into()];
        let silent = plan_block(plain, &edit, &ctx()).unwrap();
        assert_eq!(silent.lines, "");
        assert!(silent.invitees.is_empty());
        edit.send_invitations = true;
        let plan = plan_block(plain, &edit, &ctx()).unwrap();
        assert_eq!(
            plan.lines,
            "ORGANIZER:mailto:toni@example.org\r\nATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:bob@ex\r\n ample.net\r\n"
        );
        assert!(matches!(plan.change, PeopleChange::Invite { .. }));
    }

    #[test]
    fn a_change_reaches_the_other_blocks_of_the_series() {
        let block = format!(
            "BEGIN:VEVENT\r\nRECURRENCE-ID:20261116T150000Z\r\n{ORGANIZER_LINE}{CHAIR_ROW}{BOB_ROW}SUMMARY:moved\r\nEND:VEVENT\r\n"
        );
        let replaced = apply_to_block(
            &block,
            &PeopleChange::Replace {
                removed: vec!["bob@example.net".into()],
                added: vec!["carol@example.net".into()],
            },
        );
        assert_eq!(
            replaced,
            format!("BEGIN:VEVENT\r\nRECURRENCE-ID:20261116T150000Z\r\nATTENDEE;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;RSVP=TRUE:mailto:carol@\r\n example.net\r\n{ORGANIZER_LINE}{CHAIR_ROW}SUMMARY:moved\r\nEND:VEVENT\r\n")
        );
        let cleared = apply_to_block(&block, &PeopleChange::Clear);
        assert_eq!(
            cleared,
            "BEGIN:VEVENT\r\nRECURRENCE-ID:20261116T150000Z\r\nSUMMARY:moved\r\nEND:VEVENT\r\n"
        );
        assert_eq!(apply_to_block(&block, &PeopleChange::Keep), block);
    }
}
