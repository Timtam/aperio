import type { Task, TaskList } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import { createEvent, listCalendars } from '../api/calendar';
import { getTasks, listTaskLists } from '../api/client';
import { readLastUsedCalendar } from './lastUsedCalendar';
import { applyTaskToggle } from './taskToggle';

// The app's end of the action queue — requests other processes leave for it.
//
// Two write into it today: the home-screen widget's checkbox, and Siri via the
// app-target intents. Neither can do the work itself, and for the same reason.
//
// A widget button cannot complete a task itself. Completion in Aperio cascades
// to parents and children, self-assigns on shared lists, advances a recurring
// series, appends to the event log and queues a sync push — the app's rules over
// the Rust core, none of it reachable from an extension process with a few tens
// of megabytes and no bridge.
//
// So the widget records the REQUEST and this drains it, through
// `applyTaskToggle` — the one shared check-off path every other surface uses.
// That matters beyond tidiness: the check-off MODE is a synced setting, and
// under "cycle" one tap moves open → in progress → done rather than straight to
// done. Sending the widget's taps anywhere else would make it the one surface
// that ignores the user's own setting.

/** One queued request, as `WidgetActionStore` hands it over. */
interface QueuedAction {
  /** The queue file's name, for clearing exactly this one. */
  id: string;
  version: number;
  /** `toggle` — advance a task one step, whatever the check-off mode says that
   *  is. Deliberately not "complete": the caller asks for the same thing a tap
   *  in the app asks for, and the app decides what it means.
   *
   *  `createEvent` — make an event from a spoken title and time. */
  action: string;
  at: string;
  /** `toggle` only. */
  itemId?: string;
  containerId?: string;
  /** `createEvent` only. `startsAt` is RFC-3339, as Siri resolved it. */
  title?: string;
  startsAt?: string;
}

/** The shape this build understands. An action from a newer widget is dropped
 *  rather than guessed at — app and extension update together, but the queue
 *  survives the update between them. */
const SUPPORTED_VERSION = 1;

async function readPending(): Promise<QueuedAction[]> {
  try {
    const raw = JSON.parse(await CalFfi.pendingWidgetActionsJson()) as QueuedAction[];
    return Array.isArray(raw) ? raw : [];
  } catch {
    return [];
  }
}

// Set when a drain actually changed something, cleared when the app has been
// told. A module flag rather than a return value, because the pass that applies
// a tap is often the BACKGROUND one — no React alive to hear about it — and the
// views still have to learn of it when the app next comes forward. Without this,
// ticking on the lock screen overnight would leave the task showing as open in
// the app until something else happened to refetch.
let appliedUnseen = false;

/** Whether a drain has applied something since this was last called. Reading
 *  clears it. */
export function consumeQueuedActionsApplied(): boolean {
  const seen = appliedUnseen;
  appliedUnseen = false;
  return seen;
}

// One drain at a time. Unlike the snapshot's guard there is no `rerun`: the
// queue is drained to empty every pass, so a trigger arriving mid-drain has
// nothing left to do that this pass will not already have done.
let draining = false;

/**
 * Perform every queued widget tap, then clear it. Never throws.
 *
 * An action is cleared after being ATTEMPTED, not only after succeeding. A tap
 * that cannot be carried out — the task deleted meanwhile, its list gone — would
 * otherwise sit in the queue forever, and because the widget hides anything
 * queued, that row would stay invisible for good. A dropped tap is recoverable:
 * the task is still there in the app.
 *
 * Returns true when something was applied, so the caller knows the snapshot is
 * now out of date.
 */
export async function drainQueuedActions(): Promise<boolean> {
  if (draining) return false;
  draining = true;
  try {
    const pending = await readPending();
    if (pending.length === 0) return false;

    // The task catalogue is only needed to plan a check-off cascade, so it is
    // loaded only when a tap is actually queued — a spoken "new event" must not
    // pay for a full fan-out, nor be dropped because one failed.
    let lists: TaskList[] = [];
    let allTasks: Task[] = [];
    if (pending.some((a) => a.action === 'toggle')) {
      try {
        lists = await listTaskLists();
        allTasks = (
          await Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[])))
        ).flat();
      } catch {
        // Could not read the catalogue at all — leave the queue for the next
        // pass rather than clearing taps nothing was even attempted for.
        return false;
      }
    }
    const byId = new Map(allTasks.map((t) => [t.id, t]));
    const listById = new Map(lists.map((l) => [l.id, l]));

    let applied = false;
    for (const action of pending) {
      try {
        if (action.version === SUPPORTED_VERSION && action.action === 'toggle') {
          // `itemId` is optional on the union — a `toggle` without one is a
          // malformed entry, not a task that happens to be missing.
          const task = action.itemId != null ? byId.get(action.itemId) : undefined;
          // Gone, or already terminal: both are a state the tap cannot improve
          // on, so neither is a failure.
          if (task != null && task.status !== 'completed' && task.status !== 'cancelled') {
            // Reads the synced check-off mode fresh and applies the cascade —
            // exactly what a tap on the row in the app would have done.
            const next = await applyTaskToggle(task, listById.get(task.list_id), allTasks);
            if (next != null) {
              applied = true;
              appliedUnseen = true;
            }
          }
        } else if (action.version === SUPPORTED_VERSION && action.action === 'createEvent') {
          if (await createSpokenEvent(action)) {
            applied = true;
            appliedUnseen = true;
          }
        }
      } catch {
        // Attempted and failed. Cleared below all the same — see the note above.
      }
      await CalFfi.clearWidgetAction(action.id).catch(() => undefined);
    }
    return applied;
  } catch {
    return false;
  } finally {
    draining = false;
  }
}

/** How long a spoken event runs. Siri gives a moment, not a span — there is no
 *  duration in the phrase and asking for one would make every dictation two
 *  questions longer. An hour is the ordinary appointment; the editor is one tap
 *  away for the rest. */
const SPOKEN_EVENT_MINUTES = 60;

/**
 * Create the event a voice request asked for.
 *
 * The calendar is the last one used — the same default the editor offers, so a
 * spoken event lands where a typed one would. Falls back to the first writable
 * calendar, because "no calendar chosen yet" must not swallow the request.
 */
async function createSpokenEvent(action: QueuedAction): Promise<boolean> {
  const title = action.title?.trim();
  const start = action.startsAt ? new Date(action.startsAt) : null;
  if (!title || start == null || Number.isNaN(start.getTime())) return false;

  const calendars = await listCalendars();
  const writable = calendars.filter((c) => !c.read_only);
  if (writable.length === 0) return false;
  const preferred = await readLastUsedCalendar().catch(() => null);
  const target =
    writable.find((c) => c.id === preferred) ?? writable[0];
  if (target == null) return false;

  await createEvent({
    calendar_id: target.id,
    title,
    description: null,
    location: null,
    start: start.toISOString(),
    end: new Date(start.getTime() + SPOKEN_EVENT_MINUTES * 60_000).toISOString(),
    all_day: false,
    recurrence: null,
    color_label: null,
    reminders: [],
    sound: null,
    attendees: [],
  });
  return true;
}
