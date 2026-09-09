//! Who a task belongs to.
//!
//! One rule, used in three places that would otherwise each decide it: the
//! reminder scheduler (does this task ring for me?), the day-start ownership
//! filter (is it offered to me?), and the "Done — N by me, M by others" split.
//!
//! It lived twice — here in spirit and privately inside
//! `host_core::reminders`, plus a TypeScript copy in
//! `shared/taskAssignment.ts`. Private meant Rust could not even reuse its own
//! copy, so a second Rust caller would have written a third. The two halves are
//! pinned against each other by `shared/contracts/taskOwnership.json`, which
//! both languages read.

use crate::TaskUser;

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
    match me {
        None => true,
        Some(me) => assignees.is_empty() || assignees.iter().any(|a| a.id == me.id),
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
