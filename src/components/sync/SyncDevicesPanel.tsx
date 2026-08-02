import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  canForgetDevice,
  deviceActivity,
  shortDeviceId,
  type DeviceRegistryRow,
} from '@aperio/shared';

import { useAnnouncer } from '../../a11y/announcerContext';
import { FocusableNote } from '../../a11y/FocusableNote';
import {
  forgetSyncDevice,
  isCommandError,
  listSyncDevices,
  setSyncDeviceName,
  syncDeviceName,
  type SyncDeviceSummary,
} from '../../api/client';
import { useDateFormat } from '../../intl/dateFormat';
import {
  SettingsSelectorDetail,
  type SettingsSelectorGroup,
} from '../SettingsSelectorDetail';
import { ConfirmDialog } from '../ConfirmDialog';
import { useSyncErrorMessage } from './syncErrorMessage';

/**
 * Settings → Synchronisation, the devices half: what this machine calls itself,
 * and who else the dataset still counts as a participant.
 *
 * ## Why there is a name field here at all
 *
 * There has only ever been one place to set it — the first-launch wizard's
 * connect form — and after sync targets became accounts, the everyday connect
 * path stopped going through that form. So a device set up from the settings
 * joined the dataset with no name, and every other device listed it as a
 * 32-character hex id with no way to fix it.
 *
 * Putting the field here rather than back in the connect form is deliberate:
 * the name is a property of THIS DEVICE, not of the target it syncs through. It
 * has to survive a change of target, and it has to be correctable without
 * tearing down a working connection.
 *
 * ## What removing a device does, and what it does not
 *
 * It drops the registry entry. That is worth doing for more than tidiness: the
 * compactor floors its GC cutoff at the lowest held horizon across every
 * REGISTERED device, so an entry left behind by a reinstall keeps log files
 * alive that nothing will ever read.
 *
 * It is not a revocation and it does not delete anything the device wrote. A
 * device that still runs re-registers on its next round, and its log files stay
 * until the snapshot covers them. The confirmation says both, because a user
 * looking at a list of eight ids needs to know that guessing wrong is cheap —
 * otherwise the honest response to an ambiguous row is to leave it there
 * forever, which is the state this panel exists to end.
 *
 * ## Accessibility
 *
 * - The list is the shared master/detail listbox
 *   ([`SettingsSelectorDetail`](../SettingsSelectorDetail.tsx)): one tab stop,
 *   arrow keys, selection follows focus. Each option's accessible name carries
 *   the name AND when the device was last here, so arrowing down the list is
 *   enough to find the leftovers — the judgement never requires opening a
 *   detail pane.
 * - Every line of prose is a [`FocusableNote`]. This panel lives inside the
 *   Settings modal, whose body is `role="application"`, where NVDA's focus-mode
 *   traversal skips static text entirely.
 * - Focus after a removal lands on the status note at the top, which is the one
 *   node that survives it — the pressed button is inside the detail pane of a
 *   row that has just stopped existing. Same reasoning as the target picker's.
 * - Failures move focus onto the message as well as announcing, so the reason
 *   is both spoken and re-readable. Imperative rather than in an effect: the
 *   same refusal twice in a row is a no-op re-render, so an effect keyed on the
 *   message would not re-run and the second press would be met with silence.
 */

/** Module scope so the memos inside the shared selector stay stable. */
const deviceId = (device: SyncDeviceSummary): string => device.id;

export interface SyncDevicesPanelProps {
  /** Whether this device currently has a working target. The registry lives on
   *  the remote, so there is nothing to list without one — and saying "no other
   *  devices" then would be a claim the panel cannot support. */
  configured: boolean;
}

