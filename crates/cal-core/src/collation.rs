//! How text a person reads is ordered.
//!
//! Every task list, section list, backlog rail, sidebar and calendar lane ends
//! up here for its tiebreaker. The rule was `localeCompare` in TypeScript,
//! which meant three things this module changes on purpose:
//!
//! 1. **It lived only in JavaScript.** A reMarkable frontend could not borrow
//!    it, so it would have implemented a third ordering — and a list that
//!    sorts differently on one surface is the kind of wrongness nobody
//!    reports, they just lose track of where a task went.
//! 2. **It followed the RUNTIME's locale, not the app's.** Every call passed
//!    `undefined` as the locale, so a user reading Aperio in German on an
//!    English system got English collation. The language the user picked is
//!    now what decides.
//! 3. **It was not the same collation on every surface.** The desktop webview,
//!    Hermes on iOS and Hermes on Android each bring their own, of different
//!    vintages. One collator compiled into the core is the same everywhere.
//!
//! # What it costs, stated plainly
//!
//! ICU4X's baked collation data is roughly a megabyte. Measured: the desktop's
//! WebAssembly module goes from 26.7 KB to 1154.3 KB. Trimming to the app's
//! two languages does not help much — the CLDR root table is what carries the
//! weight and every locale needs it; a per-language tailoring is a small
//! addition on top. That price was weighed and accepted (TODO A11).
//!
//! # Two rules, not a bag of options
//!
//! The call sites want two different things, and naming them keeps the choice
//! from being made per-call by whoever writes the next comparator:
//!
//! - [`compare_names`] for names of things — accounts, containers, contacts,
//!   day markers. Case and accents do not separate two names a person would
//!   call the same.
//! - [`compare_titles`] for user-written text — task titles, section names.
//!   Digit runs sort by value, so "Kapitel 2" comes before "Kapitel 10".

use core::cmp::Ordering;

use icu_collator::{
    options::{CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
    Collator, CollatorBorrowed, CollatorPreferences,
};
use icu_locale_core::Locale;

/// The language whose ordering rules apply.
///
/// Deliberately not a free-form string: the app ships two languages, and a
/// caller that passes something else should get the app's default rather than
/// an error it has no way to handle in the middle of a comparator. Adding a
/// language here is the same edit as adding it to `locales/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CollationLanguage {
    /// German. The default, and the app's original language.
    #[default]
    German,
    /// English.
    English,
}

impl CollationLanguage {
    /// Read the app's language tag (`"de"`, `"de-DE"`, `"en"`, `"en-GB"`, …).
    ///
    /// Anything unrecognised answers the default rather than failing. A
    /// comparator has nowhere to report an error to, and an unknown tag is not
    /// a reason to order a list arbitrarily.
    pub fn from_tag(tag: &str) -> Self {
        match tag.split(['-', '_']).next().unwrap_or("") {
            "en" => CollationLanguage::English,
            _ => CollationLanguage::German,
        }
    }

    fn locale(self) -> Locale {
        match self {
            CollationLanguage::German => "de".parse().expect("de is a valid tag"),
            CollationLanguage::English => "en".parse().expect("en is a valid tag"),
        }
    }
}

/// Compare two NAMES — of an account, a container, a contact, a day marker.
///
/// Case- and accent-insensitive: "Arbeit" and "arbeit" are the same name to a
/// person, so neither sorts above the other, and a list that mixes them does
/// not split into two blocks. This is what `sensitivity: 'base'` meant at the
/// call sites this replaces.
pub fn compare_names(a: &str, b: &str, language: CollationLanguage) -> Ordering {
    let mut prefs = CollatorPreferences::from(&language.locale());
    prefs.numeric_ordering = Some(CollationNumericOrdering::False);
    let mut options = CollatorOptions::default();
    // Primary strength: only the base letters decide. Case and accents do not.
    options.strength = Some(Strength::Primary);
    collator(prefs, options).compare(a, b)
}

/// Compare two TITLES — task titles, section names, anything the user typed.
///
/// Digit runs sort by value, not by digit: "Kapitel 2" before "Kapitel 10".
/// Without that, a list numbered past nine reorders itself in a way that looks
/// like a bug and is impossible to fix from the outside.
///
/// Full strength, unlike [`compare_names`]: two titles differing only in case
/// are different titles, and collapsing them would make the order of the two
/// depend on which happened to be read first.
pub fn compare_titles(a: &str, b: &str, language: CollationLanguage) -> Ordering {
    let mut prefs = CollatorPreferences::from(&language.locale());
    prefs.numeric_ordering = Some(CollationNumericOrdering::True);
    collator(prefs, CollatorOptions::default()).compare(a, b)
}

