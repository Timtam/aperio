import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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

import {
  heldSentence,
  heldStatusSentence,
  importedCreate,
  importInto,
  importOwner,
  importRowLabel,
  importWhen,
  invitationListSentence,
  joinNames,
  rechosenForHeld,
  selectableEventCalendars,
  skippedSentence,
  writeErrorMessage,
  type ImportedEvent,
  type ImportInto,
} from '@aperio/shared';

import { selectableCheckState, selectableRole } from '../a11y/roles';
import { createEvent, listCalendars, type Calendar } from '../api/calendar';
import { SelectFieldButton } from '../components/SelectFieldButton';
import { useCancelHeader } from '../components/useCancelHeader';
import { formatLongDate } from '../intl/dateFormat';
import type { RootStackScreenProps } from '../navigation/types';
import { useShowHiddenCalendarTargets } from '../settings/hiddenTargets';
import { useCalendarVisibility } from '../state/calendarVisibility';
import { importTarget } from '../state/fileImportFlow';
import { newImportToken, onImportedFromEditor } from '../state/importProgress';
import { readLastUsedCalendar, writeLastUsedCalendar } from '../state/lastUsedCalendar';
import { useCalendarOwnAddress } from '../state/useCalendarOwnAddress';
import { useHeldOnImport } from '../state/useHeldOnImport';
import { useThemedStyles, type ThemeColors } from '../theme';

const announce = (message: string) => AccessibilityInfo.announceForAccessibility(message);

/**
 * The appointments of a calendar file that holds more than one
 * (DESIGN-file-import.md, I5): each to open in the editor, or all chosen ones
 * written to one calendar, one after another. Mirrors the desktop's
 * `ImportDialog`; one appointment never reaches it, the editor opens on it.
 *
 * A row is one element for the reader: a checkbox with "Edit …" as a rotor
 * action, the sighted-only twin of the visible Edit button, as the calendar
 * list's rows carry Manage.
 */
