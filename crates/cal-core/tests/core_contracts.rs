//! The core does not read the clock, and it does not read the device's zone.
//!
//! One of two contracts written down before any more of `shared/` moves behind
//! the boundary (DESIGN §4.5, TODO A11). This file enforces the half that a
//! machine can enforce.
//!
//! # Why it matters more here than it looks
//!
//! `cal-core` is compiled into five different places that disagree about what
//! "now" is: the desktop host process, the mobile app through `cal-ffi`, the
//! desktop webview through WebAssembly, an iOS widget extension that renders
//! hours after its snapshot was taken, and twelve adapter repositories. A rule
//! that reads the clock therefore answers differently depending on WHERE it
//! runs, and the widget case is the one that shows it: the extension re-renders
//! from a file written hours earlier, so a core function reading `Utc::now()`
//! would silently mean "now, at render", not "now, at snapshot".
//!
//! The device's timezone is worse, because it is invisible. `chrono::Local`
//! reads an ambient setting; in the wasm build there is no meaningful one, and
//! on mobile it is whatever the phone is set to — which is not necessarily the
//! zone the calendar the user is looking at is written in. Aperio's day keys
//! are LOCAL days (see `DayLog::day`), and which local day a UTC instant falls
//! on is exactly the question a device zone would answer wrongly.
//!
//! So: day keys and UTC offsets are always PARAMETERS. A caller that knows
//! which clock and which zone it means passes them in.
//!
//! # The rule applies to tests too
//!
//! Deliberately no exemption for `#[cfg(test)]`. A test that reads the wall
//! clock is a test that fails on one day of the year, and pinning the instant
//! is the same one-line change either way.

use std::fs;
use std::path::{Path, PathBuf};

/// Reading the clock, or reading the machine's timezone, spelled every way
/// this repository could spell it.
///
/// `::now()` rather than `now(` on purpose: `spawn.rs` has a test called
/// `backlog_immediate_is_undated_and_visible_now`, and a needle that matched
/// function names would fire on a name while missing a rename.
///
/// Matched at a word boundary (see `contains_token`), so `&Local` does not fire
/// on `&LocalAdapter` — a type this crate could plausibly grow a reference to,
/// and one that has nothing to do with timezones.
const FORBIDDEN: &[(&str, &str)] = &[
    (
        "::now()",
        "reads the wall clock — take the instant as a parameter",
    ),
    (
        "::today()",
        "reads the wall clock — take the day as a parameter",
    ),
    (
        "chrono::Local",
        "reads the DEVICE's timezone — take the UTC offset as a parameter",
    ),
    (
        "&Local",
        "converts into the DEVICE's timezone — take the UTC offset as a parameter",
    ),
    (
        "iana_time_zone",
        "asks the operating system for its timezone — take it as a parameter",
    ),
];

/// The crates the contract binds. `cal-core` is the domain; `plugin-core` is
/// the ABI both sides of a plugin boundary compile against, so the same
/// argument applies to it — an adapter runs in the host's process on the
/// desktop and in the app's on mobile.
const BOUND_CRATES: &[&str] = &["cal-core", "plugin-core"];

/// Files that must turn up in the walk.
///
/// The anti-silence guard, and it NAMES rather than counts: a floor of "at
/// least N files" would be the number of files there are today, and the next
/// module added or moved is exactly the change it should survive. These four
/// are load-bearing enough that their disappearance is a deliberate act, and
/// one of them — `day_marker.rs` — is where the only violation this test ever
/// found actually lived.
const MUST_SCAN: &[&str] = &[
    "cal-core/src/day_marker.rs",
    "cal-core/src/recurrence.rs",
    "cal-core/src/types.rs",
    "plugin-core/src/lib.rs",
];

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cal-core lives under crates/")
        .to_path_buf()
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

