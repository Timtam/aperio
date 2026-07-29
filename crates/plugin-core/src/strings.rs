//! Text that belongs to a plugin, in the languages the plugin speaks.
//!
//! Every string an adapter contributes comes from the adapter: the labels on
//! its connect form, and — the reason this exists — the labels it puts into an
//! event that travels to other people's calendars. The host never carries
//! translations on an adapter's behalf, because the host would then have to
//! learn a word about every provider anyone ever writes an adapter for.
//!
//! ## Why a declarative catalogue rather than a callback
//!
//! Three things need strings before any plugin code can run, or without running
//! it at all:
//!
//! - The connect form is drawn from the manifest. A label that required an FFI
//!   call would put a foreign code path on every repaint of the accounts panel.
//! - The language *list* has to be known to draw a language picker. Asking the
//!   plugin which languages it has, in order to decide which language to ask it
//!   for, is a circle.
//! - A translator can send a pull request against a JSON file. They cannot
//!   patch a compiled shared library.
//!
//! Every plugin ecosystem that faced this landed in the same place — VS Code's
//! `package.nls.json` (the shell renders contributions before activation),
//! WebExtensions' `_locales/<lang>/messages.json` with a mandatory
//! `default_locale`, WordPress's gettext text domains.
//!
//! ## The escape hatch
//!
//! NOT YET BUILT, and named here so the shape is agreed before anyone needs it:
//! a plugin that wants Fluent, gettext, plural rules or a catalogue it fetches
//! itself will export `aperio_plugin_strings`, answering per language, and that
//! will take precedence over the manifest. An optional NAMED export rather than
//! a vtable slot, so it costs no ABI revision — the same shape
//! `aperio_plugin_discover` and `aperio_plugin_interactive_auth` already use.
//! The host will call it once per language and cache the result, so a label
//! still never costs an FFI call on a repaint. The manifest keeps declaring
//! which languages exist regardless, for the circle above.
//!
//! ## Which language
//!
//! Not necessarily the one the app is in. A label on a connect form is read by
//! the person at the keyboard, so that one follows the UI. A label written into
//! an event is read by whoever the invitation reaches, is frozen at write time,
//! and cannot be re-rendered later — so the caller names the language, and the
//! event editor asks. Every provider that faced this (Webex's Hybrid Calendar
//! Service, Teams, Zoom) resolves the organizer's language once at write time
//! and emits ONE language; none uses the recipient's, because text baked into an
//! event body is the same for every attendee.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The language every catalogue must carry, and the last stop before a
/// verbatim fallback.
///
/// English rather than "the first one declared" so the fallback is predictable
/// from outside: a host adding a language never silently changes which
/// translation an unrelated plugin degrades to.
pub const FALLBACK_LANG: &str = "en";

/// A plugin's own strings, keyed by language tag then by key.
///
/// The outer key is a BCP-47 language tag as the host uses it (`"en"`, `"de"`).
/// Matching is exact after lowercasing, plus one fallback from a regional tag
/// to its base (`"de-AT"` → `"de"`), because a plugin that wrote `de` should
/// serve an Austrian user rather than dropping them to English.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StringCatalogue(pub BTreeMap<String, BTreeMap<String, String>>);

