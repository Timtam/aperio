/** Mirrors EventDialog's last-used-calendar memo. The task-list
 *  picker on a new task remembers the user's previous pick so a
 *  multi-list setup doesn't reset to `taskLists[0]` on every open. */
const LAST_USED_TASK_LIST_KEY = 'aperio.lastUsedTaskList.v1';

export function readLastUsedTaskList(): string | null {
  try {
    return localStorage.getItem(LAST_USED_TASK_LIST_KEY);
  } catch {
    return null;
  }
}

export function writeLastUsedTaskList(id: string): void {
  try {
    localStorage.setItem(LAST_USED_TASK_LIST_KEY, id);
  } catch {
    // Best effort.
  }
}
