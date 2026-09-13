//! Who a task belongs to.
//!
//! One rule, used in three places that would otherwise each decide it: the
//! reminder scheduler (does this task ring for me?), the day-start ownership
//! filter (is it offered to me?), and the "Done — N by me, M by others" split.
//!
//! It lived twice — here in spirit and privately inside
//! `host_core::reminders`, plus a TypeScript copy in
//! `shared/taskAssignment.ts`. Private meant Rust could not even reuse its own
//! copy, so a second Rust caller would have written a third. The TypeScript
//! copy went when the day start moved into the core (`crate::day_start`, which
//! asks [`mine_or_unassigned`] over ids); `shared/contracts/taskOwnership.json`
//! still pins the rule, read by the reminder scheduler's test in
//! `host_core::reminders` and by the TypeScript suite through the day-start door.
//!
//! # The three rules around it
//!
//! Who holds a task after a status change ([`self_assign_on_status_change`]:
//! take it when starting or finishing a task nobody owns, step back when
//! reopening one I hold), how many people a list can hold on one task
//! ([`task_assignment_mode`]: an undeclared capability is none), and what a
//! list of assignees is trimmed to when a task moves ([`clamp_assignees`]: the
//! FIRST stays, the same one Todoist's adapter keeps). Both surfaces apply
//! them when a task is checked off or moved; they were TypeScript on both, and
//! are here now. A user is its id on the wire — the rules compare ids alone —
//! and the answer is positions (which of the given assignees stay) or a state
//! ("take me"), never a user row (DESIGN §4.5 a). Pinned by
//! `tests/fixtures/taskAssignment.json`, measured from the TypeScript this
//! replaces; the `contract` module below reads it, and so does the
//! TypeScript contract test on the other side of the boundary.

use serde::{Deserialize, Serialize};

use crate::{TaskAssignment, TaskStatus, TaskUser};

/// Is this task mine to act on?
///
/// True when the account has no identity at all (`me` is `None` — the local
/// store and other personal-style adapters), when nobody is assigned, or when
/// I am one of the assignees. False ONLY when it is assigned to concrete OTHER
/// people and not to me.
///
/// The false case is the one that matters: in a shared Vikunja or Todoist list
/// a colleague's task must not ring on my phone, must not be offered in my
/// day-start review, and must not be counted among the things I finished. It is
/// still visible — this decides ownership, not visibility.
pub fn is_mine_or_unassigned(assignees: &[TaskUser], me: Option<&TaskUser>) -> bool {
    mine_or_unassigned(
        assignees.iter().map(|a| a.id.as_str()),
        me.map(|m| m.id.as_str()),
    )
}

/// [`is_mine_or_unassigned`] over ids — the form a door carries a user in.
pub(crate) fn mine_or_unassigned<'a>(
    assignees: impl IntoIterator<Item = &'a str>,
    me: Option<&str>,
) -> bool {
    let Some(me) = me else {
        return true;
    };
    let mut nobody = true;
    for assignee in assignees {
        if assignee == me {
            return true;
        }
        nobody = false;
    }
    nobody
}

/// The question [`self_assign_on_status_change`] answers, over the wire. A
/// user is its id.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct SelfAssignInput {
    /// The status the task changes to.
    pub status: TaskStatus,
    /// The assignee ids as the task carries them, in order.
    pub assignees: Vec<String>,
    /// The account's own id, or none when the adapter reports no identity.
    #[serde(default)]
    pub me: Option<String>,
    /// The Settings → Tasks toggle.
    pub enabled: bool,
}

/// Who holds the task afterwards: nothing changes, I take it, or these of
/// the given assignees stay (positions into the input, in order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "change", rename_all = "snake_case")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum SelfAssignOutcome {
    Unchanged,
    AssignMe,
    Keep { positions: Vec<usize> },
}

/// The new holders of a task after it changes to `status`, in shared lists
/// where the adapter knows "me":
/// - into `in_progress` or `completed` on an UNASSIGNED task → I take it;
/// - into `open` while I am an assignee → the others stay, I step back;
/// - otherwise nothing changes.
///
/// Only with `enabled` and an identity. The step-back removes ONLY me — every
/// copy of me, should a provider list the same person twice — and keeps the
/// colleagues in their order; symmetric to the auto-assign, which acts only
/// on a task nobody owns. `cancelled` is neither taking nor stepping back.
pub fn self_assign_on_status_change(
    status: TaskStatus,
    assignee_ids: &[String],
    me: Option<&str>,
    enabled: bool,
) -> SelfAssignOutcome {
    if !enabled {
        return SelfAssignOutcome::Unchanged;
    }
    let Some(me) = me else {
        return SelfAssignOutcome::Unchanged;
    };
    let became_active = matches!(status, TaskStatus::InProgress | TaskStatus::Completed);
    if became_active && assignee_ids.is_empty() {
        return SelfAssignOutcome::AssignMe;
    }
    if status == TaskStatus::Open && assignee_ids.iter().any(|a| a == me) {
        return SelfAssignOutcome::Keep {
            positions: assignee_ids
                .iter()
                .enumerate()
                .filter(|(_, a)| a.as_str() != me)
                .map(|(i, _)| i)
                .collect(),
        };
    }
    SelfAssignOutcome::Unchanged
}