/// Build a collator, or fall back to comparing codepoints.
///
/// `try_new` can only fail if the baked data is missing for the requested
/// locale, which for two compiled-in languages means never — but a comparator
/// has nowhere to report an error, and panicking inside `Array.sort` would
/// take the view down. The fallback is deliberately the dumbest possible
/// order rather than an approximation of collation: if it ever fires, a list
/// that looks obviously wrong is easier to notice than one that is subtly so.
fn collator(prefs: CollatorPreferences, options: CollatorOptions) -> CollatorOrFallback {
    match Collator::try_new(prefs, options) {
        Ok(collator) => CollatorOrFallback::Icu(collator),
        Err(_) => CollatorOrFallback::Codepoints,
    }
}

/// `try_new` hands back a `CollatorBorrowed<'static>` — a view onto the data
/// baked into the binary, so there is nothing to own and nothing to box.
enum CollatorOrFallback {
    Icu(CollatorBorrowed<'static>),
    Codepoints,
}

impl CollatorOrFallback {
    fn compare(&self, a: &str, b: &str) -> Ordering {
        match self {
            CollatorOrFallback::Icu(collator) => collator.compare(a, b),
            CollatorOrFallback::Codepoints => a.cmp(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_order_digit_runs_by_value() {
        // The reason `numeric` exists. Without it "Kapitel 10" sorts before
        // "Kapitel 2", which reads as a bug in any numbered list.
        assert_eq!(
            compare_titles("Kapitel 2", "Kapitel 10", CollationLanguage::German),
            Ordering::Less
        );
        assert_eq!(
            compare_titles("Datei 9", "Datei 100", CollationLanguage::German),
            Ordering::Less
        );
    }

    #[test]
    fn titles_are_alphabetical_across_case() {
        // Codepoint order would put every capital first, so "Zebra" would come
        // before "apfel". A collation interleaves them.
        assert_eq!(
            compare_titles("apfel", "Zebra", CollationLanguage::German),
            Ordering::Less
        );
    }

    #[test]
    fn german_umlauts_sort_with_their_base_letter() {
        // The case the app is actually full of, and the one codepoint order
        // gets worst: `Ä` is U+00C4, so it would land after every ASCII letter
        // — "Äpfel" filed behind "Zebra".
        assert_eq!(
            compare_titles("Äpfel", "Zebra", CollationLanguage::German),
            Ordering::Less
        );
        assert_eq!(
            compare_names("Ärger", "Zebra", CollationLanguage::German),
            Ordering::Less
        );
    }

    #[test]
    fn names_ignore_case_and_accents() {
        assert_eq!(
            compare_names("Arbeit", "arbeit", CollationLanguage::German),
            Ordering::Equal
        );
        assert_eq!(
            compare_names("resume", "résumé", CollationLanguage::English),
            Ordering::Equal
        );
    }

    #[test]
    fn titles_keep_case_apart() {
        // Unlike names: two titles differing only in case are two titles, and
        // answering Equal would make their order depend on input order.
        assert_ne!(
            compare_titles("Arbeit", "arbeit", CollationLanguage::German),
            Ordering::Equal
        );
    }

    #[test]
    fn a_language_tag_is_read_leniently_and_never_fails() {
        assert_eq!(
            CollationLanguage::from_tag("en"),
            CollationLanguage::English
        );
        assert_eq!(
            CollationLanguage::from_tag("en-GB"),
            CollationLanguage::English
        );
        assert_eq!(CollationLanguage::from_tag("de"), CollationLanguage::German);
        assert_eq!(
            CollationLanguage::from_tag("de_AT"),
            CollationLanguage::German
        );
        // Unknown, empty and nonsense all answer the default rather than
        // erroring into a comparator that cannot handle it.
        assert_eq!(CollationLanguage::from_tag("fr"), CollationLanguage::German);
        assert_eq!(CollationLanguage::from_tag(""), CollationLanguage::German);
    }

    #[test]
    fn the_comparison_is_a_proper_ordering() {
        let values = ["", "a", "A", "Äpfel", "apfel", "Datei 2", "Datei 10"];
        for a in values {
            assert_eq!(
                compare_titles(a, a, CollationLanguage::German),
                Ordering::Equal
            );
            for b in values {
                assert_eq!(
                    compare_titles(a, b, CollationLanguage::German),
                    compare_titles(b, a, CollationLanguage::German).reverse(),
                    "{a:?} vs {b:?}"
                );
            }
        }
    }
}