impl StringCatalogue {
    /// The languages this catalogue declares, lowercased and sorted.
    ///
    /// What a language picker is built from — readable without loading, let
    /// alone calling, the plugin.
    pub fn languages(&self) -> Vec<String> {
        let mut out: Vec<String> = self.0.keys().map(|l| l.to_ascii_lowercase()).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Look `key` up in `lang`, then in the base of a regional `lang`, then in
    /// [`FALLBACK_LANG`]. `None` when no language has it.
    pub fn lookup(&self, key: &str, lang: &str) -> Option<&str> {
        let lang = lang.to_ascii_lowercase();
        let base = lang.split(['-', '_']).next().unwrap_or(&lang).to_string();
        for candidate in [lang.as_str(), base.as_str(), FALLBACK_LANG] {
            if let Some(found) = self.in_language(candidate).and_then(|m| m.get(key)) {
                return Some(found.as_str());
            }
        }
        None
    }

    /// Case-insensitive lookup of one language's map.
    fn in_language(&self, lang: &str) -> Option<&BTreeMap<String, String>> {
        self.0
            .iter()
            .find(|(declared, _)| declared.eq_ignore_ascii_case(lang))
            .map(|(_, map)| map)
    }

    /// Whether the catalogue carries nothing at all. `serde`
    /// `skip_serializing_if`, so an absent block round-trips as absent.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether [`FALLBACK_LANG`] is present and non-empty.
    ///
    /// Not enforced at parse time: a plugin that declares only German is
    /// answering honestly, and refusing to load it would be worse than serving
    /// its verbatim labels to everyone else. The loader logs it instead.
    pub fn has_fallback(&self) -> bool {
        self.in_language(FALLBACK_LANG)
            .is_some_and(|m| !m.is_empty())
    }
}

/// Resolve one label the way every caller should: the plugin's catalogue in the
/// requested language, then the verbatim text the manifest carries anyway.
///
/// The verbatim last stop is what makes a third-party plugin work with no
/// catalogue at all — it writes `label` and nothing else, and its form renders
/// in whatever language it wrote.
pub fn resolve_label<'a>(
    catalogue: Option<&'a StringCatalogue>,
    key: Option<&str>,
    verbatim: &'a str,
    lang: &str,
) -> &'a str {
    key.and_then(|key| catalogue?.lookup(key, lang))
        .unwrap_or(verbatim)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> StringCatalogue {
        serde_json::from_str(
            r#"{
                "en": {"join": "Join the meeting", "phone": "Join by phone"},
                "de": {"join": "Meeting beitreten"}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn a_key_present_in_the_asked_language_wins() {
        assert_eq!(catalogue().lookup("join", "de"), Some("Meeting beitreten"));
    }

    #[test]
    fn a_regional_tag_falls_back_to_its_base_before_english() {
        // A plugin that wrote `de` should serve an Austrian reader, not drop
        // them to English for want of a `de-AT`.
        assert_eq!(
            catalogue().lookup("join", "de-AT"),
            Some("Meeting beitreten")
        );
        assert_eq!(
            catalogue().lookup("join", "de_CH"),
            Some("Meeting beitreten")
        );
    }

    #[test]
    fn a_key_the_language_lacks_degrades_to_english_not_to_nothing() {
        // `phone` exists only in `en`. A German reader gets the English line
        // rather than a missing one — a label they can still act on.
        assert_eq!(catalogue().lookup("phone", "de"), Some("Join by phone"));
    }

    #[test]
    fn an_unknown_key_is_absent_rather_than_invented() {
        assert_eq!(catalogue().lookup("nope", "en"), None);
    }

    #[test]
    fn language_matching_ignores_case() {
        assert_eq!(catalogue().lookup("join", "DE"), Some("Meeting beitreten"));
    }

    #[test]
    fn the_language_list_is_what_a_picker_is_built_from() {
        assert_eq!(catalogue().languages(), vec!["de".to_string(), "en".into()]);
        assert!(catalogue().has_fallback());
        assert!(!StringCatalogue::default().has_fallback());
    }

    #[test]
    fn a_plugin_without_a_catalogue_still_renders_its_own_words() {
        // The third-party case: `label` in the manifest and nothing else.
        assert_eq!(
            resolve_label(None, Some("join"), "Join the meeting", "de"),
            "Join the meeting"
        );
    }

    #[test]
    fn a_label_with_no_key_is_taken_verbatim() {
        let c = catalogue();
        assert_eq!(resolve_label(Some(&c), None, "Whatever", "de"), "Whatever");
    }

    #[test]
    fn a_key_that_resolves_beats_the_verbatim_label() {
        let c = catalogue();
        assert_eq!(
            resolve_label(Some(&c), Some("join"), "Join the meeting", "de"),
            "Meeting beitreten"
        );
    }
}
