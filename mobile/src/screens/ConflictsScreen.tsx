import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import { groupSyncConflicts, type ConflictGroup } from '@aperio/shared';

import { getTaskById, listTaskLists } from '../api/client';
import {
  listSyncConflicts,
  resolveSyncConflict,
  SyncConflict,
  SyncResolutionChoice,
} from '../api/sync';
import { formatLongDateTime } from '../intl/dateFormat';
import type { RootStackScreenProps } from '../navigation/types';
import { useThemedStyles, type ThemeColors } from '../theme';

// Sync-conflict resolution — an RN port of the reworked desktop
// SyncConflictsDialog (DESIGN §19.3). The applier records ONE conflict per
// differing FIELD, so a task edited on two devices surfaces as several rows
// (status + scheduled_date + completed_at …). We GROUP them by owning row (via
// the shared groupSyncConflicts) so each item is ONE card with a "resolve all of
// this item" action — you almost never want to keep 2 of 3 fields from different
// devices — and resolve the raw UUID to the item's NAME so it reads for casual
// users. The per-field values + per-field buttons stay under each card as the
// escape hatch for the rare mixed resolution.
//
// Name resolution is limited to what the mobile client can fetch: tasks (via
// getTaskById) and task lists (via listTaskLists). Kinds without a mobile fetch
// API (event / calendar / color_label) fall back to the unnamedRow label rather
// than adding backend commands.
//
// Screen-reader-first (the primary user is BLIND): each card is NOT a single
// accessible node (so its buttons stay separately focusable), and every button's
// label carries the item NAME + (per-field) the field, so a button never reads
// identically to its siblings. Reading order per card: heading → group actions →
// per-field details. No Tauri event bus on mobile → re-fetch after each resolve.

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/** Conflict values are JSON-encoded scalars; decode for display, null → em-dash. */
function decodeForDisplay(raw: string | null): string {
  if (raw == null) return '—';
  try {
    const v = JSON.parse(raw);
    return v == null ? '—' : String(v);
  } catch {
    return raw;
  }
}

/** Resolve each group's owning row to its human name, limited to the kinds the
 *  mobile client can fetch: `task` (getTaskById) + `task_list` (listTaskLists).
 *  Other kinds (event / calendar / color_label) have no mobile fetch API, so
 *  they're left out and fall back to the short label in the UI. Returns a fresh
 *  `key → name` map. */
async function resolveConflictLabels(
  groups: ConflictGroup<SyncConflict>[],
): Promise<Map<string, string>> {
  const map = new Map<string, string>();
  const lists = groups.some((g) => g.rowKind === 'task_list')
    ? await listTaskLists().catch(() => [])
    : [];
  await Promise.all(
    groups.map(async (g) => {
      let name: string | undefined;
      try {
        if (g.rowKind === 'task') {
          name = (await getTaskById(g.rowId))?.title;
        } else if (g.rowKind === 'task_list') {
          name = lists.find((l) => l.id === g.rowId)?.name;
        }
        // event / calendar / color_label: no mobile fetch API → fallback label.
      } catch {
        // Deleted / unfetchable — leave it out; the UI shows the fallback.
      }
      if (name && name.trim()) map.set(g.key, name.trim());
    }),
  );
  return map;
}

