// Imperative bridge for the shared event-scope chooser. The calendar surfaces
// call confirmDeleteEvent / editEventWithScope from row handlers with no dialog
// state of their own (exactly as they did with a native Alert); those push a
// config here, and the single <EventScopeDialogHost /> mounted at the app root
// renders it. Kept separate from the host COMPONENT so the component file only
// exports a component (Fast Refresh requirement).

export interface EventScopeOption {
  key: string;
  label: string;
  /** Danger styling for a delete/cancel action (never for a plain edit scope). */
  destructive?: boolean;
  /** Run the action. `sendCancellations` is the notify-radio value when the
   *  dialog shows one, else always false. */
  run: (sendCancellations: boolean) => void;
}

export interface EventScopeDialogConfig {
  title: string;
  message?: string;
  /** When present, a notify/silent radio (default: notify) sits above the
   *  options and its value is passed to each option's run(). Absent => run(false). */
  notify?: { legend: string; notifyLabel: string; silentLabel: string };
  options: EventScopeOption[];
  cancelLabel: string;
}

let emit: ((cfg: EventScopeDialogConfig) => void) | null = null;

/** Open the shared event-scope dialog. No-op if the host isn't mounted. */
export function showEventScopeDialog(cfg: EventScopeDialogConfig): void {
  emit?.(cfg);
}

/** The host registers its open-handler here; returns an unsubscribe. */
export function registerEventScopeDialog(
  fn: (cfg: EventScopeDialogConfig) => void,
): () => void {
  emit = fn;
  return () => {
    if (emit === fn) emit = null;
  };
}
