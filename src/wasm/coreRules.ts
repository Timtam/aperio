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
