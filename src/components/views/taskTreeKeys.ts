/**
 * Key-handling decisions for the task tree, kept in their own module so they
 * stay pure and unit-testable (and so the view component file keeps a single
 * component export for Fast Refresh).
 */

/**
 * On a focused **group-header** row (Backlog / a list / a section / the
 * synthetic Done or Zukünftig group), task-only shortcuts are inert — they
 * must never act on the synthetic group row. We suppress a plain key's
 * (harmless) browser default to keep the row quiet, but an OS / global
 * shortcut must reach the window: anything carrying a system modifier
 * (`Alt+F4`, `Ctrl+R`, `Meta+…`) and `Tab` (focus move) pass through
 * untouched.
 *
 * Returns `true` when the key should be suppressed (`preventDefault`), `false`
 * when it must pass through.
 */
export function suppressGroupHeaderKey(e: {
  key: string;
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
}): boolean {
  return e.key !== 'Tab' && !e.altKey && !e.ctrlKey && !e.metaKey;
}
