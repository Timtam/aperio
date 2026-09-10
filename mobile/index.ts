import { registerRootComponent } from 'expo';

import {
  installConferenceDetector,
  installGroupSuggestionRules,
  installMeetingDuplicateFilter,
  installTaskPriorityRules,
  installTextCollation,
  type TaskPriority,
} from '@aperio/shared';

// Initialise i18next (shared translations) before the app renders.
import i18n from './i18n';
import CalFfi from './modules/cal-ffi';
import App from './src/App';

// The core's text-ordering rule, reached through a SYNCHRONOUS bridge call
// (`Function`, not `AsyncFunction`) because its callers are `Array.sort`
// comparators. Installed before the first render for the same reason the
// desktop awaits its WebAssembly module at startup — see DESIGN §4.3 for the
// door, §4.4 for the rule that goes through it.
//
// Bound to the language the USER chose. Every `localeCompare` this replaces
// passed `undefined` and so followed the DEVICE, which is why a German list
// could order differently on a phone than on the desktop.
const installCollation = () =>
  installTextCollation({
    compareNames: (a, b) => CalFfi.compareNames(a, b, i18n.language),
    compareTitles: (a, b) => CalFfi.compareTitles(a, b, i18n.language),
  });
installCollation();
i18n.on('languageChanged', installCollation);

// The priority ranking, from `cal_core::task_priority` through the same
// synchronous bridge. No language in it, so it is installed once and never
// re-bound — unlike the collation above.
//
// Until this existed the rule was reachable from the DESKTOP only (it lived in
// `crates/cal-core-wasm`), so this app ran the TypeScript copy while the
// desktop ran Rust.
installTaskPriorityRules({
  priorityRank: (priority, scale) => CalFfi.priorityRank(priority, scale),
  isImportantPriority: (priority) => CalFfi.isImportantPriority(priority),
  // The bridge speaks strings — `''` is "nothing before", and the answer is
  // one of the three the core knows. The cast is the boundary, not a guess.
  normalPriority: (previous) =>
    CalFfi.normalPriority(previous ?? '') as TaskPriority,
});

// Finding the online meeting in an event — the same rule the desktop reaches
// through WebAssembly, here over the bridge. See `shared/conferencing.ts` for
// the 384 lines of TypeScript this replaces and the six ways the two had drifted.
installConferenceDetector({
  detectConferenceJson: (sourcesJson) => CalFfi.detectConference(sourcesJson),
});

// Recognising a copy — "these two rows look like one appointment". Also
// synchronous, and for the same reason as the collation: both callers ask
// during a render (`useMemo`), where nothing can await. See
// `shared/groupSuggestions.ts` for the two TypeScript rules this replaces.
installGroupSuggestionRules({
  findGroupSuggestionsJson: (inputJson) => CalFfi.findGroupSuggestions(inputJson),
  suggestGroupMateJson: (inputJson) => CalFfi.suggestGroupMate(inputJson),
});

// Hiding a provider-side meeting that already has a calendar entry. The whole
// window crosses at once — the rule this replaces asked the detection door
// twice per row.
installMeetingDuplicateFilter({
  withoutDuplicateMeetingsJson: (eventsJson) =>
    CalFfi.withoutDuplicateMeetings(eventsJson),
});

// registerRootComponent calls AppRegistry.registerComponent('main', () => App);
// It also ensures that whether you load the app in Expo Go or in a native build,
// the environment is set up appropriately
registerRootComponent(App);