/// The file with its `//` comments removed.
///
/// Without this the walk trips over prose: `adapter.rs` documents "Local
/// adapter does an SQL …", which is a sentence about a crate, not a timezone.
/// Block comments are left alone — this repository does not use them, and a
/// stripper that got them wrong would hide code rather than reveal it.
fn without_line_comments(source: &str) -> String {
    source
        .lines()
        .map(|line| match line.find("//") {
            // A `//` inside a string literal is not a comment. Rather than
            // parse Rust, keep the whole line when the prefix has an odd number
            // of quotes — the conservative direction, since keeping too much
            // can only cause a false ALARM, which a human resolves, while
            // dropping too much causes a false silence, which nobody sees.
            Some(at) if source_prefix_is_balanced(&line[..at]) => &line[..at],
            _ => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `needle` in `haystack`, but not as the prefix of a longer identifier.
///
/// The difference that matters is `&Local` against `&LocalAdapter`: the first
/// converts an instant into the machine's timezone, the second is a reference
/// to a struct in this repository. A plain `contains` cannot tell them apart,
/// and a false alarm on a type name would teach the next reader to ignore this
/// test.
fn contains_token(haystack: &str, needle: &str) -> bool {
    let mut from = 0;
    while let Some(at) = haystack[from..].find(needle) {
        let end = from + at + needle.len();
        let next = haystack[end..].chars().next();
        if !next.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            return true;
        }
        from = end;
    }
    false
}

fn source_prefix_is_balanced(prefix: &str) -> bool {
    let mut quotes = 0usize;
    let mut escaped = false;
    for ch in prefix.chars() {
        match ch {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => quotes += 1,
            _ => escaped = false,
        }
    }
    quotes.is_multiple_of(2)
}

#[test]
fn the_core_reads_neither_the_clock_nor_the_device_zone() {
    let crates = crates_dir();
    let mut scanned: Vec<String> = Vec::new();
    let mut problems: Vec<String> = Vec::new();

    for krate in BOUND_CRATES {
        let mut files = Vec::new();
        rust_sources(&crates.join(krate).join("src"), &mut files);
        for path in files {
            let rel = path
                .strip_prefix(&crates)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let source = fs::read_to_string(&path).expect("a source file this walk just found");
            let code = without_line_comments(&source);
            for (needle, why) in FORBIDDEN {
                for (n, line) in code.lines().enumerate() {
                    if contains_token(line, needle) {
                        problems.push(format!("{rel}:{}: `{needle}` — {why}", n + 1));
                    }
                }
            }
            scanned.push(rel);
        }
    }

    // Anti-silence, part one: the walk found the files it is supposed to read.
    // A wrong path would otherwise scan nothing and report nothing.
    for must in MUST_SCAN {
        assert!(
            scanned.iter().any(|s| s == must),
            "the walk never reached {must} — it scanned {} file(s), so its root is wrong \
             and this test proves nothing.\nScanned: {}",
            scanned.len(),
            scanned.join(", "),
        );
    }

    // Anti-silence, part two: the matcher can still match. A needle list that
    // stopped firing — a renamed constant, a comment stripper that ate the
    // code — would make the test above pass and this one fail.
    let sample = "let stamp = Utc::now();\nlet zone = chrono::Local;";
    let stripped = without_line_comments(sample);
    assert!(
        FORBIDDEN
            .iter()
            .filter(|(needle, _)| contains_token(&stripped, needle))
            .count()
            == 2,
        "the needles no longer match code they are meant to catch",
    );
    // ...and that the boundary is doing its job, rather than matching nothing
    // at all, which would make the line above pass for the wrong reason.
    assert!(
        !contains_token("let a = &LocalAdapter::new(db);", "&Local"),
        "the word boundary is gone — `&Local` now fires on `&LocalAdapter`",
    );

    assert!(
        problems.is_empty(),
        "the core reads the clock or the device's timezone in {} place(s). Day keys and \
         UTC offsets are PARAMETERS — see DESIGN §4.5.\n\n  {}",
        problems.len(),
        problems.join("\n  "),
    );
}

/// The mechanical half of the second contract: the core carries no
/// localization machinery.
///
/// The contract itself — *the core answers with an i18n KEY plus variables,
/// never with finished prose* — is **not** mechanically checkable, and this
/// test does not pretend otherwise. A key and a sentence are both `String`,
/// and `Error` variants legitimately carry English sentences that reach a user
/// through the container error surface. A guard that tried to tell those apart
/// would have to guess, and a guard that guesses is worse than none.
///
/// What IS checkable is the dependency edge. The day someone reaches for a
/// localization crate in `cal-core` is the day the core starts producing text,
/// and that is a decision that should be argued for rather than merged. The
/// model to copy instead is `ConferenceProvider::i18n_key`.
#[test]
fn the_core_carries_no_localization_dependency() {
    const LOCALIZATION_CRATES: &[&str] = &[
        "fluent",
        "rust-i18n",
        "gettext",
        "unic-langid",
        "sys-locale",
        "locale_config",
        "i18n-embed",
    ];
    let crates = crates_dir();
    for krate in BOUND_CRATES {
        let manifest_path = crates.join(krate).join("Cargo.toml");
        let manifest = fs::read_to_string(&manifest_path)
            .unwrap_or_else(|_| panic!("{krate} has a Cargo.toml"));
        // Named, so a renamed crate directory fails here rather than skipping
        // the check in silence.
        assert!(
            manifest.contains(&format!("name = \"{krate}\"")),
            "{} is not {krate}'s manifest — this check is looking in the wrong place",
            manifest_path.display(),
        );
        for dep in LOCALIZATION_CRATES {
            assert!(
                !manifest.contains(dep),
                "{krate} depends on `{dep}`. The core answers with an i18n KEY plus \
                 variables and lets each surface render it — see \
                 `ConferenceProvider::i18n_key` and DESIGN §4.5.",
            );
        }
    }
}
