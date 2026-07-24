import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { listen } from '@tauri-apps/api/event';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';
import {
  getEventById,
  getTaskById,
  listCalendars,
  listColorLabels,
  listSyncConflicts,
  listTaskLists,
  resolveSyncConflict,
  type SyncConflict,
  type SyncResolutionChoice,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import {
  groupSyncConflicts,
  type ConflictGroup,
} from '@aperio/shared';
import { Modal } from './Modal';

/**
 * Conflict-resolution dialog for the cross-device sync layer
 * (DESIGN.md §19.3, Phase Sh + Si).
 *
 * The applier records ONE conflict per differing FIELD, so a task edited on two
 * devices surfaces as several rows (status + scheduled_date + completed_at …).
 * The dialog GROUPS them by owning row (task/event/…) so each item is ONE card
 * with a "resolve all of this item" action — you almost never want to keep 2 of
 * 3 fields from different devices — and resolves the raw UUID to the item's NAME
 * (via getTaskById / getEventById / the list APIs) so it reads for casual users.
 * The per-field values + per-field buttons stay under each card as the escape
 * hatch for the rare mixed resolution.
 *
 * Loads on open + on every `sync-conflicts-changed` Tauri event. Resolutions
 * fire optimistically (the row/group vanishes on click; the next event re-syncs
 * the canonical state).
 */
export interface SyncConflictsDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

/** Resolve each group's owning row to its human name (task/event title, list/
 *  calendar/label name). Returns a fresh `key → name` map; unresolved rows
 *  (deleted, or a kind we don't name) are simply absent and fall back to a short
 *  label in the UI. The per-id fetches are cached backend-side, so re-running on
 *  every list change is cheap. */
async function resolveConflictLabels(
  groups: ConflictGroup<SyncConflict>[],
): Promise<Map<string, string>> {
  const map = new Map<string, string>();
  const [lists, cals, labels] = await Promise.all([
    groups.some((g) => g.rowKind === 'task_list')
      ? listTaskLists().catch(() => [])
      : Promise.resolve([]),
    groups.some((g) => g.rowKind === 'calendar')
      ? listCalendars().catch(() => [])
      : Promise.resolve([]),
    groups.some((g) => g.rowKind === 'color_label')
      ? listColorLabels().catch(() => [])
      : Promise.resolve([]),
  ]);
  await Promise.all(
    groups.map(async (g) => {
      let name: string | undefined;
      try {
        if (g.rowKind === 'task') {
          name = (await getTaskById(g.rowId))?.title;
        } else if (g.rowKind === 'event') {
          name = (await getEventById(g.rowId))?.title;
        } else if (g.rowKind === 'task_list') {
          name = lists.find((l) => l.id === g.rowId)?.name;
        } else if (g.rowKind === 'calendar') {
          name = cals.find((c) => c.id === g.rowId)?.name;
        } else if (g.rowKind === 'color_label') {
          name = labels.find((l) => l.id === g.rowId)?.name;
        }
      } catch {
        // Deleted / unfetchable — leave it out; the UI shows the fallback.
      }
      if (name && name.trim()) map.set(g.key, name.trim());
    }),
  );
  return map;
}

export function SyncConflictsDialog({
  isOpen,
  onClose,
}: SyncConflictsDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  // False until the first listSyncConflicts() settles. Without it the dialog
  // rendered the "Keine Konflikte" copy against the initial empty array before
  // the load returned — the user heard "no conflicts", pressed Escape, and left
  // real conflicts unresolved.
  const [loaded, setLoaded] = useState(false);
  const [labelByKey, setLabelByKey] = useState<Map<string, string>>(new Map());
  // Group keys currently resolving — disables that card's buttons + marks it
  // aria-busy until the round-trip settles.
  const [busyKeys, setBusyKeys] = useState<Set<string>>(new Set());
  const listRef = useRef<HTMLUListElement>(null);
  // Stable id for the always-rendered intro note so reparkFocus can fall back
  // to it once the list <ul> has unmounted (the last conflict resolved).
  const introId = useId();
  // The intro note holds focus on open but reads the "loading" text; once the
  // first load settles its text changes, but a focused element isn't re-read —
  // so announce the real count/empty state exactly once.
  const announcedLoadRef = useRef(false);

  const groups = groupSyncConflicts(conflicts);

  const refresh = useCallback(() => {
    listSyncConflicts()
      .then(setConflicts)
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('list_sync_conflicts failed', err);
      })
      .finally(() => setLoaded(true));
  }, []);

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

  // Resolve the owning-row names whenever the conflict set changes.
  useEffect(() => {
    if (conflicts.length === 0) {
      setLabelByKey(new Map());
      return undefined;
    }
    let cancelled = false;
    void resolveConflictLabels(groupSyncConflicts(conflicts)).then((map) => {
      if (!cancelled) setLabelByKey(map);
    });
    return () => {
      cancelled = true;
    };
    // `groups` is derived from conflicts; keying on conflicts is sufficient.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [conflicts]);

  // Speak the real state once the first load settles (see announcedLoadRef).
  useEffect(() => {
    if (!loaded || announcedLoadRef.current) return;
    announcedLoadRef.current = true;
    announce(
      conflicts.length === 0
        ? t('dialogs.syncConflicts.empty')
        : conflicts.length === 1
          ? t('dialogs.syncConflicts.intro_one')
          : t('dialogs.syncConflicts.intro_other', { count: conflicts.length }),
    );
  }, [loaded, conflicts.length, announce, t]);

  // Re-park focus on the listbox so the user keeps walking the remaining
  // cards after a row/group unmounts (otherwise focus drops to <body>).
  // On the LAST conflict the <ul> itself unmounts (replaced by the "Keine
  // Konflikte" intro), so listRef is null and focusing it would be a no-op that
  // strands focus on <body> — NVDA leaves application mode with the dialog
  // still up. Fall back to the always-rendered intro note (it now reads the
  // empty state) so focus stays inside the role="application" body and the new
  // state is spoken.
  const reparkFocus = useCallback(() => {
    requestAnimationFrame(() => {
      (listRef.current ?? document.getElementById(introId))?.focus({
        preventScroll: true,
      });
    });
  }, [introId]);

  const afterResolveError = useCallback(
    (err: unknown) => {
      // eslint-disable-next-line no-console
      console.warn('resolve_sync_conflict failed', err);
      if (
        typeof err === 'object' &&
        err !== null &&
        'code' in err &&
        (err as { code?: string }).code === 'unsupported'
      ) {
        announce(
          t('dialogs.syncConflicts.actionSaveBothUnsupported'),
          'assertive',
        );
      }
      refresh();
    },
    [announce, refresh, t],
  );

  // Resolve a single field (the escape hatch for mixed resolutions).
  const resolveOne = useCallback(
    async (conflict: SyncConflict, choice: SyncResolutionChoice) => {
      setConflicts((prev) => prev.filter((c) => c.id !== conflict.id));
      reparkFocus();
      try {
        await resolveSyncConflict(conflict.id, choice);
        announce(t('dialogs.syncConflicts.resolved'));
      } catch (err) {
        afterResolveError(err);
      }
    },
    [afterResolveError, announce, reparkFocus, t],
  );

  // Resolve EVERY field of one item at once — the common path.
  const resolveGroup = useCallback(
    async (group: ConflictGroup<SyncConflict>, choice: SyncResolutionChoice) => {
      const ids = new Set(group.conflicts.map((c) => c.id));
      setBusyKeys((prev) => new Set(prev).add(group.key));
      setConflicts((prev) => prev.filter((c) => !ids.has(c.id)));
      reparkFocus();
      try {
        // Sequential so a mid-list failure leaves a consistent state we can
        // refresh back to, rather than half-applied parallel writes.
        for (const id of ids) {
          await resolveSyncConflict(id, choice);
        }
        announce(
          t('dialogs.syncConflicts.resolvedGroup', { count: ids.size }),
        );
      } catch (err) {
        afterResolveError(err);
      } finally {
        setBusyKeys((prev) => {
          const next = new Set(prev);
          next.delete(group.key);
          return next;
        });
      }
    },
    [afterResolveError, announce, reparkFocus, t],
  );

  const onCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(
        conflictsToText(groups, labelByKey, t, fmt),
      );
      announce(t('dialogs.syncConflicts.copied'));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('copy sync conflicts failed', err);
      announce(t('dialogs.syncConflicts.copyFailed'), 'assertive');
    }
  }, [groups, labelByKey, t, fmt, announce]);

  const intro =
    conflicts.length === 1
      ? t('dialogs.syncConflicts.intro_one')
      : t('dialogs.syncConflicts.intro_other', { count: conflicts.length });

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.syncConflicts.title')}
      className="sync-conflicts-dialog"
    >
      <FocusableNote id={introId} className="sync-conflicts__intro">
        {!loaded
          ? t('dialogs.syncConflicts.loading')
          : conflicts.length === 0
            ? t('dialogs.syncConflicts.empty')
            : intro}
      </FocusableNote>
      {conflicts.length > 0 && (
        <button
          type="button"
          className="sync-conflicts__copy"
          onClick={() => void onCopy()}
        >
          {t('dialogs.syncConflicts.copy')}
        </button>
      )}
      {groups.length > 0 && (
        <ul
          ref={listRef}
          tabIndex={-1}
          className="sync-conflicts__list"
          aria-label={t('dialogs.syncConflicts.listLabel')}
        >
          {groups.map((group) => (
            <ConflictGroupCard
              key={group.key}
              group={group}
              label={labelByKey.get(group.key)}
              fmt={fmt}
              t={t}
              busy={busyKeys.has(group.key)}
              onResolveGroup={resolveGroup}
              onResolveOne={resolveOne}
            />
          ))}
        </ul>
      )}
    </Modal>
  );
}

