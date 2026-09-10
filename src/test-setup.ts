// Test environment bootstrap.
// jsdom does not implement `matchMedia`. Stub it so hooks that probe
// prefers-color-scheme / prefers-reduced-motion can run.
import '@testing-library/jest-dom/vitest';

// Pin the test language so component assertions stay deterministic — the
// app default is now the *system* language (jsdom would resolve to English),
// but the existing tests assert the German UI strings.
import i18n from './i18n';
void i18n.changeLanguage('de');

if (typeof window !== 'undefined' && !window.matchMedia) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: (query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    }),
  });
}

// The core rules the desktop calls during render live in `cal-core`, compiled
// into the webview as WebAssembly. The app awaits `initCoreRules()` once in
// `main.tsx` before the first render; the test environment does the equivalent
// here, so any test that reaches a rule through `src/intl/taskStatus` finds it
// ready.
//
// `initSync` rather than the async loader: Node has no limit on synchronous
// module compilation (browsers do, which is why the app awaits), and a setup
// file that cannot await keeps every existing test synchronous.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import {
  installConferenceDetector,
  installGroupSuggestionRules,
  installMeetingDuplicateFilter,
  installMeetingLinkRules,
  installTaskPriorityRules,
  installTextCollation,
} from '@aperio/shared';

import { initSync } from '../crates/cal-core-wasm/pkg/cal_core_wasm';
import {
  compareNames,
  compareTitles,
  detectConferenceJson,
  findGroupSuggestionsJson,
  suggestGroupMateJson,
  withoutDuplicateMeetingsJson,
  findMeetingLinkPairsJson,
  normalizeJoinUrlThroughCore,
  isImportantPriority,
  markCoreRulesReady,
  normalPriority,
  priorityRank,
} from './wasm/coreRules';

initSync({
  module: readFileSync(
    resolve(process.cwd(), 'crates/cal-core-wasm/pkg/cal_core_wasm_bg.wasm'),
  ),
});
markCoreRulesReady();

// The surface installs its door into `cal_core::collation` at startup; the
// test environment installs the same one. German, because the suite asserts
// the German UI.
installTextCollation({
  compareNames: (a, b) => compareNames(a, b, 'de'),
  compareTitles: (a, b) => compareTitles(a, b, 'de'),
});

// The same door the surface installs, for the same reason: the ranking lives
// in `cal_core::task_priority` and the tests must exercise it, not a stand-in.
installTaskPriorityRules({
  priorityRank,
  isImportantPriority,
  normalPriority,
});

installConferenceDetector({ detectConferenceJson });
installGroupSuggestionRules({ findGroupSuggestionsJson, suggestGroupMateJson });
installMeetingDuplicateFilter({ withoutDuplicateMeetingsJson });
installMeetingLinkRules({
  findMeetingLinkPairsJson,
  normalizeJoinUrl: normalizeJoinUrlThroughCore,
});