export function SyncDevicesPanel({ configured }: SyncDevicesPanelProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const messageForError = useSyncErrorMessage();

  const [name, setName] = useState('');
  /** The host name, offered on a device that has never been named. Never
   *  written on the user's behalf: a suggestion the user did not accept is not
   *  a decision, and silently publishing the machine's host name to everyone
   *  else's device list is not ours to make. */
  const [suggested, setSuggested] = useState<string | null>(null);
  const [savedName, setSavedName] = useState<string | null>(null);
  /** Whether `savedName` has been read yet. Until it has, `null` means "not
   *  asked", which is not the same as "no name" — and this device's row is
   *  rendered from `savedName`, so treating the two alike would flash
   *  "Unnamed (a1b2c3d4…)" over a device that has been named for months. */
  const [nameLoaded, setNameLoaded] = useState(false);
  const [savingName, setSavingName] = useState(false);

  const [devices, setDevices] = useState<SyncDeviceSummary[]>([]);
  /** False until the first load has answered. Without it the empty list on the
   *  first render is stated as a fact, which the user cannot tell from the
   *  truth. */
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmForget, setConfirmForget] = useState<SyncDeviceSummary | null>(
    null,
  );

  const statusNoteRef = useRef<HTMLParagraphElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);

  const showError = useCallback((message: string) => {
    setError(message);
    requestAnimationFrame(() => {
      errorRef.current?.focus({ preventScroll: true });
    });
  }, []);

  const loadName = useCallback(async () => {
    try {
      const info = await syncDeviceName();
      setName(info.configured ?? '');
      setSavedName(info.configured);
      setSuggested(info.suggested);
      setNameLoaded(true);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('reading the device name failed', err);
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
      // eslint-disable-next-line no-console
      console.warn('listing the sync devices failed', err);
      // A target that is chosen but not running answers `not_configured`, and
      // that is a state the panel above already explains — repeating it as an
      // error here would put a red message under a sentence that has just said
      // the same thing more calmly.
      setDevices([]);
      setLoadError(
        isCommandError(err) && err.code === 'not_configured'
          ? null
          : messageForError(err),
      );
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

  /** When a device was last here, in one phrase.
   *
   *  Three cases and no fourth: this device, a relative time, and "unknown".
   *  The unknown case is a registry written before the stamp existed — it is
   *  NOT filled in from the content horizon, which is the very confusion the
   *  stamp was added to end. */
  const activityPhrase = useCallback(
    (device: SyncDeviceSummary) => {
      const activity = deviceActivity(device as DeviceRegistryRow);
      switch (activity.kind) {
        case 'self':
          return t('dialogs.settings.sync.deviceThisOne');
        case 'seen':
          // Absolute, like every other timestamp in the app ("Letzter
          // erfolgreicher Abgleich", the cache's "zuletzt aktualisiert"). A
          // relative form reads faster but drifts as the panel stays open, and
          // the question being asked here — is this the device I stopped using
          // in spring — is answered by a date, not by "vor 4 Monaten".
          return t('dialogs.settings.sync.deviceLastSeen', {
            when: fmt.format(activity.at, 'PPPp'),
          });
        case 'unknown':
          return t('dialogs.settings.sync.deviceLastSeenUnknown');
      }
    },
    [fmt, t],
  );

  /** What to call a device in the list.
   *
   *  For THIS device the local preference wins over the registry, and that is
   *  not a nicety — it is the only correct reading. Renaming writes the
   *  preference; the registry copy is published by the next heartbeat, which
   *  can be a quarter of an hour away. Rendering our own row from the registry
   *  meant saving a name and watching the list go on showing the old one, with
   *  nothing on screen to say why.
   *
   *  So the row states what this device IS called and the hint under the field
   *  says when the others will hear about it. Note the fallback does not run
   *  through `device.name` when the local name is empty: clearing a name has to
   *  show the id straight away, and falling back would have resurrected the
   *  registry's stale copy of the name just deleted.
   *
   *  `nameLoaded` gates the whole thing, because before the read answers,
   *  `savedName` is `null` for "not asked yet" and would read as "no name". */
  const displayName = useCallback(
    (device: SyncDeviceSummary) => {
      const local =
        device.is_this_device && nameLoaded ? (savedName ?? '') : device.name;
      return (
        local?.trim() ||
        t('dialogs.settings.sync.deviceUnnamed', {
          id: shortDeviceId(device.id),
        })
      );
    },
    [nameLoaded, savedName, t],
  );

  /** One group. The selector takes groups because the account pickers need
   *  them; a registry has no second axis to split on, and inventing one would
   *  add a level of headings for nothing. */
  const groups: SettingsSelectorGroup<SyncDeviceSummary>[] = useMemo(
    () =>
      devices.length === 0
        ? []
        : [
            {
              id: 'devices',
              label: t('dialogs.settings.sync.devicesGroupLabel'),
              items: devices,
            },
          ],
    [devices, t],
  );

  const runForget = useCallback(async () => {
    const device = confirmForget;
    setConfirmForget(null);
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
      // The pressed button lived in the detail pane of a row that no longer
      // exists. The status note is the one node that outlives the change, and
      // its text has by then become the new state.
      requestAnimationFrame(() => {
        statusNoteRef.current?.focus({ preventScroll: true });
      });
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('forget_sync_device failed', err);
      showError(messageForError(err));
    } finally {
      setBusyId(null);
    }
  }, [
    announce,
    confirmForget,
    displayName,
    loadDevices,
    messageForError,
    showError,
    t,
  ]);

  const otherCount = devices.filter((d) => !d.is_this_device).length;

  return (
    <div className="sync-panel__devices">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.sync.deviceName')}
        </span>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
          // The host name as a placeholder, not as a value: it says what the
          // field would sensibly hold without claiming the user chose it.
          placeholder={suggested ?? ''}
          autoComplete="off"
        />
      </label>
      <FocusableNote className="sync-panel__hint">
        {t('dialogs.settings.sync.deviceNameHint')}
      </FocusableNote>
      <div className="sync-panel__actions">
        <button
          type="button"
          // aria-disabled, not native `disabled`: a button that disables itself
          // mid-round-trip loses focus to <body>, which drops NVDA out of the
          // dialog. Same reasoning as everywhere else on this panel.
          aria-disabled={savingName || name.trim() === (savedName ?? '')}
          aria-busy={savingName}
          onClick={() => {
            if (!savingName && name.trim() !== (savedName ?? '')) void saveName();
          }}
        >
          {t('dialogs.settings.sync.deviceNameSave')}
        </button>
        {suggested && !name.trim() && (
          <button type="button" onClick={() => setName(suggested)}>
            {t('dialogs.settings.sync.deviceNameUseSuggested', {
              name: suggested,
            })}
          </button>
        )}
      </div>

      <FocusableNote ref={statusNoteRef} className="sync-panel__hint">
        {!configured
          ? t('dialogs.settings.sync.devicesNoTarget')
          : !loaded
            ? t('dialogs.accounts.loading')
            : otherCount === 0
              ? // Its own key rather than a `_zero` plural: German and English
                // have no zero category in CLDR, so `_zero` would never be
                // selected and the sentence would be dead in the file.
                t('dialogs.settings.sync.devicesStatusNone')
              : t('dialogs.settings.sync.devicesStatus', {
                  count: otherCount,
                })}
      </FocusableNote>
      <FocusableNote className="sync-panel__hint">
        {t('dialogs.settings.sync.devicesIntro')}
      </FocusableNote>

      {error && (
        <FocusableNote ref={errorRef} className="sync-panel__error form__error">
          {error}
        </FocusableNote>
      )}
      {loadError && (
        <FocusableNote className="sync-panel__error form__error">
          {t('dialogs.settings.sync.devicesLoadFailed', {
            message: loadError,
          })}
        </FocusableNote>
      )}

      {configured && loaded && groups.length > 0 && (
        <SettingsSelectorDetail<SyncDeviceSummary>
          groups={groups}
          getItemId={deviceId}
          getItemName={displayName}
          getItemSummary={activityPhrase}
          selectorLabel={t('dialogs.settings.sync.devicesSelectorLabel')}
          optionLabel={({ name: rowName, summary }) =>
            t('dialogs.settings.sync.deviceOptionLabel', {
              name: rowName,
              summary,
            })
          }
          detailHeading={({ name: rowName }) =>
            t('dialogs.settings.sync.deviceDetailHeading', { name: rowName })
          }
          // The panel's own "Geräte" is an <h3>; a second <h3> in the same
          // section would read as its sibling rather than as the selected
          // row's detail.
          detailHeadingLevel={4}
          renderDetail={(device) => {
            const busy = busyId === device.id;
            // Gate on ANY removal being in flight, not just this row's:
            // selection follows focus, so arrowing away during a round trip
            // and pressing another row's button would race two writes of the
            // same meta.json against each other.
            const blocked = busyId !== null;
            return (
              <>
                <FocusableNote className="sync-panel__hint">
                  {activityPhrase(device)}
                </FocusableNote>
                <FocusableNote className="sync-panel__hint">
                  {t('dialogs.settings.sync.deviceAppVersion', {
                    version: device.app_version,
                  })}
                </FocusableNote>
                {/* The full id. Spoken it is thirty-two characters, which is
                    why it is not the row's name — but it is what a user
                    matching this list against another device's settings needs,
                    so it is reachable here rather than nowhere. */}
                <FocusableNote className="sync-panel__hint">
                  {t('dialogs.settings.sync.deviceIdFull', { id: device.id })}
                </FocusableNote>
                {device.stale && (
                  <FocusableNote className="sync-panel__hint">
                    {t('dialogs.settings.sync.deviceStaleNote')}
                  </FocusableNote>
                )}
                {canForgetDevice(device as DeviceRegistryRow) && (
                  <div className="sync-panel__actions">
                    <button
                      type="button"
                      aria-disabled={blocked}
                      aria-busy={busy}
                      onClick={() => {
                        if (!blocked) setConfirmForget(device);
                      }}
                    >
                      {t('dialogs.settings.sync.deviceForget', {
                        name: displayName(device),
                      })}
                    </button>
                  </div>
                )}
              </>
            );
          }}
        />
      )}

      <ConfirmDialog
        isOpen={confirmForget !== null}
        onClose={() => setConfirmForget(null)}
        onConfirm={() => void runForget()}
        title={t('dialogs.settings.sync.deviceForgetTitle')}
        // States what it does AND what it does not: the entry goes, the data
        // stays, and a device that still runs comes back. A user pruning eight
        // ids has to know that guessing wrong is cheap, or the safe move is to
        // leave every ambiguous row alone forever.
        message={t('dialogs.settings.sync.deviceForgetConfirm', {
          name: confirmForget ? displayName(confirmForget) : '',
        })}
        confirmLabel={t('dialogs.settings.sync.deviceForgetConfirmLabel')}
      />
    </div>
  );
}