/** Kind label + short id when the name isn't resolved (deleted row, or an
 *  unnamed kind) — some identity without dumping a full UUID on a casual user. */
function fallbackLabel(
  group: ConflictGroup<SyncConflict>,
  t: ReturnType<typeof useTranslation>['t'],
): string {
  const kind = t(`dialogs.syncConflicts.rowKind.${group.rowKind}`);
  return t('dialogs.syncConflicts.unnamedRow', {
    kind,
    shortId: group.rowId.slice(0, 8),
  });
}

interface ConflictGroupCardProps {
  group: ConflictGroup<SyncConflict>;
  label: string | undefined;
  fmt: ReturnType<typeof useDateFormat>;
  t: ReturnType<typeof useTranslation>['t'];
  busy: boolean;
  onResolveGroup: (
    g: ConflictGroup<SyncConflict>,
    choice: SyncResolutionChoice,
  ) => void;
  onResolveOne: (c: SyncConflict, choice: SyncResolutionChoice) => void;
}

function ConflictGroupCard({
  group,
  label,
  fmt,
  t,
  busy,
  onResolveGroup,
  onResolveOne,
}: ConflictGroupCardProps) {
  const headingId = useId();
  const kindLabel = t(`dialogs.syncConflicts.rowKind.${group.rowKind}`);
  const name = label ?? fallbackLabel(group, t);
  const count = group.conflicts.length;
  // One source line for the whole item — same remote device, and (near enough)
  // the same edit time across its fields; show the first conflict's.
  const remoteTime = (() => {
    try {
      return fmt.format(new Date(group.conflicts[0].remote_timestamp), 'PPPp');
    } catch {
      return group.conflicts[0].remote_timestamp;
    }
  })();
  // Context folded into every button's accessible name so an SR user knows which
  // item they're acting on without back-arrowing to the heading.
  const groupContext = t('dialogs.syncConflicts.groupActionAriaContext', {
    kind: kindLabel,
    name,
    count,
  });

  return (
    <li
      className="sync-conflict-group"
      role="group"
      aria-labelledby={headingId}
      aria-busy={busy || undefined}
    >
      <div id={headingId} className="sync-conflict-group__heading">
        <span className="sync-conflict-group__kind">{kindLabel}</span>
        <strong className="sync-conflict-group__name">{name}</strong>
        <span className="sync-conflict-group__count">
          {count === 1
            ? t('dialogs.syncConflicts.groupConflictCount_one')
            : t('dialogs.syncConflicts.groupConflictCount_other', { count })}
        </span>
      </div>
      <p className="sync-conflict-group__source">
        {t('dialogs.syncConflicts.remoteSourceLabel', {
          time: remoteTime,
          device: group.conflicts[0].remote_device_id,
        })}
      </p>
      {/* Primary path: resolve ALL of this item's fields the same way. Only
          when there's more than one field — for a single-field item the group
          buttons would just duplicate the field's own keep/take below. */}
      {count > 1 && (
        <div className="sync-conflict-group__actions">
          <button
            type="button"
            disabled={busy}
            aria-label={`${t('dialogs.syncConflicts.actionKeepAllLocal')} — ${groupContext}`}
            onClick={() => onResolveGroup(group, 'keep_local')}
          >
            {t('dialogs.syncConflicts.actionKeepAllLocal')}
          </button>
          <button
            type="button"
            disabled={busy}
            aria-label={`${t('dialogs.syncConflicts.actionTakeAllRemote')} — ${groupContext}`}
            onClick={() => onResolveGroup(group, 'take_remote')}
          >
            {t('dialogs.syncConflicts.actionTakeAllRemote')}
          </button>
        </div>
      )}
      {/* Per-field detail + escape hatch (rare mixed resolution). */}
      <ul className="sync-conflict-group__fields">
        {group.conflicts.map((c) => (
          <ConflictFieldRow
            key={c.id}
            conflict={c}
            name={name}
            t={t}
            busy={busy}
            onResolve={onResolveOne}
          />
        ))}
      </ul>
    </li>
  );
}

