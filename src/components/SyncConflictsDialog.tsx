import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { listen } from '@tauri-apps/api/event';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  listSyncConflicts,
  resolveSyncConflict,
  type SyncConflict,
  type SyncResolutionChoice,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { Modal } from './Modal';

/**
 * Conflict-resolution dialog for the cross-device sync layer
 * (DESIGN.md §19.3, Phase Sh + Si).
 *
 * Loads the list of unresolved conflicts on open + on every
 * `sync-conflicts-changed` Tauri event (the backend emits this
 * after the applier records a new conflict and after the user
 * resolves one).
 *
 * Each row shows:
 *   - Row kind + id (so the user knows which item is in conflict)
 *   - Field name (the differing column)
 *   - Both candidate values, JSON-decoded for display
 *   - A timestamp + device id for the remote edit
 *
 * Three resolution buttons per row, matching §19.3:
 *   - Meine Version behalten (keep_local)
 *   - Andere Version nehmen (take_remote)
 *   - Beide speichern (save_both) — backend rejects this with
 *     `unsupported` for now; the button surfaces a polite message
 *     and stays clickable so the UX is consistent once Sh's
 *     follow-up lands the fork logic.
 *
 * Resolutions fire optimistically: the row vanishes from the list
 * the moment the user clicks, and the next
 * `sync-conflicts-changed` event re-syncs the canonical state.
 */
export interface SyncConflictsDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

export function SyncConflictsDialog({
  isOpen,
  onClose,
}: SyncConflictsDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  const [busy, setBusy] = useState<number | null>(null);
  const listRef = useRef<HTMLUListElement>(null);

  const refresh = useCallback(() => {
    listSyncConflicts()
      .then(setConflicts)
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('list_sync_conflicts failed', err);
      });
  }, []);

  // Initial load + subscribe to backend changes. The dialog stays
  // mounted while open, so a refresh handler attaches once per
  // open cycle.
  useEffect(() => {
    if (!isOpen) return undefined;
    refresh();
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    listen('sync-conflicts-changed', () => {
      refresh();
    })
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('listen sync-conflicts-changed failed', err);
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [isOpen, refresh]);

  const resolve = useCallback(
    async (conflict: SyncConflict, choice: SyncResolutionChoice) => {
      setBusy(conflict.id);
      // Optimistic remove: drop the row immediately so the user
      // sees their click land. The `sync-conflicts-changed` event
      // re-syncs the truth in case of an error.
      setConflicts((prev) => prev.filter((c) => c.id !== conflict.id));
      // Re-park focus on the listbox so the next row (or empty
      // state) is reachable without the user having to hunt for
      // focus. The button that was just clicked is unmounting
      // along with its row, so without an explicit move the
      // browser drops focus to <body> and NVDA loses context.
      requestAnimationFrame(() => {
        listRef.current?.focus({ preventScroll: true });
      });
      try {
        await resolveSyncConflict(conflict.id, choice);
        announce(t('syncConflicts.resolved'));
      } catch (err: unknown) {
        // eslint-disable-next-line no-console
        console.warn('resolve_sync_conflict failed', err);
        // Surface the backend's code if it's the "save_both not
        // implemented" branch — the user clicked, we owe them a
        // hint.
        if (
          typeof err === 'object' &&
          err !== null &&
          'code' in err &&
          (err as { code?: string }).code === 'unsupported'
        ) {
          announce(t('syncConflicts.actionSaveBothUnsupported'), 'assertive');
        }
        // Put the row back so the user can try again.
        refresh();
      } finally {
        setBusy(null);
      }
    },
    [announce, refresh, t],
  );

  const intro =
    conflicts.length === 1
      ? t('syncConflicts.intro_one')
      : t('syncConflicts.intro_other', { count: conflicts.length });

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('syncConflicts.title')}
      className="sync-conflicts-dialog"
    >
      {/* Modal's body lives in `role="application"` for the
          dialog focus-mode trick (see Modal.tsx), so prose
          paragraphs need to be focusable for NVDA's
          arrow-navigation. FocusableNote wires the text as the
          element's accessible name so the screen reader reads
          the actual content instead of "Anmerkung". */}
      <FocusableNote className="sync-conflicts__intro">
        {conflicts.length === 0 ? t('syncConflicts.empty') : intro}
      </FocusableNote>
      {conflicts.length > 0 && (
        // The list itself is a tab-stop landing target — after
        // the user resolves a row, we re-park focus on the
        // <ul> so they can keep walking the remaining rows
        // without the browser dropping focus to <body>.
        <ul
          ref={listRef}
          tabIndex={-1}
          className="sync-conflicts__list"
          aria-label={t('syncConflicts.listLabel')}
        >
          {conflicts.map((c) => (
            <ConflictRow
              key={c.id}
              conflict={c}
              fmt={fmt}
              t={t}
              busy={busy === c.id}
              onResolve={resolve}
            />
          ))}
        </ul>
      )}
    </Modal>
  );
}

