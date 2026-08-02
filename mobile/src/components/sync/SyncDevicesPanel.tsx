import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';
import {
  canForgetDevice,
  deviceActivity,
  shortDeviceId,
  type DeviceRegistryRow,
} from '@aperio/shared';

import {
  forgetSyncDevice,
  listSyncDevices,
  setSyncDeviceName,
  syncDeviceName,
  type SyncDeviceSummary,
} from '../../api/sync';
import { useSyncErrorMessage } from '../../api/syncErrorMessage';
import { formatLongDateTime } from '../../intl/dateFormat';
import { useThemedStyles, type ThemeColors } from '../../theme';
import { AppDialog } from '../AppDialog';

/**
 * Settings → Sync, the devices half: what this phone calls itself, and who else
 * the dataset still counts as a participant.
 *
 * The desktop twin is `src/components/sync/SyncDevicesPanel.tsx` — same
 * behaviour and the same locale keys, different markup. A phone has no
 * master/detail listbox, so what would be a detail pane there rides on the row
 * here, exactly as the target picker next door already does.
 *
 * ## Why the name field is here
 *
 * It used to live in the per-backend connect form, which only the first-launch
 * wizard still uses — so a device set up from the settings joined the dataset
 * nameless and appeared in every other device's list as a 32-character hex id
 * with no screen anywhere to fix it. The name is a property of THIS DEVICE
 * rather than of the target it syncs through: it has to survive a change of
 * target and be correctable without tearing down a working connection.
 *
 * The suggestion is `Constants.deviceName` — what the user called this phone in
 * its own settings — offered, never stored on their behalf.
 *
 * ## What removing a device does
 *
 * It drops the registry entry, which is worth more than tidiness: the compactor
 * floors its GC cutoff at the lowest held horizon across every REGISTERED
 * device, so an entry left behind by a reinstall keeps log files alive that
 * nothing will ever read.
 *
 * It is not a revocation. No data is deleted, the device's log files stay until
 * the snapshot covers them, and a device that still runs re-registers on its
 * next round. The confirmation says all three, because someone looking at a
 * list of hex ids has to know that guessing wrong is cheap — otherwise the
 * rational move is to leave every ambiguous row alone forever, which is the
 * state this panel exists to end.
 *
 * ## Accessibility
 *
 * - Each device is one addressable row whose accessible name carries the name
 *   AND when it was last here, so swiping through the list is enough to find
 *   the leftovers — no row has to be opened to be judged.
 * - Removing is a real button AND a custom accessibility action: on iOS the row
 *   is a single accessible element, so the button inside it is not reachable
 *   and the rotor action is the only way a VoiceOver user can activate it.
 * - Focus after a removal lands on the status note, the one node that outlives
 *   a row that has just stopped existing. Announced too, imperatively in the
 *   handler — `accessibilityState.busy` has no VoiceOver equivalent and a
 *   changed label is not re-read for the element that already has focus.
 * - Refusals are announced AND focused, both carrying the same sentence. Not a
 *   live region: TalkBack would then say it twice, and an effect keyed on the
 *   message never re-runs when the same refusal is set twice.
 */

export interface SyncDevicesPanelProps {
  /** Whether this device currently has a working target. The registry lives on
   *  the remote, so there is nothing to list without one — and "no other
   *  devices" would then be a claim this panel cannot support. */
  configured: boolean;
}