interface ConflictFieldRowProps {
  conflict: SyncConflict;
  name: string;
  t: ReturnType<typeof useTranslation>['t'];
  busy: boolean;
  onResolve: (c: SyncConflict, choice: SyncResolutionChoice) => void;
}

function ConflictFieldRow({
  conflict,
  name,
  t,
  busy,
  onResolve,
}: ConflictFieldRowProps) {
  const fieldId = useId();
  const localId = useId();
  const remoteId = useId();
  const saveBothHintId = useId();
  const fieldLabel = t(`dialogs.syncConflicts.fieldName.${conflict.field}`, {
    defaultValue: conflict.field,
  });
  // Per-field context: item name + THIS field, so a per-field button doesn't
  // read identically to the group action or the sibling fields.
  const fieldContext = t('dialogs.syncConflicts.fieldActionAriaContext', {
    name,
    field: fieldLabel,
  });
  return (
    // The label ties together the field name AND both candidate values, so the
    // whole "Field: X — this device Y, other device Z" reads when a SR user tabs
    // into the group. Without the values in the accessible name they'd be
    // unreachable: the Modal body is role="application" (NVDA stays in focus
    // mode), where only focusable elements — not the static value text — are
    // navigable.
    <li
      className="sync-conflict-field"
      role="group"
      aria-labelledby={`${fieldId} ${localId} ${remoteId}`}
    >
      <div id={fieldId} className="sync-conflict-field__label">
        {t('dialogs.syncConflicts.fieldLabel')}: {fieldLabel}
      </div>
      <div className="sync-conflict-field__values">
        <div id={localId}>
          <span className="sync-conflict-field__valueLabel">
            {t('dialogs.syncConflicts.localValueLabel')}
          </span>
          <code>{decodeForDisplay(conflict.local_value)}</code>
        </div>
        <div id={remoteId}>
          <span className="sync-conflict-field__valueLabel">
            {t('dialogs.syncConflicts.remoteValueLabel')}
          </span>
          <code>{decodeForDisplay(conflict.remote_value)}</code>
        </div>
      </div>
      <div className="sync-conflict-field__actions">
        <button
          type="button"
          disabled={busy}
          aria-label={`${t('dialogs.syncConflicts.actionKeepLocal')} — ${fieldContext}`}
          onClick={() => onResolve(conflict, 'keep_local')}
        >
          {t('dialogs.syncConflicts.actionKeepLocal')}
        </button>
        <button
          type="button"
          disabled={busy}
          aria-label={`${t('dialogs.syncConflicts.actionTakeRemote')} — ${fieldContext}`}
          onClick={() => onResolve(conflict, 'take_remote')}
        >
          {t('dialogs.syncConflicts.actionTakeRemote')}
        </button>
        <button
          type="button"
          disabled={busy}
          aria-label={`${t('dialogs.syncConflicts.actionSaveBoth')} — ${fieldContext}`}
          aria-describedby={saveBothHintId}
          onClick={() => onResolve(conflict, 'save_both')}
        >
          {t('dialogs.syncConflicts.actionSaveBoth')}
        </button>
        <span id={saveBothHintId} className="sr-only">
          {t('dialogs.syncConflicts.actionSaveBothUnsupported')}
        </span>
      </div>
    </li>
  );
}

