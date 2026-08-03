import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

// Siri's German phrases live in a different file from the phrases themselves,
// and nothing checks that the two agree.
//
// `mobile/ios-app/AperioShortcuts.swift` declares the phrases in English — the
// development language — and `de.lproj/AppShortcuts.strings` translates them,
// keyed by the English text VERBATIM. Reword a phrase in the Swift and the key
// stops matching. That does not fail the build and does not fail at runtime: it
// silently drops the German wording, a German Siri stops recognising the
// sentence, and the request goes to Apple's Calendar instead. Which is exactly
// how this feature failed the first time it reached a phone.
//
// So the same treatment as the widget snapshot's wire check: read both sides and
// insist they line up. Cheap here, twenty-five minutes and a device otherwise.

function iosApp(relative: string): string {
  // Relative to the repo root, which is where vitest runs.
  return readFileSync(resolve(process.cwd(), 'mobile/ios-app', relative), 'utf8');
}

const swift = iosApp('AperioShortcuts.swift');

/** The app-name placeholder, spelled one way in Swift and another in a
 *  `.strings` file. Apple requires it in EVERY phrase and fails the build
 *  without it — a rule worth catching in a second rather than in a build. */
const SWIFT_APP_NAME = '\\(.applicationName)';
const STRINGS_APP_NAME = '${applicationName}';

/** Strip `/* … *\/` blocks so prose in a comment cannot be read as an entry. */
function withoutComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, '');
}

/** Every phrase declared in a `phrases: [ … ]` array, as its `.strings` key. */
function swiftPhrases(): string[] {
  const phrases: string[] = [];
  for (const block of swift.matchAll(/phrases:\s*\[([\s\S]*?)\]/g)) {
    for (const literal of block[1]!.matchAll(/"([^"]*)"/g)) {
      phrases.push(literal[1]!.split(SWIFT_APP_NAME).join(STRINGS_APP_NAME));
    }
  }
  return phrases;
}

/** `key → value` from a `.strings` file. */
function strings(relative: string): Map<string, string> {
  const entries = new Map<string, string>();
  const source = withoutComments(iosApp(relative));
  for (const line of source.matchAll(/^\s*"([^"]+)"\s*=\s*"([^"]+)"\s*;/gm)) {
    entries.set(line[1]!, line[2]!);
  }
  return entries;
}

describe('the Siri phrases have German translations', () => {
  const phrases = swiftPhrases();
  const german = strings('de.lproj/AppShortcuts.strings');

  it('found the phrases at all', () => {
    // Guards the parser rather than the code: a regex that silently matched
    // nothing would make every check below pass without testing anything.
    expect(phrases.length).toBeGreaterThan(4);
  });

  it('translates every phrase', () => {
    for (const phrase of phrases) {
      expect(german.has(phrase), `de.lproj/AppShortcuts.strings has no key \`${phrase}\``).toBe(
        true,
      );
    }
  });

  it('has no translation for a phrase that no longer exists', () => {
    for (const key of german.keys()) {
      expect(phrases, `\`${key}\` is translated but is not a phrase any more`).toContain(key);
    }
  });

  it('keeps the app-name placeholder in both languages', () => {
    for (const phrase of phrases) {
      expect(phrase, 'a phrase without the app name fails the iOS build').toContain(
        STRINGS_APP_NAME,
      );
    }
    for (const [key, value] of german) {
      expect(value, `the German for \`${key}\` drops the app name`).toContain(STRINGS_APP_NAME);
    }
  });

  it('does not translate a phrase into itself', () => {
    // A copy-paste that left the English in place reads as "translated" to
    // every other check here, and to Xcode.
    for (const [key, value] of german) {
      expect(value, `\`${key}\` is still English`).not.toBe(key);
    }
  });
});

describe('the spoken and displayed wording has German translations', () => {
  const german = strings('de.lproj/Localizable.strings');
  const source = withoutComments(swift);

  /** The literals Siri reads out or shows: intent and shortcut names, their
   *  descriptions, parameter labels and the questions asked for each one.
   *
   *  Deliberately an explicit list of the attribute positions rather than
   *  "every string in the file" — the file also holds an App Group id, action
   *  names and a date format, none of which are ever spoken. A new KIND of
   *  user-facing attribute would go unchecked here; the reverse direction below
   *  is the safety net for the mistake that actually happens. */
  const spoken = new Set<string>();
  for (const pattern of [
    /LocalizedStringResource\s*=\s*"([^"]+)"/g,
    /IntentDescription\("([^"]+)"\)/g,
    /\btitle:\s*"([^"]+)"/g,
    /shortTitle:\s*"([^"]+)"/g,
    /requestValueDialog:\s*"([^"]+)"/g,
  ]) {
    for (const match of source.matchAll(pattern)) spoken.add(match[1]!);
  }

  it('found the wording at all', () => {
    expect(spoken.size).toBeGreaterThan(5);
  });

  it('translates everything Siri says or shows', () => {
    for (const literal of spoken) {
      expect(
        german.has(literal),
        `de.lproj/Localizable.strings has no key \`${literal}\``,
      ).toBe(true);
    }
  });

  it('has no translation for wording that no longer exists', () => {
    for (const key of german.keys()) {
      expect(source, `\`${key}\` is translated but appears nowhere in the Swift`).toContain(
        `"${key}"`,
      );
    }
  });
});
