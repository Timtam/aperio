// The coalescer behind the live reload: which categories a cache push touched,
// when they are flushed to the subscribers, and what waits for the end of a
// warm pass. Free of React and React Native so its timing can be tested on its
// own; `cacheObserver.ts` wires it to the native push and re-exports it.

/** Coarse category a cache scope belongs to — drives the announcement string +
 *  which screens reload. The Host scopes are events/calendars (→ calendar),
 *  tasks/task_lists (→ tasks), contacts/contact_lists (→ contacts), and
 *  sections (→ sections: the task store reads the lists it holds again — a
 *  section write invalidates them, and only the refresh brings the truth). */
export type CacheCategory = 'calendar' | 'tasks' | 'contacts' | 'sections';

function categoryForScope(scope: string): CacheCategory | null {
  switch (scope) {
    case 'events':
    case 'calendars':
      return 'calendar';
    case 'tasks':
    case 'task_lists':
      return 'tasks';
    case 'contacts':
    case 'contact_lists':
      return 'contacts';
    case 'sections':
      return 'sections';
    default:
      return null;
  }
}

// Module-level bus: the root observer fans coalesced category notifications out
// to whichever screens are subscribed (via useCacheReload).
type BusListener = (category: CacheCategory) => void;
const busListeners = new Set<BusListener>();

// Reminder-scheduler hook — REGISTERED by the scheduler at its module init
// (a direct import here would close a module cycle through the api layer).
// Invoked whenever data lands that can change what should be scheduled: a
// peer sync round applied changes, or an external-cache refresh finished.
// The scheduler defers its scan to the end of a running pass itself.
let remindersRefreshHook: (() => void) | null = null;
export function setRemindersRefreshHook(cb: () => void): void {
  remindersRefreshHook = cb;
}

/**
 * Non-hook subscription for a MODULE-LEVEL store (a screen uses
 * `useCacheReload`). A store that hydrates once and holds its value in memory
 * — the signature list — has to hear about a sync round that rewrote its row,
 * or every consumer (editors included) keeps the pre-round value until the
 * next launch. Returns the unsubscribe.
 */
export function subscribeCacheReload(category: CacheCategory, cb: () => void): () => void {
  return subscribeBus((cat) => {
    if (cat === category) cb();
  });
}

function subscribeBus(cb: BusListener): () => void {
  busListeners.add(cb);
  return () => {
    busListeners.delete(cb);
  };
}

/** What a screen subscription needs of the screen it is on: react-navigation's
 *  `navigation` object. Outside a screen (the app root's stores) there is none. */
export interface ScreenFocus {
  isFocused(): boolean;
  addListener(event: 'focus', cb: () => void): () => void;
}

/**
 * A screen's live reload. On the focused screen every flush of `category`
 * reloads at once. A screen that is mounted but not in view (another tab, an
 * editor on top of it) only notes the flush and reloads once on its next
 * focus: during an account refresh every flush used to reload every mounted
 * screen, and all those reads queued in front of the one the user was
 * looking at. Without a `screen` (a store at the app root) every flush
 * reloads. Returns the unsubscribe.
 */
export function subscribeScreenReload(
  category: CacheCategory,
  reload: () => void,
  screen: ScreenFocus | undefined,
): () => void {
  let missed = false;
  const offBus = subscribeBus((cat) => {
    if (cat !== category) return;
    if (screen != null && !screen.isFocused()) {
      missed = true;
      return;
    }
    reload();
  });
  const offFocus = screen?.addListener('focus', () => {
    if (!missed) return;
    missed = false;
    reload();
  });
  return () => {
    offBus();
    offFocus?.();
  };
}

/** Coalesce window: a burst of per-container events (no warm pass running)
 *  waits this long after the FIRST event before reloading once per category. */
export const COALESCE_MS = 700;

/** Max flush cadence while a backend warm pass is in flight. A pass spreads
 *  its per-container emissions over seconds, which used to defeat the short
 *  coalesce window and turn one pass into many full reload waves — each
 *  re-exposing whatever intermediate cache state existed at that moment (the
 *  app-start entry-count oscillation). The pass-end status flushes
 *  immediately, so the settled state paints exactly once; a hung pass can't
 *  starve the UI because the throttle keeps its own cadence. */
export const PASS_THROTTLE_MS = 2500;

/** The longest work deferred to the end of a pass waits: a pass that never
 *  reports its end must not keep the badge, the widget and the reminders on
 *  their old state for good. */
export const SETTLE_CAP_MS = 30_000;

// Module-level coalescer state, shared by the native listeners and
// notifyDataReload so EVERY reload producer rides the same gating.
const pendingCategories = new Set<CacheCategory>();
let flushTimer: ReturnType<typeof setTimeout> | null = null;
let passRefreshing = false;

