/**
 * The desktop's synchronous door into `cal-core`.
 *
 * Every other surface already has one. Mobile calls Rust through a synchronous
 * Expo `Function(...)`; a native frontend links Rust and calls it. The desktop
 * runs its UI in a webview, and the only road from a webview to the Tauri
 * process is `invoke` — IPC, and therefore always asynchronous. A React render
 * cannot await, and `priorityRank` is called inside `Array.prototype.sort`
 * comparators, which cannot await at all. So a rule needed during render can
 * only live in the core if the core can be reached synchronously.
 *
 * WebAssembly is that road: the same Rust compiled into the webview.
 *
 * # Loading is async ONCE; calling is synchronous forever after
 *
 * `initCoreRules()` must be awaited before the first render — see `main.tsx`.
 * Browsers refuse to compile a module this size synchronously on the main
 * thread, so there is no way around the one await, and no reason to want one:
 * it happens while the app is starting anyway. Afterwards every call below is
 * an ordinary function call with no promise in sight.
 *
 * Calling before the module is ready throws rather than returning a wrong
 * answer. A silent fallback here would be a second implementation of the rule,
 * which is the thing this exists to remove.
 */
import init, {
  compareNames as wasmCompareNames,
  detectConference as wasmDetectConference,
  findGroupSuggestions as wasmFindGroupSuggestions,
  suggestGroupMate as wasmSuggestGroupMate,
  withoutDuplicateMeetings as wasmWithoutDuplicateMeetings,
  findMeetingLinkPairs as wasmFindMeetingLinkPairs,
  normalizeJoinUrl as wasmNormalizeJoinUrl,
  compareTitles as wasmCompareTitles,
  isImportantPriority as wasmIsImportantPriority,
  normalPriority as wasmNormalPriority,
  priorityRank as wasmPriorityRank,
} from '../../crates/cal-core-wasm/pkg/cal_core_wasm';

import type { PriorityScale, TaskPriority } from '@aperio/shared';

let ready = false;

/** Compile and instantiate the core rules. Idempotent; await once at startup. */
export async function initCoreRules(): Promise<void> {
  if (ready) return;
  await init();
  ready = true;
}

/**
 * Declare the module already instantiated.
 *
 * For the test environment only, which compiles it synchronously with
 * `initSync` (Node allows that at any size; browsers do not, which is why the
 * app awaits instead). Exported rather than inferred so there is exactly one
 * place that can lie about readiness, and it says so.
 */
export function markCoreRulesReady(): void {
  ready = true;
}

function assertReady(): void {
  if (!ready) {
    throw new Error(
      'core rules used before initCoreRules() resolved — await it in the app bootstrap',
    );
  }
}

/** See `cal_core_wasm::priority_rank`. Synchronous: safe inside a comparator. */
export function priorityRank(
  priority: TaskPriority,
  scale: PriorityScale = 'three',
): number {
  assertReady();
  return wasmPriorityRank(priority, scale);
}

/** See `cal_core_wasm::is_important_priority`. */
export function isImportantPriority(priority: TaskPriority): boolean {
  assertReady();
  return wasmIsImportantPriority(priority);
}

/** See `cal_core_wasm::normal_priority`. */
export function normalPriority(
  previous?: TaskPriority | null,
): TaskPriority {
  assertReady();
  return wasmNormalPriority(previous ?? undefined) as TaskPriority;
}

/** See `cal_core::compare_names`. Synchronous: safe inside a comparator. */
export function compareNames(a: string, b: string, languageTag: string): number {
  assertReady();
  return wasmCompareNames(a, b, languageTag);
}

/** See `cal_core::compare_titles`. Synchronous: safe inside a comparator. */
export function compareTitles(
  a: string,
  b: string,
  languageTag: string,
): number {
  assertReady();
  return wasmCompareTitles(a, b, languageTag);
}

/** See `cal_core::conferencing::detect_conference_json`. JSON in, JSON out;
 *  synchronous, so a render can ask. */
export function detectConferenceJson(sourcesJson: string): string {
  assertReady();
  return wasmDetectConference(sourcesJson);
}

/** See `cal_core::group_suggestion::find_group_suggestions_json`. JSON in,
 *  positions out; synchronous, because the caller asks during a render. */
export function findGroupSuggestionsJson(inputJson: string): string {
  assertReady();
  return wasmFindGroupSuggestions(inputJson);
}

/** See `cal_core::group_suggestion::suggest_group_mate_json`. */
export function suggestGroupMateJson(inputJson: string): string {
  assertReady();
  return wasmSuggestGroupMate(inputJson);
}

/** See `cal_core::meeting_events::without_duplicate_meetings_json`. The whole
 *  window in, the positions that survive out. */
export function withoutDuplicateMeetingsJson(eventsJson: string): string {
  assertReady();
  return wasmWithoutDuplicateMeetings(eventsJson);
}

/** See `cal_core::meeting_link_grouping::find_meeting_link_pairs_json`. */
export function findMeetingLinkPairsJson(inputJson: string): string {
  assertReady();
  return wasmFindMeetingLinkPairs(inputJson);
}

/** See `cal_core::normalize_join_url`. */
export function normalizeJoinUrlThroughCore(url: string): string {
  assertReady();
  return wasmNormalizeJoinUrl(url);
}
