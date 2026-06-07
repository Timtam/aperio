import { useEffect } from 'react';

/**
 * Suppress browser shortcuts that don't belong in a native-feeling app.
 *
 * DESIGN.md section 15.2 calls these out specifically:
 *  - Right-click context menu → handled by our own native menu plugin
 *    (Phase 4); the browser version must not show up.
 *  - `Ctrl+R` / `F5` → page reload. `Ctrl+R` is repurposed for "sync now"
 *    in section 15.7. `F5` is wired to nothing.
 *  - `Ctrl+U` → view source. No use case here.
 *  - `Ctrl+P` → browser print dialog. The print flow (DESIGN.md section
 *    25) will use our own dialog.
 *  - `dragstart` on non-draggable surfaces → the browser tries to start a
 *    selection drag, which feels nothing like a desktop app.
 *
 * **Editable targets are exempt.** Inside `<input>`, `<textarea>`, or
 * `contenteditable`, shortcuts like `Ctrl+A` and the system context menu
 * must still work — otherwise text editing breaks. The check is done per
 * event, not per listener, so we can attach a single global listener.
 *
 * The hook returns the cleanup automatically through React's effect.
 */
export function useSuppressBrowserDefaults(): void {
  useEffect(() => {
    const onContextMenu = (e: MouseEvent) => {
      if (isEditableTarget(e.target)) return;
      e.preventDefault();
    };

    const onKeyDown = (e: KeyboardEvent) => {
      if (isEditableTarget(e.target)) return;

      const ctrlOrMeta = e.ctrlKey || e.metaKey;
      // F5 / Ctrl+R: reload — never useful here.
      if (e.key === 'F5' || (ctrlOrMeta && e.key.toLowerCase() === 'r')) {
        e.preventDefault();
        // Re-emit Ctrl+R as the app-level "sync now" intent.
        // The shortcut system (Phase 5) will pick this up; until then it
        // is a no-op rather than a page reload.
        return;
      }
      // Ctrl+U: view source.
      if (ctrlOrMeta && e.key.toLowerCase() === 'u') {
        e.preventDefault();
        return;
      }
      // Ctrl+P: browser print.
      if (ctrlOrMeta && e.key.toLowerCase() === 'p') {
        e.preventDefault();
        return;
      }
    };

    const onDragStart = (e: DragEvent) => {
      // Allow drag from elements that explicitly opt in: anything
      // `draggable="true"` (the native opt-in every app chip already sets —
      // backlog tasks, event/task chips, sidebar rows) or inside a
      // `[data-drag-source]` container. Everything else (selected text, the
      // links/images the browser would drag by default) must not start a
      // drag — that feels nothing like a desktop app.
      const target = e.target as HTMLElement | null;
      if (target?.closest('[draggable="true"], [data-drag-source]')) {
        return;
      }
      e.preventDefault();
    };

    window.addEventListener('contextmenu', onContextMenu);
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('dragstart', onDragStart);
    return () => {
      window.removeEventListener('contextmenu', onContextMenu);
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('dragstart', onDragStart);
    };
  }, []);
}

function isEditableTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  if (tag === 'input' || tag === 'textarea' || tag === 'select') return true;
  if (target.isContentEditable) return true;
  return false;
}
