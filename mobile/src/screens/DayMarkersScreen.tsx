import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { moveDayMarker, sortDayMarkers, type DayMarker } from '@aperio/shared';

import {
  createDayMarker,
  deleteDayMarker,
  listDayMarkers,
  updateDayMarker,
} from '../api/dayMarkers';
import { FormScrollView } from '../components/FormScrollView';
import { useListFocusManager } from '../a11y/useListFocusManager';
import {
  duringDayMarkerBurst,
  useDayMarkersChanged,
} from '../state/dayMarkersChanged';
import { useThemedStyles, type ThemeColors } from '../theme';

// The day-marker vocabulary, managed on the phone. Twin of the desktop
// DayMarkersPanel — same rules, same wording, the platform's own widgets.
//
// Every marker is one row carrying its own controls rather than a listbox with
// a separate edit mode: on a touch screen a mode switch costs a whole screen,
// and the vocabulary is short enough that per-row editing stays cheap. The
// name field IS the editor — it writes on blur, so there is no Save to hunt
// for and no state to lose by backing out.

export default function DayMarkersScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const [markers, setMarkers] = useState<DayMarker[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [newName, setNewName] = useState('');
  const [newSymbol, setNewSymbol] = useState('');
  const focus = useListFocusManager(markers.length);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const refresh = useCallback(async () => {
    try {
      setMarkers(sortDayMarkers(await listDayMarkers()));
      setError(null);
    } catch (err) {
      // Keep what is on screen: an empty list would read as "you have none",
      // which is a different statement from "the read failed".
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A marker added or renamed on another device, arriving on a sync round while
  // this screen stands open.
  useDayMarkersChanged(() => {
    void refresh();
  });

  const onAdd = useCallback(async () => {
    const name = newName.trim();
    if (!name) return;
    try {
      focus.onAdd();
      await createDayMarker(name, newSymbol.trim() || null, null);
      setNewName('');
      setNewSymbol('');
      await refresh();
      announce(t('dialogs.settings.dayMarkers.added', { name }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, [newName, newSymbol, refresh, announce, t, focus]);

  const onRename = useCallback(
    async (marker: DayMarker, name: string, symbol: string) => {
      const trimmed = name.trim();
      if (!trimmed || (trimmed === marker.name && symbol.trim() === (marker.symbol ?? ''))) {
        return;
      }
      try {
        await updateDayMarker({
          ...marker,
          name: trimmed,
          symbol: symbol.trim() || null,
        });
        await refresh();
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [refresh],
  );

  const onDelete = useCallback(
    async (marker: DayMarker, index: number) => {
      try {
        focus.onRemove(index);
        await deleteDayMarker(marker.id);
        await refresh();
        announce(t('dialogs.settings.dayMarkers.deleted', { name: marker.name }));
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    },
    [refresh, announce, t, focus],
  );

  const onMove = useCallback(
    async (marker: DayMarker, delta: number) => {
      const reordered = moveDayMarker(markers, marker.id, delta);
      if (reordered === markers) return;
      setMarkers(reordered);
      try {
        // Only the rows that actually shifted — a move near the top of a long
        // list must not rewrite the whole vocabulary.
        const before = new Map(markers.map((m) => [m.id, m.position ?? 0]));
        // One burst — see the desktop twin.
        await duringDayMarkerBurst(async () => {
          for (const m of reordered) {
            if (before.get(m.id) !== m.position) await updateDayMarker(m);
          }
        });
        announce(
          t('dialogs.settings.dayMarkers.moved', {
            name: marker.name,
            position: reordered.findIndex((m) => m.id === marker.id) + 1,
            count: reordered.length,
          }),
        );
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
        await refresh();
      }
    },
    [markers, refresh, announce, t],
  );

  return (
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
      <Text style={styles.intro} accessibilityRole="text">
        {t('dialogs.settings.dayMarkers.intro')}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {loading ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.dayMarkers.loading')}
        </Text>
      ) : markers.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.dayMarkers.empty')}
        </Text>
      ) : (
        markers.map((m, i) => (
          <MarkerRow
            key={m.id}
            marker={m}
            index={i}
            count={markers.length}
            registerRow={focus.registerRow}
            styles={styles}
            onCommit={onRename}
            onMove={onMove}
            onDelete={() => void onDelete(m, i)}
          />
        ))
      )}

      <View style={styles.addBlock}>
        <Text style={styles.label} accessibilityRole="header">
          {t('dialogs.settings.dayMarkers.addHeading')}
        </Text>
        <TextInput
          style={styles.input}
          value={newName}
          onChangeText={setNewName}
          accessibilityLabel={t('dialogs.settings.dayMarkers.nameLabel')}
          placeholder={t('dialogs.settings.dayMarkers.nameLabel')}
        />
        <TextInput
          style={styles.input}
          value={newSymbol}
          onChangeText={setNewSymbol}
          accessibilityLabel={t('dialogs.settings.dayMarkers.symbolLabel')}
          placeholder={t('dialogs.settings.dayMarkers.symbolLabel')}
          autoCapitalize="none"
          autoCorrect={false}
        />
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.dayMarkers.symbolHint')}
        </Text>
        <Pressable
          ref={focus.registerAdd}
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.dayMarkers.add')}
          onPress={() => void onAdd()}
          style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
        >
          <Text style={styles.addButtonText}>
            {t('dialogs.settings.dayMarkers.add')}
          </Text>
        </Pressable>
      </View>
    </FormScrollView>
  );
}

/** One marker: its name and symbol, editable in place, plus order and delete. */
function MarkerRow({
  marker,
  index,
  count,
  registerRow,
  styles,
  onCommit,
  onMove,
  onDelete,
}: {
  marker: DayMarker;
  index: number;
  count: number;
  registerRow: (i: number) => (el: never) => void;
  styles: ReturnType<typeof makeStyles>;
  onCommit: (marker: DayMarker, name: string, symbol: string) => void;
  onMove: (marker: DayMarker, delta: number) => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(marker.name);
  const [symbol, setSymbol] = useState(marker.symbol ?? '');
  const rowName = t('dialogs.settings.dayMarkers.rowLabel', {
    name: marker.name,
    position: index + 1,
    count,
  });

  return (
    <View style={styles.row}>
      <TextInput
        ref={registerRow(index)}
        style={styles.input}
        value={name}
        onChangeText={setName}
        // Writes on blur: no Save to hunt for, and backing out of the screen
        // cannot lose an edit the user considers made.
        onBlur={() => onCommit(marker, name, symbol)}
        accessibilityLabel={rowName}
      />
      <TextInput
        style={styles.input}
        value={symbol}
        onChangeText={setSymbol}
        onBlur={() => onCommit(marker, name, symbol)}
        accessibilityLabel={`${t('dialogs.settings.dayMarkers.symbolLabel')}, ${rowName}`}
        autoCapitalize="none"
        autoCorrect={false}
      />
      <View style={styles.rowActions}>
        {index > 0 && (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${t('dialogs.settings.dayMarkers.moveUp')}, ${rowName}`}
            onPress={() => onMove(marker, -1)}
            style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
          >
            <Text style={styles.smallButtonText}>
              {t('dialogs.settings.dayMarkers.moveUp')}
            </Text>
          </Pressable>
        )}
        {index < count - 1 && (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={`${t('dialogs.settings.dayMarkers.moveDown')}, ${rowName}`}
            onPress={() => onMove(marker, 1)}
            style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
          >
            <Text style={styles.smallButtonText}>
              {t('dialogs.settings.dayMarkers.moveDown')}
            </Text>
          </Pressable>
        )}
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={`${t('dialogs.settings.dayMarkers.delete')}, ${rowName}`}
          onPress={onDelete}
          style={({ pressed }) => [styles.removeButton, pressed && styles.pressed]}
        >
          <Text style={styles.removeButtonText}>
            {t('dialogs.settings.dayMarkers.delete')}
          </Text>
        </Pressable>
      </View>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 16 },
    intro: { fontSize: 15, color: c.textSecondary },
    hint: { fontSize: 13, color: c.textSecondary },
    error: { fontSize: 15, color: c.danger },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    row: {
      gap: 8,
      padding: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    rowActions: { flexDirection: 'row', gap: 8, flexWrap: 'wrap' },
    addBlock: { gap: 8 },
    input: {
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    pressed: { opacity: 0.7 },
    smallButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    smallButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    addButton: {
      paddingVertical: 12,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    addButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    removeButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    removeButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