interface ConflictRowProps {
  conflict: SyncConflict;
  fmt: ReturnType<typeof useDateFormat>;
  t: ReturnType<typeof useTranslation>['t'];
  busy: boolean;
  onResolve: (c: SyncConflict, choice: SyncResolutionChoice) => void;
}

function ConflictRow({
  conflict,
  fmt,
  t,
  busy,
  onResolve,
}: ConflictRowProps) {
  const kindLabel = t(`syncConflicts.rowKind.${conflict.row_kind}`);
  const remoteTime = (() => {
    try {
      return fmt.format(new Date(conflict.remote_timestamp), 'PPPp');
    } catch {
      return conflict.remote_timestamp;
    }
  })();
  const headingId = useId();
  const saveBothHintId = useId();
  // Per-row context for the three resolution buttons. Without
  // this every row's "Keep my version" button reads identically
  // — three rows × three buttons = nine generic labels. Threading
  // the field + kind into each button's accessible name lets SR
  // users know which conflict they're acting on without
  // back-arrowing to the row heading.
  const ariaContext = t('syncConflicts.actionAriaContext', {
    kind: kindLabel,
    field: conflict.field,
  });
  return (
    <li
      className="sync-conflict-row"
      // Group semantics: the buttons inside read as part of a
      // discrete conflict group whose accessible name is the
      // heading. NVDA announces "Konflikt-Gruppe: Termin, Feld
      // title" once on entry rather than re-reading the
      // heading for each button.
      role="group"
      aria-labelledby={headingId}
      aria-busy={busy || undefined}
    >
      <div
        id={headingId}
        className="sync-conflict-row__heading"
      >
        <strong>{kindLabel}</strong>
        <span className="sync-conflict-row__field">
          {t('syncConflicts.fieldLabel')}: {conflict.field}
        </span>
      </div>
      <p className="sync-conflict-row__source">
        {t('syncConflicts.remoteSourceLabel', {
          time: remoteTime,
          device: conflict.remote_device_id,
        })}
      </p>
      <div className="sync-conflict-row__values">
        <div>
          <span className="sync-conflict-row__valueLabel">
            {t('syncConflicts.localValueLabel')}
          </span>
          <code>{decodeForDisplay(conflict.local_value)}</code>
        </div>
        <div>
          <span className="sync-conflict-row__valueLabel">
            {t('syncConflicts.remoteValueLabel')}
          </span>
          <code>{decodeForDisplay(conflict.remote_value)}</code>
        </div>
      </div>
      <div className="sync-conflict-row__actions">
        <button
          type="button"
          disabled={busy}
          aria-label={`${t('syncConflicts.actionKeepLocal')} — ${ariaContext}`}
          onClick={() => onResolve(conflict, 'keep_local')}
        >
          {t('syncConflicts.actionKeepLocal')}
        </button>
        <button
          type="button"
          disabled={busy}
          aria-label={`${t('syncConflicts.actionTakeRemote')} — ${ariaContext}`}
          onClick={() => onResolve(conflict, 'take_remote')}
        >
          {t('syncConflicts.actionTakeRemote')}
        </button>
        <button
          type="button"
          disabled={busy}
          aria-label={`${t('syncConflicts.actionSaveBoth')} — ${ariaContext}`}
          // Use aria-describedby (not title) for the "not yet
          // implemented" hint so SR users on platforms that
          // ignore title still get the message.
          aria-describedby={saveBothHintId}
          onClick={() => onResolve(conflict, 'save_both')}
        >
          {t('syncConflicts.actionSaveBoth')}
        </button>
        <span id={saveBothHintId} className="sr-only">
          {t('syncConflicts.actionSaveBothUnsupported')}
        </span>
      </div>
    </li>
  );
}

/** Decode the JSON-encoded backend value into something readable.
 *  Numbers/booleans stringify naturally; quoted strings get
 *  unquoted; null → em-dash so the empty case is visible.
 *  Everything else falls back to the raw JSON. */
function decodeForDisplay(raw: string | null): string {
  if (raw === null) return '—';
  try {
    const parsed = JSON.parse(raw);
    if (parsed === null || parsed === undefined) return '—';
    if (typeof parsed === 'string') return parsed;
    if (typeof parsed === 'number' || typeof parsed === 'boolean') {
      return String(parsed);
    }
    return JSON.stringify(parsed);
  } catch {
    return raw;
  }
}