// What waits for the end of the running pass (see whenPassSettled), once each.
const settledWork = new Set<() => void>();
let settleCapTimer: ReturnType<typeof setTimeout> | null = null;

function flushPending(): void {
  flushTimer = null;
  if (pendingCategories.size === 0) return;
  const categories = Array.from(pendingCategories);
  pendingCategories.clear();
  for (const cat of categories) busListeners.forEach((l) => l(cat));
  // Fresh data can change reminder-relevant state (provider-side edits,
  // peer-synced tasks) — re-plan the scheduled OS notifications. Coalesced
  // here + debounced in the scheduler, so a warm pass costs one reschedule.
  remindersRefreshHook?.();
}

// Collect-then-flush: the FIRST category of a window arms the timer; later
// ones just accumulate (no per-event reset, so a drip of events can't
// postpone the flush forever and latency stays bounded).
function scheduleFlush(): void {
  if (flushTimer != null) return;
  flushTimer = setTimeout(
    flushPending,
    passRefreshing ? PASS_THROTTLE_MS : COALESCE_MS,
  );
}

/** A native `cache_updated` push for `scope`: collected, then flushed. */
export function noteCacheUpdated(scope: string): void {
  const category = categoryForScope(scope);
  if (category == null) return;
  pendingCategories.add(category);
  scheduleFlush();
}

/**
 * The warm pass started (`true`) or ended (`false`). At the end the settled
 * state paints NOW instead of waiting out a throttle window armed mid-pass,
 * and then what waited for the end runs, each piece once. The flush still
 * counts as part of the pass, so work it defers joins that one run instead
 * of running twice.
 */
export function notePassRefreshing(refreshing: boolean): void {
  if (refreshing === passRefreshing) return;
  if (refreshing) {
    passRefreshing = true;
    return;
  }
  if (flushTimer != null) clearTimeout(flushTimer);
  flushPending();
  passRefreshing = false;
  runSettledWork();
}

/**
 * Run `work` now, or, while a warm pass runs, once when it ends. For what
 * nobody looks at during a pass and that would otherwise redo itself with
 * every flush: the app badge, the widget, the reminder scan. A `work` deferred
 * twice runs once; pass a stable function (see `useWhenPassSettled`). Runs at
 * the latest `SETTLE_CAP_MS` after it was first deferred, or when the app
 * leaves the foreground (`runSettledWork`).
 */
export function whenPassSettled(work: () => void): void {
  if (!passRefreshing) {
    work();
    return;
  }
  settledWork.add(work);
  settleCapTimer ??= setTimeout(runSettledWork, SETTLE_CAP_MS);
}

/** Drop `work` from what waits for the end of the pass (its owner unmounted). */
export function cancelWhenPassSettled(work: () => void): void {
  settledWork.delete(work);
}

/** Run what waits for the end of the pass now: the pass ended, the cap ran
 *  out, or the app is leaving the foreground, where iOS suspends the JS and
 *  the widget is about to be looked at. */
export function runSettledWork(): void {
  if (settleCapTimer != null) clearTimeout(settleCapTimer);
  settleCapTimer = null;
  const due = Array.from(settledWork);
  settledWork.clear();
  for (const work of due) work();
}

/**
 * Ask every screen subscribed via `useCacheReload` to reload, across ALL
 * categories. Use after a CROSS-DEVICE sync round (or onboarding) applies a
 * peer's data: that path writes straight to the local store and never goes
 * through the Host's external `onCacheUpdated` push, so without this the open
 * screens stay stale until the app restarts. Silent — it's a data reload, not
 * the external-refresh cue. Routed through the SAME coalescer as the native
 * push, so a sync round landing mid-warm-pass can't bypass the reload gating.
 */
export function notifyDataReload(): void {
  pendingCategories.add('calendar');
  pendingCategories.add('tasks');
  pendingCategories.add('contacts');
  pendingCategories.add('sections');
  // Immediate flush: the sync UI just reported applied changes (or the user
  // pulled to refresh) — waiting out a pass throttle here reads as "sync
  // done but the screen is stale". Going through the shared flush still
  // clears `pendingCategories`, so a running pass's own emissions aren't
  // replayed on top.
  if (flushTimer != null) clearTimeout(flushTimer);
  flushPending();
}

/** True unmount of the root observer (app teardown): flush whatever is
 *  pending so no subscriber is left stale, then disarm. */
export function endCacheUpdates(): void {
  if (flushTimer != null) clearTimeout(flushTimer);
  flushPending();
  passRefreshing = false;
  runSettledWork();
}