/// [`self_assign_on_status_change`] over the wire.
pub fn self_assign_on_status_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: SelfAssignInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&self_assign_on_status_change(
        input.status,
        &input.assignees,
        input.me.as_deref(),
        input.enabled,
    ))
}

/// The slice of a list's capabilities this rule reads; the rest of the block
/// is the plugin manifest's (`plugin_core::TaskCapabilities`), and a full one
/// deserializes into this.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AssignmentCapabilities {
    #[serde(default)]
    pub task_assignment: TaskAssignment,
}

/// The question [`task_assignment_mode`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct AssignmentModeInput {
    /// The list's capabilities, or none when the list has none or is unknown.
    #[serde(default)]
    pub capabilities: Option<AssignmentCapabilities>,
}

/// How many people a list can hold on one task. Absent capabilities, or a
/// block that does not say, mean `None`: an adapter that has not said it can
/// assign is taken at its word rather than credited with an ability whose
/// failure is silent — the editor would offer a choice the source cannot keep.
pub fn task_assignment_mode(capabilities: Option<&AssignmentCapabilities>) -> TaskAssignment {
    capabilities.map(|c| c.task_assignment).unwrap_or_default()
}

/// [`task_assignment_mode`] over the wire.
pub fn task_assignment_mode_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: AssignmentModeInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&task_assignment_mode(input.capabilities.as_ref()))
}

/// The question [`clamp_assignees`] answers, over the wire.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub struct ClampAssigneesInput {
    pub mode: TaskAssignment,
    /// The assignee ids as the form holds them, in order.
    pub assignees: Vec<String>,
}

/// Which of `count` assignees a list in `mode` can hold: positions, in order.
/// `Single` keeps the FIRST — the same one Todoist's adapter keeps when it
/// clamps (`first_assignee_id`), so the editor and the wire agree about who
/// survives. `None` is deliberately NOT emptied: the editor simply does not
/// show the picker there, and the task may still carry assignees another
/// client wrote — clearing them on open would destroy what the user was never
/// shown, which is the whole failure this capability exists to stop.
pub fn clamp_assignees(mode: TaskAssignment, count: usize) -> Vec<usize> {
    let kept = match mode {
        TaskAssignment::Single => count.min(1),
        TaskAssignment::None | TaskAssignment::Multiple => count,
    };
    (0..kept).collect()
}

/// [`clamp_assignees`] over the wire.
pub fn clamp_assignees_json(input_json: &str) -> Result<String, serde_json::Error> {
    let input: ClampAssigneesInput = serde_json::from_str(input_json)?;
    serde_json::to_string(&clamp_assignees(input.mode, input.assignees.len()))
}

/// The contract with the TypeScript this replaced.
///
/// Its other half is `src/state/taskAssignment.contract.test.ts`, reading
/// this same file through the doors. The fixture was written by running the
/// TypeScript BEFORE the port, so the port is measured against what was.
#[cfg(test)]
mod contract {
    use super::*;
    use serde_json::{json, Value};