/** Serialize every conflict to a readable, paste-able block — for reporting a
 *  sync problem from the dialog. Grouped by item with the resolved name, and the
 *  same value decode as the rows so a serialization difference stays visible. */
function conflictsToText(
  groups: ConflictGroup<SyncConflict>[],
  labelByKey: Map<string, string>,
  t: ReturnType<typeof useTranslation>['t'],
  fmt: ReturnType<typeof useDateFormat>,
): string {
  const total = groups.reduce((n, g) => n + g.conflicts.length, 0);
  const lines: string[] = [
    t('dialogs.syncConflicts.copyHeader', { count: total }),
    '',
  ];
  groups.forEach((g, gi) => {
    const kind = t(`dialogs.syncConflicts.rowKind.${g.rowKind}`);
    const name = labelByKey.get(g.key) ?? fallbackLabel(g, t);
    let remoteTime: string;
    try {
      remoteTime = fmt.format(new Date(g.conflicts[0].remote_timestamp), 'PPPp');
    } catch {
      remoteTime = g.conflicts[0].remote_timestamp;
    }
    lines.push(
      `[${gi + 1}] ${kind}: ${name} (${g.rowId})`,
      `    ${t('dialogs.syncConflicts.remoteSourceLabel', {
        time: remoteTime,
        device: g.conflicts[0].remote_device_id,
      })}`,
    );
    g.conflicts.forEach((c) => {
      const fieldLabel = t(`dialogs.syncConflicts.fieldName.${c.field}`, {
        defaultValue: c.field,
      });
      lines.push(
        `    ${t('dialogs.syncConflicts.fieldLabel')}: ${fieldLabel}`,
        `        ${t('dialogs.syncConflicts.localValueLabel')}: ${decodeForDisplay(c.local_value)}`,
        `        ${t('dialogs.syncConflicts.remoteValueLabel')}: ${decodeForDisplay(c.remote_value)}`,
      );
    });
    lines.push('');
  });
  return lines.join('\n').trimEnd() + '\n';
}

/** Decode the JSON-encoded backend value into something readable. */
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
