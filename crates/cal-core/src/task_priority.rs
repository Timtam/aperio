//! What a priority MEANS — the rules, separate from how it is drawn.
//!
//! Every surface shows priority differently. The desktop draws exclamation
//! marks, mobile draws a star, an e-ink display will plausibly want neither.
//! What none of them may differ on is the ORDER: two devices showing one task
//! list must put the same task first, or the list a person learned the shape of
//! stops being the same list.
//!
//! So the glyphs stay in the frontends and the ranking lives here. That split
//! is the same one the day markers and the collation follow: the core answers
//! with a state or an order, each surface picks its characters.
//!
//! # Why this is not in `cal-core-wasm`
//!
//! It was. `priority_rank` was written into `crates/cal-core-wasm/src/rules.rs`
//! when the desktop first needed a synchronous road into Rust — and that crate
//! is the DESKTOP's binding, so the rule was reachable from exactly one surface
//! while the other two kept running the TypeScript copy. Three implementations
//! of an order that must not differ.
//!
//! `cal-core` is the core: what a frontend calls synchronously lives here, and
//! the bindings only translate.
//!
//! # The two-level scale is a LENS, not a migration
//!
//! [`PriorityScale::Two`] shows a task as important or normal. It does not
//! rewrite anything: a task a provider stores as `low` stays `low` in the
//! database and at the provider, so switching the setting back and forth loses
//! nothing and no provider's data is changed behind the user's back.

use serde::{Deserialize, Serialize};

use crate::TaskPriority;

/// How many priority levels the user is shown.
///
/// Carried as an enum rather than as `"two"`/`"three"` strings so the union the
/// frontends switch on is generated from this list. Adding a scale is then a
/// compile error on both surfaces until they handle it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS), ts(export))]
pub enum PriorityScale {
    /// Low, medium, high — the historical behaviour, and the default so a
    /// caller that does not care sorts as it always did.
    #[default]
    Three,
    /// Important or normal. The synced `tasks.twoLevelPriority` setting.
    Two,
}

impl TaskPriority {
    /// Whether this is the TOP priority — the one level that survives in the
    /// two-level system, where it is called "important" rather than "high".
    pub fn is_important(self) -> bool {
        self == TaskPriority::High
    }
}

/// Sort rank: 0 sorts first.
///
/// The two-level scale collapses low and medium into ONE band. It has to: the
/// two are indistinguishable on screen there, so ranking them apart would order
/// a list by an attribute the reader cannot perceive, and the A-to-Z tiebreak
/// each band promises would appear to break at random.
///
/// This is the call that decided a synchronous core binding was needed at all.
/// Its callers are `Array.prototype.sort` comparators, which cannot await —
/// an async comparator returns promises, every promise compares equal, and the
/// list comes out in arbitrary order.
pub fn priority_rank(priority: TaskPriority, scale: PriorityScale) -> u32 {
    match scale {
        PriorityScale::Two => u32::from(!priority.is_important()),
        PriorityScale::Three => match priority {
            TaskPriority::High => 0,
            TaskPriority::Medium => 1,
            TaskPriority::Low => 2,
        },
    }
}

/// The priority a task gets when the user clears "important" in the two-level
/// system: whatever it already had, as long as that is not the top one.
///
/// Unchecking must not rewrite `low` into `medium`. Both read as "normal" and
/// look identical on every surface, so the write would change nothing the user
/// can see while changing what other clients — and the three-level system, if
/// they switch back — display. `None` means the task carried nothing before;
/// `Medium` is the neutral answer.
pub fn normal_priority(previous: Option<TaskPriority>) -> TaskPriority {
    match previous {
        Some(p) if !p.is_important() => p,
        _ => TaskPriority::Medium,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_level_scale_ranks_high_first() {
        assert_eq!(priority_rank(TaskPriority::High, PriorityScale::Three), 0);
        assert_eq!(priority_rank(TaskPriority::Medium, PriorityScale::Three), 1);
        assert_eq!(priority_rank(TaskPriority::Low, PriorityScale::Three), 2);
    }

    #[test]
    fn the_two_level_scale_puts_low_and_medium_in_one_band() {
        // Not a rounding of the three-level answer: low and medium must rank
        // EQUAL, or the list orders itself by something the reader cannot see.
        assert_eq!(priority_rank(TaskPriority::High, PriorityScale::Two), 0);
        assert_eq!(
            priority_rank(TaskPriority::Medium, PriorityScale::Two),
            priority_rank(TaskPriority::Low, PriorityScale::Two),
        );
        assert_eq!(priority_rank(TaskPriority::Low, PriorityScale::Two), 1);
    }

    #[test]
    fn three_levels_is_the_default_scale() {
        // A caller that does not name a scale sorts as the app always did.
        assert_eq!(
            priority_rank(TaskPriority::Low, PriorityScale::default()),
            priority_rank(TaskPriority::Low, PriorityScale::Three),
        );
    }

    #[test]
    fn clearing_important_keeps_what_the_task_already_had() {
        // The whole point: unchecking must not silently promote `low` to
        // `medium`, because the two look identical here and different in the
        // three-level system the user may switch back to.
        assert_eq!(
            normal_priority(Some(TaskPriority::Low)),
            TaskPriority::Low,
            "unchecking must not rewrite low into medium",
        );
        assert_eq!(
            normal_priority(Some(TaskPriority::Medium)),
            TaskPriority::Medium,
        );
        // It WAS the top one, so there is nothing to keep.
        assert_eq!(
            normal_priority(Some(TaskPriority::High)),
            TaskPriority::Medium,
        );
        // Nothing before: the neutral answer.
        assert_eq!(normal_priority(None), TaskPriority::Medium);
    }

    #[test]
    fn only_the_top_priority_is_important() {
        assert!(TaskPriority::High.is_important());
        assert!(!TaskPriority::Medium.is_important());
        assert!(!TaskPriority::Low.is_important());
    }
}