    const CONTRACT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/taskAssignment.json"
    ));

    fn doc() -> Value {
        serde_json::from_str(CONTRACT).expect("the contract parses")
    }

    fn ids(v: &Value) -> Vec<String> {
        v.as_array()
            .expect("an id list")
            .iter()
            .map(|s| s.as_str().expect("an id").to_string())
            .collect()
    }

    fn has_row(cases: &[Value], name: &str) {
        assert!(
            cases.iter().any(|c| c["name"] == name),
            "the contract lost `{name}`"
        );
    }

    #[test]
    fn every_self_assign_row_holds() {
        let doc = doc();
        let cases = doc["selfAssign"]
            .as_array()
            .expect("selfAssign is an array");
        for needed in [
            "an-unassigned-task-going-in-progress-becomes-mine",
            "a-task-a-colleague-holds-is-not-taken",
            "reopening-removes-only-me",
            "reopening-keeps-the-order-when-i-am-in-the-middle",
            "reopening-removes-every-copy-of-me",
            "cancelling-changes-nothing",
            "no-identity-changes-nothing",
        ] {
            has_row(cases, needed);
        }
        for case in cases {
            let i = &case["input"];
            let assignees = ids(&i["assignees"]);
            let me = i["me"].as_str();
            let status: TaskStatus =
                serde_json::from_value(i["nextStatus"].clone()).expect("a status");
            let got = match self_assign_on_status_change(
                status,
                &assignees,
                me,
                i["enabled"].as_bool().expect("a bool"),
            ) {
                SelfAssignOutcome::Unchanged => Value::Null,
                SelfAssignOutcome::AssignMe => json!([me.expect("assign-me needs an identity")]),
                SelfAssignOutcome::Keep { positions } => {
                    json!(positions.iter().map(|&p| &assignees[p]).collect::<Vec<_>>())
                }
            };
            assert_eq!(got, case["expect"], "{}: {}", case["name"], case["note"]);
        }
    }

    #[test]
    fn every_mode_row_holds() {
        let doc = doc();
        let cases = doc["mode"].as_array().expect("mode is an array");
        has_row(
            cases,
            "a-capabilities-block-without-the-field-cannot-assign",
        );
        for case in cases {
            let caps = &case["input"]["capabilities"];
            let caps: Option<AssignmentCapabilities> = if caps.is_null() || caps == "absent" {
                None
            } else {
                Some(serde_json::from_value(caps.clone()).expect("a capabilities block"))
            };
            let got = task_assignment_mode(caps.as_ref());
            assert_eq!(json!(got), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn every_clamp_row_holds() {
        let doc = doc();
        let cases = doc["clamp"].as_array().expect("clamp is an array");
        has_row(cases, "single-keeps-the-first");
        has_row(cases, "none-never-empties-what-it-cannot-show");
        for case in cases {
            let assignees = ids(&case["input"]["assignees"]);
            let mode: TaskAssignment =
                serde_json::from_value(case["input"]["mode"].clone()).expect("a mode");
            let kept: Vec<&String> = clamp_assignees(mode, assignees.len())
                .into_iter()
                .map(|p| &assignees[p])
                .collect();
            assert_eq!(json!(kept), case["expect"], "{}", case["name"]);
        }
    }

    #[test]
    fn the_wire_speaks_the_outcome_and_reads_a_full_capabilities_block() {
        assert_eq!(
            self_assign_on_status_json(
                r#"{"status": "open", "assignees": ["a", "me", "b"], "me": "me", "enabled": true}"#
            )
            .expect("valid"),
            r#"{"change":"keep","positions":[0,2]}"#
        );
        assert_eq!(
            self_assign_on_status_json(
                r#"{"status": "completed", "assignees": [], "me": "me", "enabled": true}"#
            )
            .expect("valid"),
            r#"{"change":"assign_me"}"#
        );
        assert_eq!(
            self_assign_on_status_json(
                r#"{"status": "completed", "assignees": [], "enabled": true}"#
            )
            .expect("valid"),
            r#"{"change":"unchanged"}"#
        );
        // A full manifest block deserializes: the other fields are ignored.
        assert_eq!(
            task_assignment_mode_json(
                r#"{"capabilities": {"nested_projects": true, "task_span": false,
                    "task_assignment": "multiple", "subtasks": true}}"#
            )
            .expect("valid"),
            r#""multiple""#
        );
        assert_eq!(
            clamp_assignees_json(r#"{"mode": "single", "assignees": ["a", "b"]}"#).expect("valid"),
            "[0]"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> TaskUser {
        TaskUser {
            id: id.to_string(),
            name: id.to_string(),
            email: None,
        }
    }

    #[test]
    fn a_personal_list_has_no_identity_so_everything_is_mine() {
        // The local store never reports a "me". Filtering there would silence
        // every reminder the user has.
        assert!(is_mine_or_unassigned(&[], None));
        assert!(is_mine_or_unassigned(&[user("someone")], None));
    }

    #[test]
    fn an_unassigned_task_in_a_shared_list_is_mine() {
        assert!(is_mine_or_unassigned(&[], Some(&user("me"))));
    }

    #[test]
    fn a_task_assigned_to_me_is_mine_even_alongside_others() {
        let me = user("me");
        assert!(is_mine_or_unassigned(
            &[user("colleague"), me.clone()],
            Some(&me)
        ));
    }

    #[test]
    fn a_colleagues_task_is_not_mine() {
        // The only false case, and the reason the rule exists.
        assert!(!is_mine_or_unassigned(
            &[user("colleague")],
            Some(&user("me"))
        ));
    }

    #[test]
    fn ownership_is_decided_by_id_not_by_name_or_mail() {
        // Providers hand back the same person with different display names per
        // endpoint; matching on anything but the id would drop a real
        // assignment.
        let me = TaskUser {
            id: "u1".into(),
            name: "Toni".into(),
            email: Some("a@example.org".into()),
        };
        let same_person_other_label = TaskUser {
            id: "u1".into(),
            name: "T. B.".into(),
            email: None,
        };
        assert!(is_mine_or_unassigned(&[same_person_other_label], Some(&me)));
    }
}
