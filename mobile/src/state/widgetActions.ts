import type { Task, TaskList } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import { getTasks, listTaskLists } from '../api/client';
import { setTaskStatusTo } from './taskToggle';

// The app's end of the widget's action queue.
//
// A widget button cannot complete a task itself. Completion in Aperio cascades
// to parents and children, self-assigns on shared lists, advances a recurring
// series, appends to the event log and queues a sync push — the app's rules over
// the Rust core, none of it reachable from an extension process with a few tens
// of megabytes and no bridge.
//
// So the widget records the REQUEST and this drains it, through
// `setTaskStatusTo` — the same call the day-start review's "Mark done" uses.
// Re-implementing completion here to save a fan-out would be how the widget and
// the app quietly start disagreeing about what "done" means.

/** One queued tap, as `WidgetActionStore` hands it over. */
interface WidgetAction {
  /** The queue file's name, for clearing exactly this one. */
  id: string;
  version: number;
  action: string;
  itemId: string;
  containerId: string;
  at: string;
}

/** The shape this build understands. An action from a newer widget is dropped
 *  rather than guessed at — app and extension update together, but the queue
 *  survives the update between them. */
const SUPPORTED_VERSION = 1;

async function readPending(): Promise<WidgetAction[]> {
  try {
    const raw = JSON.parse(await CalFfi.pendingWidgetActionsJson()) as WidgetAction[];
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
export function consumeWidgetActionsApplied(): boolean {
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
export async function drainWidgetActions(): Promise<boolean> {
  if (draining) return false;
  draining = true;
  try {
    const pending = await readPending();
    if (pending.length === 0) return false;

    // Loaded once for the whole batch: `setTaskStatusTo` needs the full task set
    // to plan the cascade, and the batch is normally one or two taps.
    let lists: TaskList[] = [];
    let allTasks: Task[] = [];
    try {
      lists = await listTaskLists();
      allTasks = (
        await Promise.all(lists.map((l) => getTasks(l.id).catch(() => [] as Task[])))
      ).flat();
    } catch {
      // Could not read the catalogue at all — leave the queue for the next pass
      // rather than clearing taps nothing was even attempted for.
      return false;
    }
    const byId = new Map(allTasks.map((t) => [t.id, t]));
    const listById = new Map(lists.map((l) => [l.id, l]));

    let applied = false;
    for (const action of pending) {
      try {
        if (action.version === SUPPORTED_VERSION && action.action === 'complete') {
          const task = byId.get(action.itemId);
          // Already gone, or already completed elsewhere: both are the state the
          // user asked for, so neither is a failure.
          if (task != null && task.status !== 'completed') {
            await setTaskStatusTo(task, 'completed', listById.get(task.list_id), allTasks);
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