export default function FileImportScreen({
  route,
  navigation,
}: RootStackScreenProps<'FileImport'>) {
  const { fileName, read } = route.params;
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const { hidden } = useCalendarVisibility();
  const includeHidden = useShowHiddenCalendarTargets();

  const [busy, setBusyState] = useState(false);
  // Read where a render may not have caught up yet: the list closes itself
  // right after its writes end, in the same turn.
  const busyRef = useRef(false);
  const setBusy = useCallback((value: boolean) => {
    busyRef.current = value;
    setBusyState(value);
  }, []);
  // Not while it writes: the outcome has to be said where it is seen.
  useCancelHeader(navigation, 'mobile.cancel', {
    writing: busy,
    close: () => {
      if (!busyRef.current) navigation.goBack();
    },
  });
  useEffect(
    () =>
      navigation.addListener('beforeRemove', (event) => {
        if (busyRef.current) event.preventDefault();
      }),
    [navigation],
  );

  const [calendars, setCalendars] = useState<Calendar[]>([]);
  const [calendarId, setCalendarId] = useState('');
  const calendarTouched = useRef(false);
  // Read by the load below as they are when it lands, without loading again
  // whenever they change: the first pick is made once, on open.
  const hiddenRef = useRef(hidden);
  hiddenRef.current = hidden;
  const includeHiddenRef = useRef(includeHidden);
  includeHiddenRef.current = includeHidden;
  useEffect(() => {
    void (async () => {
      const [cals, lastUsed] = await Promise.all([
        listCalendars().catch(() => [] as Calendar[]),
        readLastUsedCalendar().catch(() => null),
      ]);
      setCalendars(cals);
      // The pick is the person's once they made one.
      if (!calendarTouched.current) {
        setCalendarId(
          importTarget(cals, lastUsed, hiddenRef.current, includeHiddenRef.current) ?? '',
        );
      }
    })();
  }, []);
  const targets = useMemo(
    () =>
      selectableEventCalendars(calendars, {
        selectedIds: new Set(calendars.filter((c) => !hidden.has(c.id)).map((c) => c.id)),
        currentId: calendarId || undefined,
        includeHidden,
      }),
    [calendars, hidden, includeHidden, calendarId],
  );
  const calendar = targets.find((c) => c.id === calendarId);
  const calendarName = calendar?.name ?? '';

  const events = read.events;
  const [chosen, setChosen] = useState<ReadonlySet<number>>(
    () => new Set(events.map((_, i) => i)),
  );
  const [imported, setImported] = useState<ReadonlySet<number>>(() => new Set());
  const markImported = useCallback((index: number) => {
    setImported((prev) => new Set(prev).add(index));
  }, []);
  // The editor tells this list which of its rows it created.
  const [token] = useState(newImportToken);
  useEffect(() => onImportedFromEditor(token, markImported), [token, markImported]);

  const [notice, setNotice] = useState<string | null>(null);
  const noticeRef = useRef<Text>(null);
  useEffect(() => {
    if (notice == null) return;
    requestAnimationFrame(() => {
      const tag = findNodeHandle(noticeRef.current);
      if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
    });
  }, [notice]);

  // What the chosen calendar holds already (I4): unchecked when the answer
  // comes, and imported as a copy, under a UID of its own, when checked again.
  const held = useHeldOnImport(calendarId, events);
  const isHeld = (index: number) => held.status === 'known' && held.held.has(index);
  // Each answer is taken once: its rows unchecked, and the rows unchecked for
  // the calendar asked before checked again where this one does not hold them.
  const lastAnswer = useRef<typeof held | null>(null);
  const uncheckedRows = useRef<ReadonlySet<number>>(new Set());
  useEffect(() => {
    if (held.status === 'checking' || lastAnswer.current === held) return;
    lastAnswer.current = held;
    if (held.status === 'failed') {
      announce(t('fileImport.held.checkFailed', { calendar: calendarName }));
      return;
    }
    const before = uncheckedRows.current;
    uncheckedRows.current = held.held;
    setChosen((prev) => rechosenForHeld(prev, before, held.held));
    if (held.held.size > 0) announce(heldSentence(events, held.held, calendarName, t));
  }, [held, calendarName, events, t]);
  const heldStatus = heldStatusSentence(events, held, calendarName, t);

  // Whose each one is, as the chosen calendar tells: the account's own meeting
  // is a new appointment, someone else's an invitation the calendar keeps or
  // cannot keep quietly (I6).
  const own = useCalendarOwnAddress(
    calendarId,
    events.some((event) => event.organizer?.trim()),
  );
  // Not known before the calendars are: until they load, a calendar that keeps
  // invitations would read as one that does not.
  const intoOf = (event: ImportedEvent): ImportInto =>
    calendars.length === 0 ? { kind: 'checking' } : importInto(importOwner(event, own), calendar);
  const summary = [
    t('fileImport.list.summary', { file: fileName, count: events.length }),
    skippedSentence(read.skipped, t),
    invitationListSentence(events.map(intoOf), calendarName, t),
  ]
    .filter(Boolean)
    .join(' ');

  const time = useMemo(
    () => new Intl.DateTimeFormat(i18n.language, { timeStyle: 'short' }),
    [i18n.language],
  );
  const when = (event: ImportedEvent): string =>
    importWhen(
      event,
      { date: (at) => formatLongDate(at, i18n.language), time: (at) => time.format(at) },
      t,
    );

  const toImport = events
    .map((event, index) => ({ event, index }))
    .filter(({ index }) => chosen.has(index) && !imported.has(index));

  // As the file has it: the editor decides, for the calendar it ends up in,
  // whether an invitation keeps its people.
  const edit = (event: ImportedEvent, index: number) => {
    if (busyRef.current || !calendarId) return;
    navigation.navigate('EventEditor', {
      eventId: null,
      calendarId,
      importFrom: event,
      importNote: null,
      importToken: token,
      importIndex: index,
    });
  };

  const importChosen = async () => {
    if (busyRef.current) return;
    if (!calendarId) {
      setNotice(t('fileImport.noCalendar'));
      return;
    }
    if (toImport.length === 0) {
      setNotice(t('fileImport.list.noneChosen'));
      return;
    }
    // Which are there already decides which keep their UID.
    if (held.status === 'checking') {
      setNotice(t('fileImport.held.stillChecking', { calendar: calendarName }));
      return;
    }
    // Whose they are decides which keep their people.
    if (toImport.some(({ event }) => intoOf(event).kind === 'checking')) {
      setNotice(t('fileImport.invitation.listStillChecking'));
      return;
    }
    setBusy(true);
    setNotice(null);
    const failed: { title: string; err: unknown }[] = [];
    let done = 0;
    // One after the other: a provider asked many times at once may turn the
    // rest away as busy.
    for (const { event: entry, index } of toImport) {
      try {
        await createEvent(importedCreate(entry, intoOf(entry), calendarId, isHeld(index)), {
          preserveRecurrenceZone: true,
        });
        done += 1;
        markImported(index);
      } catch (err) {
        failed.push({ title: entry.title, err });
      }
    }
    setBusy(false);
    if (done > 0) void writeLastUsedCalendar(calendarId);
    if (failed.length === 0) {
      announce(t('fileImport.list.done', { count: done, calendar: calendarName }));
      navigation.goBack();
      return;
    }
    setNotice(
      t('fileImport.list.partly', {
        done,
        count: toImport.length,
        calendar: calendarName,
        titles: joinNames(
          failed.map((f) => t('fileImport.list.quoted', { title: f.title })),
          t,
        ),
        reason: writeErrorMessage(failed[0].err, t),
      }),
    );
  };

  return (
    <ScrollView style={styles.screen} contentContainerStyle={styles.body}>
      <Text style={styles.summary} accessibilityRole="text">
        {summary}
      </Text>
      {notice != null && (
        <Text
          ref={noticeRef}
          style={styles.notice}
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
        >
          {notice}
        </Text>
      )}
      {targets.length > 0 && (
        <SelectFieldButton<string>
          label={t('fileImport.list.calendar')}
          value={calendarId}
          options={targets.map((c) => ({ value: c.id, label: c.name }))}
          onChange={(id) => {
            calendarTouched.current = true;
            setCalendarId(id);
          }}
          disabled={busy}
        />
      )}
      {heldStatus != null && (
        <Text style={styles.hint} accessibilityRole="text">
          {heldStatus}
        </Text>
      )}
      <View
        accessibilityRole="list"
        accessibilityLabel={t('fileImport.list.listLabel')}
        style={styles.list}
      >
        {events.map((event, index) => {
          const key = `import-${index}`;
          if (imported.has(index)) {
            return (
              <Text key={key} style={styles.done} accessibilityRole="text">
                {`${event.title}, ${when(event)}, ${t('fileImport.list.imported')}`}
              </Text>
            );
          }
          const checked = chosen.has(index);
          const editLabel = t('fileImport.list.editLabel', { title: event.title });
          return (
            <View key={key} style={styles.row}>
              <Pressable
                accessible
                accessibilityRole={selectableRole('checkbox')}
                accessibilityState={{ ...selectableCheckState(checked), disabled: busy }}
                accessibilityLabel={importRowLabel(
                  event,
                  when(event),
                  isHeld(index),
                  checked,
                  calendarName,
                  t,
                )}
                accessibilityActions={[{ name: 'edit', label: editLabel }]}
                onAccessibilityAction={(e) => {
                  if (e.nativeEvent.actionName === 'edit') edit(event, index);
                }}
                disabled={busy}
                onPress={() =>
                  setChosen((prev) => {
                    const next = new Set(prev);
                    if (next.has(index)) next.delete(index);
                    else next.add(index);
                    return next;
                  })
                }
                style={({ pressed }) => [
                  styles.option,
                  checked && styles.optionChecked,
                  pressed && styles.pressed,
                ]}
              >
                <Text style={styles.marker} importantForAccessibility="no">
                  {checked ? '☑' : '☐'}
                </Text>
                <Text style={styles.optionLabel} importantForAccessibility="no">
                  {importRowLabel(event, when(event), isHeld(index), checked, calendarName, t)}
                </Text>
              </Pressable>
              {/* Sighted-only twin of the row's Edit action, taken out of the
                  reader's tree so the row stays one element. */}
              <Pressable
                accessibilityElementsHidden
                importantForAccessibility="no-hide-descendants"
                disabled={busy}
                onPress={() => edit(event, index)}
                style={({ pressed }) => [styles.editButton, pressed && styles.pressed]}
              >
                <Text style={styles.editButtonText}>{t('fileImport.list.edit')}</Text>
              </Pressable>
            </View>
          );
        })}
      </View>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: busy || toImport.length === 0 }}
        onPress={() => void importChosen()}
        style={({ pressed }) => [styles.primary, pressed && styles.primaryPressed]}
      >
        <Text style={styles.primaryText}>
          {t('fileImport.list.importChosen', { count: toImport.length })}
        </Text>
      </Pressable>
    </ScrollView>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    body: { gap: 14, padding: 16 },
    summary: { fontSize: 16, color: c.textPrimary },
    notice: {
      fontSize: 15,
      fontWeight: '600',
      color: c.danger,
      backgroundColor: c.dangerBg,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      borderRadius: 8,
      paddingHorizontal: 10,
      paddingVertical: 8,
    },
    hint: { fontSize: 14, color: c.textSecondary },
    list: { gap: 10 },
    row: { flexDirection: 'row', alignItems: 'center', gap: 10 },
    option: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
    },
    optionChecked: { borderColor: c.accent, backgroundColor: c.surfaceSelected },
    pressed: { backgroundColor: c.surfacePressed },
    marker: { fontSize: 20, width: 24, textAlign: 'center', color: c.textPrimary },
    optionLabel: { flex: 1, fontSize: 16, color: c.textPrimary },
    done: { fontSize: 15, color: c.textSecondary, paddingVertical: 8 },
    editButton: {
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.background,
    },
    editButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    primary: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryPressed: { backgroundColor: c.accentPressed },
    primaryText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
  });