export default function ConflictsScreen({ navigation }: RootStackScreenProps<'Conflicts'>) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [conflicts, setConflicts] = useState<SyncConflict[]>([]);
  const [labelByKey, setLabelByKey] = useState<Map<string, string>>(new Map());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Screen-reader focus after a resolve: RN doesn't move VoiceOver/TalkBack
  // focus when the list shrinks, so without this the cursor strands at the top
  // and the user must swipe all the way back down. We register each group
  // heading's native tag + the intro, then re-park focus on the next surviving
  // item (or the empty-state intro) — the RN twin of the desktop reparkFocus().
  const groupTags = useRef<Record<string, number | null>>({});
  const introTag = useRef<number | null>(null);
  const pendingFocus = useRef<string | 'intro' | null>(null);
  const registerGroupTag = useCallback((key: string, tag: number | null) => {
    groupTags.current[key] = tag;
  }, []);

  const groups = groupSyncConflicts(conflicts);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const load = useCallback(async () => {
    try {
      setConflicts(await listSyncConflicts());
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, []);

  // Load on mount + whenever the screen regains focus (a sync round may have
  // recorded more while we were away).
  useEffect(() => {
    const unsubscribe = navigation.addListener('focus', () => void load());
    void load();
    return unsubscribe;
  }, [navigation, load]);

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

  // Apply a pending SR-focus move AFTER the list re-renders without the resolved
  // items (the target's native tag is registered in the same commit, so it's
  // available here).
  useEffect(() => {
    const pf = pendingFocus.current;
    if (pf == null) return;
    pendingFocus.current = null;
    const tag = pf === 'intro' ? introTag.current : groupTags.current[pf];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [conflicts]);

  // Resolve a single field (the escape hatch for mixed resolutions). Optimistic:
  // drop the field on tap; re-fetch on error to restore the canonical state.
  const resolveOne = useCallback(
    async (conflict: SyncConflict, choice: SyncResolutionChoice) => {
      if (busy) return;
      setBusy(true);
      setError(null);
      // Focus target: the same item if it still has other fields, else the next
      // remaining item, else the (empty) intro.
      const remaining = groupSyncConflicts(
        conflicts.filter((c) => c.id !== conflict.id),
      );
      const sameKey = `${conflict.row_kind}:${conflict.row_id}`;
      pendingFocus.current =
        remaining.find((g) => g.key === sameKey)?.key ??
        remaining[0]?.key ??
        'intro';
      setConflicts((prev) => prev.filter((c) => c.id !== conflict.id));
      try {
        await resolveSyncConflict(conflict.id, choice);
        // The empty-state intro (focused above) already announces "no conflicts";
        // only confirm when work remains, so the two don't clobber each other.
        if (remaining.length > 0) announce(t('dialogs.syncConflicts.resolved'));
      } catch (err) {
        // save_both is intentionally unsupported — surface it assertively.
        const message = errorMessage(err);
        setError(message);
        announce(
          choice === 'save_both'
            ? t('dialogs.syncConflicts.actionSaveBothUnsupported')
            : t('mobile.error', { message }),
        );
        await load();
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, conflicts, load, t],
  );

  // Resolve EVERY field of one item at once — the common path. Optimistic: drop
  // the whole group on tap; re-fetch on error.
  const resolveGroup = useCallback(
    async (group: ConflictGroup<SyncConflict>, choice: SyncResolutionChoice) => {
      if (busy) return;
      setBusy(true);
      setError(null);
      const ids = new Set(group.conflicts.map((c) => c.id));
      // Focus the next surviving item, or the (empty) intro when this was last.
      const remaining = groups.filter((g) => g.key !== group.key);
      pendingFocus.current = remaining[0]?.key ?? 'intro';
      setConflicts((prev) => prev.filter((c) => !ids.has(c.id)));
      try {
        // Sequential so a mid-list failure leaves a consistent state we can
        // refresh back to, rather than half-applied parallel writes.
        for (const id of ids) {
          await resolveSyncConflict(id, choice);
        }
        // Skip the confirmation when the list is now empty — the focused
        // empty-state intro announces it, and the two would clobber each other.
        if (remaining.length > 0) {
          announce(t('dialogs.syncConflicts.resolvedGroup', { count: ids.size }));
        }
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
        await load();
      } finally {
        setBusy(false);
      }
    },
    [announce, busy, groups, load, t],
  );

  const fmtTime = useCallback(
    (iso: string) => {
      try {
        return formatLongDateTime(new Date(iso), i18n.language);
      } catch {
        return iso;
      }
    },
    [i18n.language],
  );

  const intro =
    conflicts.length === 1
      ? t('dialogs.syncConflicts.intro_one')
      : t('dialogs.syncConflicts.intro_other', { count: conflicts.length });

  return (
    <View style={styles.screen}>
      <Text
        ref={(node) => {
          introTag.current = node ? findNodeHandle(node) : null;
        }}
        style={styles.intro}
        accessibilityRole="text"
        accessibilityLiveRegion="polite"
      >
        {loading
          ? t('mobile.loading')
          : conflicts.length === 0
            ? t('dialogs.syncConflicts.empty')
            : intro}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      <ScrollView
        accessibilityRole="list"
        accessibilityLabel={t('dialogs.syncConflicts.listLabel')}
        contentContainerStyle={styles.list}
        keyboardShouldPersistTaps="handled"
      >
        {groups.map((group) => (
          <ConflictGroupCard
            key={group.key}
            group={group}
            label={labelByKey.get(group.key)}
            styles={styles}
            t={t}
            busy={busy}
            fmtTime={fmtTime}
            onHeadingRef={registerGroupTag}
            onResolveGroup={resolveGroup}
            onResolveOne={resolveOne}
          />
        ))}
      </ScrollView>
    </View>
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
  styles: ReturnType<typeof makeStyles>;
  t: ReturnType<typeof useTranslation>['t'];
  busy: boolean;
  fmtTime: (iso: string) => string;
  onHeadingRef: (key: string, tag: number | null) => void;
  onResolveGroup: (
    g: ConflictGroup<SyncConflict>,
    choice: SyncResolutionChoice,
  ) => void;
  onResolveOne: (c: SyncConflict, choice: SyncResolutionChoice) => void;
}

function ConflictGroupCard({
  group,
  label,
  styles,
  t,
  busy,
  fmtTime,
  onHeadingRef,
  onResolveGroup,
  onResolveOne,
}: ConflictGroupCardProps) {
  const kindLabel = t(`dialogs.syncConflicts.rowKind.${group.rowKind}`);
  const name = label ?? fallbackLabel(group, t);
  const count = group.conflicts.length;
  const countLabel =
    count === 1
      ? t('dialogs.syncConflicts.groupConflictCount_one')
      : t('dialogs.syncConflicts.groupConflictCount_other', { count });
  // Context folded into every button's accessible name so an SR user knows which
  // item they're acting on without back-arrowing to the heading.
  const groupContext = t('dialogs.syncConflicts.groupActionAriaContext', {
    kind: kindLabel,
    name,
    count,
  });

  return (
    <View style={styles.card} accessibilityRole="none">
      <Text
        ref={(node) => onHeadingRef(group.key, node ? findNodeHandle(node) : null)}
        style={styles.cardName}
        accessibilityRole="header"
        accessibilityLabel={`${kindLabel} ${name}, ${countLabel}`}
      >
        {kindLabel}: {name} ({countLabel})
      </Text>
      <Text style={styles.cardSource} accessibilityRole="text">
        {t('dialogs.syncConflicts.remoteSourceLabel', {
          time: fmtTime(group.conflicts[0].remote_timestamp),
          device: group.conflicts[0].remote_device_id,
        })}
      </Text>

      {/* Primary path: resolve ALL of this item's fields the same way. Only for
          multi-field items — a single-field group's buttons would just duplicate
          the field's own keep/take below. */}
      {count > 1 && (
        <View style={styles.actions}>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy }}
            accessibilityLabel={`${t('dialogs.syncConflicts.actionKeepAllLocal')} — ${groupContext}`}
            disabled={busy}
            onPress={() => onResolveGroup(group, 'keep_local')}
            style={({ pressed }) => [styles.groupBtn, pressed && styles.pressed]}
          >
            <Text style={styles.groupBtnText}>
              {t('dialogs.syncConflicts.actionKeepAllLocal')}
            </Text>
          </Pressable>
          <Pressable
            accessibilityRole="button"
            accessibilityState={{ disabled: busy }}
            accessibilityLabel={`${t('dialogs.syncConflicts.actionTakeAllRemote')} — ${groupContext}`}
            disabled={busy}
            onPress={() => onResolveGroup(group, 'take_remote')}
            style={({ pressed }) => [styles.groupBtn, pressed && styles.pressed]}
          >
            <Text style={styles.groupBtnText}>
              {t('dialogs.syncConflicts.actionTakeAllRemote')}
            </Text>
          </Pressable>
        </View>
      )}

      {/* Per-field detail + escape hatch (rare mixed resolution). */}
      <View style={styles.fields}>
        {group.conflicts.map((c) => (
          <ConflictFieldRow
            key={c.id}
            conflict={c}
            name={name}
            styles={styles}
            t={t}
            busy={busy}
            onResolve={onResolveOne}
          />
        ))}
      </View>
    </View>
  );
}

interface ConflictFieldRowProps {
  conflict: SyncConflict;
  name: string;
  styles: ReturnType<typeof makeStyles>;
  t: ReturnType<typeof useTranslation>['t'];
  busy: boolean;
  onResolve: (c: SyncConflict, choice: SyncResolutionChoice) => void;
}

function ConflictFieldRow({
  conflict,
  name,
  styles,
  t,
  busy,
  onResolve,
}: ConflictFieldRowProps) {
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
    <View style={styles.field}>
      <Text style={styles.fieldLabel} accessibilityRole="text">
        {t('dialogs.syncConflicts.fieldLabel')}: {fieldLabel}
      </Text>
      <Text style={styles.cardValue} accessibilityRole="text">
        {t('dialogs.syncConflicts.localValueLabel')}: {decodeForDisplay(conflict.local_value)}
      </Text>
      <Text style={styles.cardValue} accessibilityRole="text">
        {t('dialogs.syncConflicts.remoteValueLabel')}: {decodeForDisplay(conflict.remote_value)}
      </Text>
      <View style={styles.actions}>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={`${t('dialogs.syncConflicts.actionKeepLocal')} — ${fieldContext}`}
          disabled={busy}
          onPress={() => onResolve(conflict, 'keep_local')}
          style={({ pressed }) => [styles.actionBtn, pressed && styles.pressed]}
        >
          <Text style={styles.actionText}>
            {t('dialogs.syncConflicts.actionKeepLocal')}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityLabel={`${t('dialogs.syncConflicts.actionTakeRemote')} — ${fieldContext}`}
          disabled={busy}
          onPress={() => onResolve(conflict, 'take_remote')}
          style={({ pressed }) => [styles.actionBtn, pressed && styles.pressed]}
        >
          <Text style={styles.actionText}>
            {t('dialogs.syncConflicts.actionTakeRemote')}
          </Text>
        </Pressable>
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: busy }}
          accessibilityHint={t('dialogs.syncConflicts.actionSaveBothUnsupported')}
          accessibilityLabel={`${t('dialogs.syncConflicts.actionSaveBoth')} — ${fieldContext}`}
          disabled={busy}
          onPress={() => onResolve(conflict, 'save_both')}
          style={({ pressed }) => [styles.actionBtn, pressed && styles.pressed]}
        >
          <Text style={styles.actionText}>
            {t('dialogs.syncConflicts.actionSaveBoth')}
          </Text>
        </Pressable>
      </View>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    intro: { fontSize: 15, color: c.textLabel, padding: 16 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger, paddingHorizontal: 16 },
    list: { gap: 12, padding: 16 },
    card: {
      gap: 4,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    cardName: { fontSize: 17, fontWeight: '700', color: c.textPrimary },
    cardSource: { fontSize: 13, color: c.textSecondary },
    cardValue: { fontSize: 15, color: c.textPrimary },
    actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, marginTop: 8 },
    groupBtn: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.accent,
      backgroundColor: c.surface,
    },
    groupBtnText: { fontSize: 15, fontWeight: '700', color: c.accent },
    fields: {
      gap: 12,
      marginTop: 12,
      paddingTop: 12,
      borderTopWidth: 1,
      borderTopColor: c.border,
    },
    field: { gap: 4 },
    fieldLabel: { fontSize: 15, fontWeight: '600', color: c.textPrimary },
    actionBtn: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    actionText: { fontSize: 15, fontWeight: '600', color: c.accent },
    pressed: { opacity: 0.7 },
  });
