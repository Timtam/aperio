// Which appointments of an import list were created from the editor
// (DESIGN-file-import.md, I5). The desktop hands the editor a callback; a
// navigation route on the phone carries only data, so the list hands over a
// token and the row's place in the file, and the editor tells them here once
// the appointment is created. A list that is gone has no listener left: the
// word is dropped.

type Listener = (index: number, inPart?: boolean) => void;

const listeners = new Map<string, Set<Listener>>();

/** Tell the list behind `token` that its row `index` was created —
 *  `inPart`: it may stand only in part, and is not offered again either. */
export function markImportedFromEditor(token: string, index: number, inPart = false): void {
  for (const listener of listeners.get(token) ?? []) listener(index, inPart);
}

/** Hear which rows of the list behind `token` the editor created, until the
 *  returned function is called. */
export function onImportedFromEditor(token: string, listener: Listener): () => void {
  let set = listeners.get(token);
  if (!set) {
    set = new Set();
    listeners.set(token, set);
  }
  set.add(listener);
  return () => {
    set.delete(listener);
    if (set.size === 0) listeners.delete(token);
  };
}

let issued = 0;

/** A token no other list on this device holds while the app runs. */
export function newImportToken(): string {
  issued += 1;
  return `import-${Date.now().toString(36)}-${issued}`;
}