export function SyncDevicesPanel({ configured }: SyncDevicesPanelProps) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  const messageForError = useSyncErrorMessage();

  const [name, setName] = useState('');
  const [suggested, setSuggested] = useState<string | null>(null);
  const [savedName, setSavedName] = useState<string | null>(null);
  const [savingName, setSavingName] = useState(false);

  const [devices, setDevices] = useState<SyncDeviceSummary[]>([]);
  /** False until the first load has answered — an empty list on the first
   *  render is a loading state, not a fact about the dataset. */
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [forgetFor, setForgetFor] = useState<SyncDeviceSummary | null>(null);

  const statusRef = useRef<Text>(null);
  const errorRef = useRef<Text>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const focusOn = useCallback((node: Text | null) => {
    const tag = node ? findNodeHandle(node) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  /** Say it and land on it, in that order and both imperatively. */
  const showError = useCallback(
    (message: string) => {
      setError(message);
      announce(message);
      requestAnimationFrame(() => focusOn(errorRef.current));
    },
    [announce, focusOn],
  );

  const loadName = useCallback(async () => {
    try {
      const info = await syncDeviceName();
      setName(info.configured ?? '');
      setSavedName(info.configured);
      setSuggested(info.suggested);
    } catch {
      // A name that cannot be read is not a failure of this screen — the field
      // starts empty and saving still works. Reporting it would put an error
      // over a panel whose real job is fine.
    }
  }, []);

  const loadDevices = useCallback(async () => {
    if (!configured) {
      setDevices([]);
      setLoadError(null);
      setLoaded(true);
      return;
    }
    try {
      setDevices(await listSyncDevices());
      setLoadError(null);
    } catch (err) {
      setDevices([]);
      setLoadError(messageForError(err));
    } finally {
      setLoaded(true);
    }
  }, [configured, messageForError]);

  useEffect(() => {
    void loadName();
  }, [loadName]);

  useEffect(() => {
    void loadDevices();
  }, [loadDevices]);

  const saveName = useCallback(async () => {
    if (savingName) return;
    const trimmed = name.trim();
    setSavingName(true);
    setError(null);
    try {
      await setSyncDeviceName(trimmed);
      setSavedName(trimmed || null);
      announce(
        trimmed
          ? t('dialogs.settings.sync.deviceNameSaved', { name: trimmed })
          : t('dialogs.settings.sync.deviceNameCleared'),
      );
    } catch (err) {
      showError(messageForError(err));
    } finally {
      setSavingName(false);
    }
  }, [announce, messageForError, name, savingName, showError, t]);

  /** When a device was last here, in one phrase. Three cases and no fourth —
   *  and the unknown case is NOT filled in from the content horizon, which is
   *  the confusion the wall-clock stamp was added to end. */
  const activityPhrase = useCallback(
    (device: SyncDeviceSummary) => {
      const activity = deviceActivity(device as DeviceRegistryRow);
      switch (activity.kind) {
        case 'self':
          return t('dialogs.settings.sync.deviceThisOne');
        case 'seen':
          // Absolute, like every other timestamp on this screen.
          return t('dialogs.settings.sync.deviceLastSeen', {
            when: formatLongDateTime(activity.at, i18n.language),
          });
        case 'unknown':
          return t('dialogs.settings.sync.deviceLastSeenUnknown');
      }
    },
    [i18n.language, t],
  );

  const displayName = useCallback(
    (device: SyncDeviceSummary) =>
      device.name?.trim() ||
      t('dialogs.settings.sync.deviceUnnamed', {
        id: shortDeviceId(device.id),
      }),
    [t],
  );

  const runForget = useCallback(async () => {
    const device = forgetFor;
    setForgetFor(null);
    if (!device) return;
    setBusyId(device.id);
    setError(null);
    try {
      await forgetSyncDevice(device.id);
      await loadDevices();
      announce(
        t('dialogs.settings.sync.deviceForgotten', {
          name: displayName(device),
        }),
      );
      requestAnimationFrame(() => focusOn(statusRef.current));
    } catch (err) {
      showError(messageForError(err));
    } finally {
      setBusyId(null);
    }
  }, [
    announce,
    displayName,
    focusOn,
    forgetFor,
    loadDevices,
    messageForError,
    showError,
    t,
  ]);

  const otherCount = devices.filter((d) => !d.is_this_device).length;
  const nameDirty = name.trim() !== (savedName ?? '');

  return (
    <>
      <Text style={styles.label} nativeID="sync-device-name-label">
        {t('dialogs.settings.sync.deviceName')}
      </Text>
      <TextInput
        value={name}
        onChangeText={setName}
        // The phone's own name as a placeholder, not as a value: it says what
        // the field would sensibly hold without claiming the user chose it.
        placeholder={suggested ?? ''}
        accessibilityLabel={t('dialogs.settings.sync.deviceName')}
        autoCapitalize="words"
        autoCorrect={false}
        style={styles.input}
      />
      <Text style={styles.hint}>
        {t('dialogs.settings.sync.deviceNameHint')}
      </Text>
      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: savingName || !nameDirty }}
        accessibilityLabel={t('dialogs.settings.sync.deviceNameSave')}
        disabled={savingName || !nameDirty}
        onPress={() => void saveName()}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>
          {t('dialogs.settings.sync.deviceNameSave')}
        </Text>
      </Pressable>
      {suggested != null && !name.trim() && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.settings.sync.deviceNameUseSuggested', {
            name: suggested,
          })}
          onPress={() => setName(suggested)}
          style={({ pressed }) => [
            styles.ghostButton,
            pressed && styles.pressed,
          ]}
        >
          <Text style={styles.ghostButtonText}>
            {t('dialogs.settings.sync.deviceNameUseSuggested', {
              name: suggested,
            })}
          </Text>
        </Pressable>
      )}

      <Text ref={statusRef} accessibilityRole="text" style={styles.status}>
        {!configured
          ? t('dialogs.settings.sync.devicesNoTarget')
          : !loaded
            ? t('mobile.loading')
            : otherCount === 0
              ? // Its own key rather than a `_zero` plural: neither German nor
                // English has a zero category in CLDR, so `_zero` would never
                // be selected and the sentence would be dead in the file.
                t('dialogs.settings.sync.devicesStatusNone')
              : t('dialogs.settings.sync.devicesStatus', {
                  count: otherCount,
                })}
      </Text>
      <Text style={styles.hint}>
        {t('dialogs.settings.sync.devicesIntro')}
      </Text>

      {error != null && (
        <Text ref={errorRef} accessibilityRole="text" style={styles.error}>
          {error}
        </Text>
      )}
      {loadError != null && (
        <Text accessibilityRole="text" style={styles.error}>
          {t('dialogs.settings.sync.devicesLoadFailed', { message: loadError })}
        </Text>
      )}

      {configured && loaded && devices.length > 0 && (
        <View style={styles.list}>
          {devices.map((device) => {
            const rowName = displayName(device);
            const summary = activityPhrase(device);
            const removable = canForgetDevice(device as DeviceRegistryRow);
            // Gate on ANY removal in flight, not just this row's: two
            // concurrent writes of the same meta.json race each other, and on
            // iOS the rotor action is the only way to reach this at all, so a
            // per-row gate would leave exactly those users able to trigger it.
            const blocked = busyId !== null;
            const forgetLabel = t('dialogs.settings.sync.deviceForget', {
              name: rowName,
            });
            return (
              <View
                key={device.id}
                accessible
                accessibilityRole="text"
                accessibilityLabel={t(
                  'dialogs.settings.sync.deviceOptionLabel',
                  { name: rowName, summary },
                )}
                accessibilityActions={
                  removable && !blocked
                    ? [{ name: 'forget', label: forgetLabel }]
                    : undefined
                }
                onAccessibilityAction={(e) => {
                  if (e.nativeEvent.actionName === 'forget' && !blocked) {
                    setForgetFor(device);
                  }
                }}
                style={[styles.row, device.is_this_device && styles.rowSelf]}
              >
                <Text style={styles.deviceName}>{rowName}</Text>
                <Text style={styles.rowNote}>{summary}</Text>
                <Text style={styles.rowNote}>
                  {t('dialogs.settings.sync.deviceAppVersion', {
                    version: device.app_version,
                  })}
                </Text>
                <Text style={styles.rowNote}>
                  {t('dialogs.settings.sync.deviceIdFull', { id: device.id })}
                </Text>
                {device.stale && (
                  <Text style={styles.rowNote}>
                    {t('dialogs.settings.sync.deviceStaleNote')}
                  </Text>
                )}
                {removable && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityState={{ disabled: blocked }}
                    accessibilityLabel={forgetLabel}
                    disabled={blocked}
                    onPress={() => setForgetFor(device)}
                    style={({ pressed }) => [
                      styles.smallButton,
                      pressed && styles.pressed,
                    ]}
                  >
                    <Text style={styles.smallButtonText}>{forgetLabel}</Text>
                  </Pressable>
                )}
              </View>
            );
          })}
        </View>
      )}

      {/* States what goes AND what stays. See the component doc. */}
      <AppDialog
        visible={forgetFor != null}
        title={t('dialogs.settings.sync.deviceForgetTitle')}
        message={t('dialogs.settings.sync.deviceForgetConfirm', {
          name: forgetFor ? displayName(forgetFor) : '',
        })}
        confirmLabel={t('dialogs.settings.sync.deviceForgetConfirmLabel')}
        cancelLabel={t('mobile.cancel')}
        destructive
        onConfirm={() => void runForget()}
        onCancel={() => setForgetFor(null)}
      />
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    label: { fontSize: 15, fontWeight: '700', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    status: { fontSize: 16, color: c.textPrimary, fontWeight: '600' },
    error: { fontSize: 14, color: c.danger },
    input: {
      borderWidth: 1,
      borderColor: c.border,
      borderRadius: 10,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 16,
      color: c.textPrimary,
      backgroundColor: c.surface,
    },
    list: { gap: 12 },
    // A column, not a row: the action is named after the device ("Test-Laptop
    // entfernen"), and a long name beside the text squeezes the button into a
    // wrapping sliver on a phone.
    row: {
      gap: 6,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowSelf: { borderColor: c.accent },
    deviceName: { fontSize: 18, color: c.textPrimary, fontWeight: '600' },
    rowNote: { fontSize: 13, color: c.textSecondary },
    ghostButton: {
      paddingVertical: 14,
      paddingHorizontal: 16,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 16, color: c.textPrimary },
    smallButton: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      alignItems: 'center',
    },
    smallButtonText: { fontSize: 15, color: c.textPrimary },
    pressed: { opacity: 0.7 },
  });
