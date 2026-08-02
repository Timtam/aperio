import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { FocusableNote } from '../a11y/FocusableNote';

import {
  adoptRemoteEncryption,
  changeSyncPassphrase,
  compactNow,
  configureSyncAdapter,
  disableSyncEncryption,
  enableSyncEncryption,
  getSyncAdapterSummary,
  isCommandError,
  setSyncInterval,
  type SyncAdapterSummary,
} from '../api/client';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/dialogStateContext';
import { useSync } from '../state/useSync';
import { ConfirmDialog } from './ConfirmDialog';
import { SyncProtocolSection } from './SyncProtocolSection';
import { SyncDevicesPanel } from './sync/SyncDevicesPanel';
import { SyncTargetAccountPicker } from './sync/SyncTargetAccountPicker';
import { useSyncErrorMessage } from './sync/syncErrorMessage';

/**
 * Settings → Synchronisation panel (DESIGN.md §19, Phase Si).
 *
 *   1. **State** — connection state + last successful sync + manual
 *      `Sync now` / `Compact now`.
 *   2. **Interval** — periodic-sync preset.
 *   3. **Target** — WHICH of the user's accounts holds the dataset
 *      ([`SyncTargetAccountPicker`](./sync/SyncTargetAccountPicker.tsx)),
 *      plus the summary of the current one and Disconnect.
 *   4. **E2E** — adopt / enable / change-passphrase / disable encryption.
 *   5. **Protocol** — the §19.9 sync history.
 *
 * ## Why there is no connect form here any more
 *
 * This panel used to ask what KIND of target to use and then for its host,
 * path and password, through the shared
 * [`SyncTargetConfigForm`](./sync/SyncTargetConfigForm.tsx). A sync target is
 * an account now: the user adds it under Settings → Accounts like any other,
 * or it arrives with a restored dataset, and the only decision left for this
 * device is which of them holds the dataset. That is the whole of the picker.
 *
 * The form itself is unchanged and still rendered by the first-launch wizard,
 * which is the one place that legitimately has no accounts yet AND has to
 * decide between joining an existing dataset and starting a new one.
 */

const INTERVAL_PRESETS: readonly number[] = [1, 5, 15, 30, 60, 240];

export function SyncPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const fmt = useDateFormat();
  const { openSyncConflicts } = useDialogState();
  const {
    status,
    refreshStatus,
    lastReport,
    lastError,
    conflictCount,
    triggering,
    triggerSync,
  } = useSync();
  // Shared sync error → message mapping, used by the E2E sections below. The
  // account picker uses the same hook internally, so both render identical
  // wording for the same backend code.
  const messageForError = useSyncErrorMessage();

  const stateHeadingId = useId();
  const adapterHeadingId = useId();
  // Where focus lands after Disconnect. The button that triggers it lives
  // inside the connected-summary block, which the same action unmounts, so
  // focus would otherwise drop to <body> — outside the settings dialog's
  // role="application", where a screen reader has nothing to read and no
  // anchor to arrow from. The section heading is the one node that survives
  // the swap, and it names the section the user is standing in.
  const adapterHeadingRef = useRef<HTMLHeadingElement>(null);
  const intervalHeadingId = useId();
  const protocolHeadingId = useId();
  const passphraseHeadingId = useId();
  const enableE2eHeadingId = useId();
  const devicesHeadingId = useId();

  // The target CONFIGURATION form (kind picker, per-kind fields, OAuth, the
  // device name, preview→join/init) lives in `SyncTargetConfigForm` and is now
  // rendered only by the first-launch wizard. This panel picks between
  // accounts and keeps the periodic-interval, compaction and E2E controls.
  const [intervalDraft, setIntervalDraft] = useState<number | null>(null);
  const [busyCompact, setBusyCompact] = useState(false);
  const [busyDisconnect, setBusyDisconnect] = useState(false);
  // Disconnecting is the one irreversible thing on this screen — it drops the
  // account row and its credentials, not just the pointer — and it used to
  // happen on a single unguarded press. The confirmation dialog says what is
  // actually lost; its focus opens on Cancel.
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);
  // §19.7 passphrase rotation.
  const [oldPassphraseDraft, setOldPassphraseDraft] = useState('');
  const [newPassphraseDraft, setNewPassphraseDraft] = useState('');
  const [busyPassphraseChange, setBusyPassphraseChange] = useState(false);
  const [passphraseChangeError, setPassphraseChangeError] = useState<
    string | null
  >(null);
  const [passphraseChangeOk, setPassphraseChangeOk] = useState(false);
  // §19.7 disable-E2E flow (reuses the same current-passphrase input).
  const [busyDisable, setBusyDisable] = useState(false);
  const [disableError, setDisableError] = useState<string | null>(null);
  // §19.7 enable-E2E flow on an already-configured but unencrypted dataset.
  const [enableNewPpDraft, setEnableNewPpDraft] = useState('');
  const [busyEnable, setBusyEnable] = useState(false);
  const [enableError, setEnableError] = useState<string | null>(null);
  const [enableOk, setEnableOk] = useState(false);
  // §19.7 cross-device adoption banner.
  const [adoptRemotePpDraft, setAdoptRemotePpDraft] = useState('');
  const [busyAdoptRemote, setBusyAdoptRemote] = useState(false);
  const [adoptRemoteError, setAdoptRemoteError] = useState<string | null>(null);
  // Compact non-secret summary of the persisted adapter config. `null` when no
  // adapter is configured (the form takes over).
  const [adapterSummary, setAdapterSummary] =
    useState<SyncAdapterSummary | null>(null);

  const interval = intervalDraft ?? status?.interval_minutes ?? 5;

  const onIntervalChange = useCallback(
    async (raw: string) => {
      const parsed = Number(raw);
      if (!Number.isFinite(parsed) || parsed < 1) return;
      setIntervalDraft(parsed);
      try {
        const persisted = await setSyncInterval(parsed);
        setIntervalDraft(persisted);
        announce(
          t('dialogs.settings.sync.intervalChanged', { minutes: persisted }),
        );
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('set_sync_interval failed', err);
        setIntervalDraft(status?.interval_minutes ?? 5);
      }
    },
    [announce, status?.interval_minutes, t],
  );

  const onDisconnect = useCallback(async () => {
    setBusyDisconnect(true);
    try {
      await configureSyncAdapter({ kind: 'none' });
      setAdapterSummary(null);
      // The summary is only half of what this panel renders. "Encrypted
      // dataset", "Sync now" and "Compact now" all hang off the sync STATUS,
      // which is pulled once on mount and refreshed only by engine events —
      // and disconnecting emits none. Without this the panel opened the
      // account picker for a new target while still describing the old one as
      // connected and encrypted, with both action buttons live; pressing "Sync
      // now" then failed, because it was acting on a target that no longer
      // existed.
      // React unmounts the summary — and the Disconnect button inside it — on
      // the state change above. Focus is NOT moved here: the picker below
      // watches the same transition and lands on its status note, whose text
      // has become "this device syncs through no account". Saying it here as
      // well, and landing on a heading that reads only "Sync target", meant two
      // sentences racing and neither of them the answer.
      await refreshStatus();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('configure_sync_adapter(none) failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyDisconnect(false);
    }
  }, [announce, messageForError, refreshStatus, t]);

  const onCompact = useCallback(async () => {
    setBusyCompact(true);
    try {
      const report = await compactNow();
      announce(
        t('dialogs.settings.sync.compactDone', {
          deleted: report.deleted_logs,
          stale: report.stale_devices,
        }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('compact_now failed', err);
      announce(
        `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        'assertive',
      );
    } finally {
      setBusyCompact(false);
    }
  }, [announce, messageForError, t]);

  // §19.7 — drive the passphrase change.
  const onChangePassphrase = useCallback(async () => {
    setPassphraseChangeOk(false);
    setPassphraseChangeError(null);
    const oldPp = oldPassphraseDraft.trim();
    const newPp = newPassphraseDraft.trim();
    if (!oldPp || !newPp) {
      setPassphraseChangeError(
        t('dialogs.settings.sync.passphraseChangeErrorEmpty'),
      );
      return;
    }
    if (oldPp === newPp) {
      setPassphraseChangeError(
        t('dialogs.settings.sync.passphraseChangeErrorSame'),
      );
      return;
    }
    setBusyPassphraseChange(true);
    try {
      await changeSyncPassphrase(oldPp, newPp);
      setPassphraseChangeOk(true);
      setOldPassphraseDraft('');
      setNewPassphraseDraft('');
      announce(t('dialogs.settings.sync.passphraseChangeOk'));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('change_sync_passphrase failed', err);
      if (isCommandError(err) && err.code === 'auth') {
        setPassphraseChangeError(
          t('dialogs.settings.sync.passphraseChangeErrorAuth'),
        );
      } else {
        setPassphraseChangeError(messageForError(err));
      }
    } finally {
      setBusyPassphraseChange(false);
    }
  }, [announce, messageForError, newPassphraseDraft, oldPassphraseDraft, t]);

  // §19.7 — turn off E2E on the dataset.
  const onDisableEncryption = useCallback(async () => {
    setDisableError(null);
    const oldPp = oldPassphraseDraft.trim();
    if (!oldPp) {
      setDisableError(
        t('dialogs.settings.sync.disableE2eErrorNeedsPassphrase'),
      );
      return;
    }
    const confirmed = window.confirm(
      t('dialogs.settings.sync.disableE2eConfirm'),
    );
    if (!confirmed) return;
    setBusyDisable(true);
    try {
      const report = await disableSyncEncryption(oldPp);
      setOldPassphraseDraft('');
      setNewPassphraseDraft('');
      announce(
        t('dialogs.settings.sync.disableE2eOkAnnouncement', {
          logs: report.logs_rewritten,
        }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('disable_sync_encryption failed', err);
      if (isCommandError(err) && err.code === 'auth') {
        setDisableError(t('dialogs.settings.sync.passphraseChangeErrorAuth'));
      } else {
        setDisableError(messageForError(err));
      }
    } finally {
      setBusyDisable(false);
    }
  }, [announce, messageForError, oldPassphraseDraft, t]);

  // §19.7 — turn ON encryption for a dataset adopted without it.
  const onEnableEncryption = useCallback(async () => {
    setEnableError(null);
    setEnableOk(false);
    const newPp = enableNewPpDraft.trim();
    if (!newPp) {
      setEnableError(t('dialogs.settings.sync.enableE2eErrorNeedsPassphrase'));
      return;
    }
    const confirmed = window.confirm(
      t('dialogs.settings.sync.enableE2eConfirm'),
    );
    if (!confirmed) return;
    setBusyEnable(true);
    try {
      const report = await enableSyncEncryption(newPp);
      setEnableNewPpDraft('');
      setEnableOk(true);
      announce(
        t('dialogs.settings.sync.enableE2eOkAnnouncement', {
          logs: report.logs_rewritten,
        }),
      );
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('enable_sync_encryption failed', err);
      if (isCommandError(err) && err.code === 'conflict') {
        setEnableError(t('dialogs.settings.sync.enableE2eErrorConflict'));
      } else {
        setEnableError(messageForError(err));
      }
    } finally {
      setBusyEnable(false);
    }
  }, [announce, enableNewPpDraft, messageForError, t]);

  // §19.7 — adopt encryption activated on another device.
  const onAdoptRemoteEncryption = useCallback(async () => {
    setAdoptRemoteError(null);
    const pp = adoptRemotePpDraft.trim();
    if (!pp) {
      setAdoptRemoteError(
        t('dialogs.settings.sync.enableE2eErrorNeedsPassphrase'),
      );
      return;
    }
    setBusyAdoptRemote(true);
    try {
      await adoptRemoteEncryption(pp);
      setAdoptRemotePpDraft('');
      announce(t('dialogs.settings.sync.adoptRemoteE2eOk'));
      void triggerSync();
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('adopt_remote_encryption failed', err);
      if (isCommandError(err) && err.code === 'auth') {
        setAdoptRemoteError(t('dialogs.settings.sync.passphraseChangeErrorAuth'));
      } else {
        setAdoptRemoteError(messageForError(err));
      }
    } finally {
      setBusyAdoptRemote(false);
    }
  }, [adoptRemotePpDraft, announce, messageForError, t, triggerSync]);

  const lastSyncedLabel = (() => {
    if (!status?.last_synced_at)
      return t('dialogs.settings.sync.stateNeverSynced');
    try {
      const dt = new Date(status.last_synced_at);
      return t('dialogs.settings.sync.stateLastSynced', {
        time: fmt.format(dt, 'PPPp'),
      });
    } catch {
      return status.last_synced_at;
    }
  })();

  // Reload the persisted-adapter summary (mount + after configure/disconnect).
  // Awaitable: the picker lands screen-reader focus on a note whose text is
  // derived from THIS answer, so a fire-and-forget refresh would have it read
  // the previous target one beat after the user chose a new one.
  const refreshAdapterSummary = useCallback(async () => {
    try {
      setAdapterSummary(await getSyncAdapterSummary());
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('get_sync_adapter_summary failed', err);
      setAdapterSummary(null);
    }
  }, []);

  useEffect(() => {
    void refreshAdapterSummary();
  }, [refreshAdapterSummary, status?.configured]);

  // Re-read both halves after the picker points this device somewhere else.
  // Neither the summary nor the status re-reads itself: pointing at another
  // account emits no engine event, so without this the panel would go on
  // naming the account the dataset used to be on. Both awaited before the
  // picker moves focus — see above.
  const onTargetChanged = useCallback(async () => {
    await refreshAdapterSummary();
    await refreshStatus();
  }, [refreshAdapterSummary, refreshStatus]);

  return (
    <div className="sync-panel">
      <section aria-labelledby={stateHeadingId}>
        <h3 id={stateHeadingId}>{t('dialogs.settings.sync.stateTitle')}</h3>
        <FocusableNote className="sync-panel__hint">
          {status?.configured
            ? t('dialogs.settings.sync.stateConfiguredSimple')
            : adapterSummary
              ? // A target is chosen and the engine did not come up on it —
                // a start-up restore that refused (locked keychain, missing
                // credential, unconfirmed host key, missing plugin). Saying
                // "no sync adapter configured yet" here was the third of
                // three contradictory sentences on one screen, and the only
                // one of them the user could act on was none.
                t('dialogs.settings.sync.stateChosenNotRunning')
              : t('dialogs.settings.sync.stateUnconfigured')}
        </FocusableNote>
        {status?.configured && (
          <FocusableNote className="sync-panel__hint">
            {status.e2e_enabled
              ? t('dialogs.settings.sync.e2eActive')
              : t('dialogs.settings.sync.e2eInactive')}
          </FocusableNote>
        )}
        {status?.configured && (
          <FocusableNote className="sync-panel__hint">
            {status.e2e_enabled
              ? t('dialogs.settings.sync.credentialsSyncedNote')
              : t('dialogs.settings.sync.credentialsLocalNote')}
          </FocusableNote>
        )}
        <FocusableNote className="sync-panel__hint">
          {lastSyncedLabel}
        </FocusableNote>
        {lastReport && (
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.lastReport', {
              pushed: lastReport.pushed_logs,
              fetched: lastReport.fetched_logs,
              applied: lastReport.applied,
            })}
          </FocusableNote>
        )}
        {lastError && (
          <p className="sync-panel__error" role="alert">
            {t('dialogs.settings.sync.errorPrefix')}: {lastError}
          </p>
        )}
        {status?.last_error_code === 'auth' && (
          <p className="sync-panel__warning" role="status">
            {t('dialogs.settings.sync.authFailureBanner')}
          </p>
        )}
        {status?.sustained_failure && (
          <p className="sync-panel__warning" role="status">
            {t('dialogs.settings.sync.sustainedFailureBanner')}
          </p>
        )}
        <div className="sync-panel__actions">
          {(() => {
            // `aria-disabled` + a no-op handler instead of native `disabled`:
            // browsers strip focus from a disabling button, which would leave
            // NVDA in focus mode with no anchor. Keeping it focusable + busy
            // preserves the tab stop.
            const busy = !status?.configured || triggering || status?.in_flight;
            return (
              <button
                type="button"
                aria-disabled={busy}
                aria-busy={triggering || status?.in_flight}
                onClick={() => {
                  if (!busy) void triggerSync();
                }}
              >
                {status?.in_flight || triggering
                  ? t('dialogs.settings.sync.syncing')
                  : t('dialogs.settings.sync.syncNow')}
              </button>
            );
          })()}
          {(() => {
            const busy = !status?.configured || busyCompact;
            return (
              <button
                type="button"
                aria-disabled={busy}
                aria-busy={busyCompact}
                onClick={() => {
                  if (!busy) void onCompact();
                }}
              >
                {busyCompact
                  ? t('dialogs.settings.sync.compacting')
                  : t('dialogs.settings.sync.compactNow')}
              </button>
            );
          })()}
          {conflictCount > 0 && (
            <button type="button" onClick={openSyncConflicts}>
              {conflictCount === 1
                ? t('syncStatus.conflict_one')
                : t('syncStatus.conflict_other', { count: conflictCount })}
            </button>
          )}
        </div>
      </section>

      <section aria-labelledby={intervalHeadingId}>
        <h3 id={intervalHeadingId}>
          {t('dialogs.settings.sync.intervalLabel')}
        </h3>
        <select
          // The section heading names this control for a sighted user; a
          // combo box has no name from surrounding text, so NVDA announced
          // "combo box, 15 minutes" with nothing saying what the 15 minutes
          // were for.
          aria-label={t('dialogs.settings.sync.intervalLabel')}
          value={interval}
          onChange={(e) => void onIntervalChange(e.target.value)}
          disabled={!status?.configured}
        >
          {INTERVAL_PRESETS.map((min) => (
            <option key={min} value={min}>
              {t('dialogs.settings.sync.intervalOption', {
                count: min,
                minutes: min,
              })}
            </option>
          ))}
        </select>
      </section>

      <section aria-labelledby={adapterHeadingId}>
        <h3 id={adapterHeadingId} ref={adapterHeadingRef} tabIndex={-1}>
          {t('dialogs.settings.sync.targetTitle')}
        </h3>
        {/* A non-editable summary of what this device syncs through, and
            Disconnect. Rendered off the SUMMARY alone, not off the engine
            being configured: a chosen target the engine did not come up on
            still has to be disconnectable, and gating this on `configured`
            was what left that state with no control at all.

            The kind is deliberately not named here any more. It used to come
            from a six-entry table in this file, forty lines above a picker
            that names the same account's protocol from the plugin's own
            declaration — so one account was read out under two different
            names on one screen ("SFTP (SSH server)" here, "SFTP (file
            transfer over SSH)" below). The detail IS the account's display
            name, the picker states the protocol, and the table is gone. */}
        {adapterSummary && (
          <div className="sync-panel__connected-summary">
            <FocusableNote className="sync-panel__hint">
              {t(
                status?.configured
                  ? 'dialogs.settings.sync.connectedSummary'
                  : 'dialogs.settings.sync.connectedSummaryNotRunning',
                { detail: adapterSummary.detail || '–' },
              )}
            </FocusableNote>
            <div className="sync-panel__actions">
              <button
                type="button"
                // aria-disabled rather than native `disabled`: a button that
                // disables itself while the round-trip runs drops focus to
                // <body>, outside the modal's role="application", and NVDA
                // leaves the dialog without hearing the outcome.
                aria-disabled={busyDisconnect}
                aria-busy={busyDisconnect}
                onClick={() => {
                  if (!busyDisconnect) setConfirmDisconnect(true);
                }}
              >
                {t('dialogs.settings.sync.adapterDisconnect')}
              </button>
            </div>
          </div>
        )}
        {/* Which account holds the dataset. Rendered whether or not one is
            chosen: moving to another account is a swap the backend probes
            before it commits, so making the user disconnect first — and lose
            the account row on the way — was never the safer order. */}
        <SyncTargetAccountPicker
          currentAccountId={adapterSummary?.account_id ?? null}
          // The ENGINE's answer, not the stored pointer's — so the picker can
          // tell "this account holds the dataset" from "this account is
          // supposed to and nothing is syncing". `null` until the status has
          // arrived, so it accuses nothing on the first render.
          active={status ? status.configured : null}
          onChanged={onTargetChanged}
        />
      </section>

      {/* §19 device registry — what this device calls itself, and who else the
          dataset still counts. Its own section rather than a block inside the
          target one: the name belongs to this device and survives a change of
          target, and the registry is a property of the DATASET rather than of
          the account that happens to hold it. */}
      <section aria-labelledby={devicesHeadingId}>
        <h3 id={devicesHeadingId}>
          {t('dialogs.settings.sync.devicesTitle')}
        </h3>
        <SyncDevicesPanel configured={status?.configured ?? false} />
      </section>

      {/* §19.7 — cross-device adoption banner. */}
      {status?.configured &&
        !status?.e2e_enabled &&
        status?.last_error_code === 'encryption_required' && (
          <section className="sync-panel__remote-e2e-banner" role="alert">
            <h3>{t('dialogs.settings.sync.adoptRemoteE2eTitle')}</h3>
            <FocusableNote>
              {t('dialogs.settings.sync.adoptRemoteE2eHint')}
            </FocusableNote>
            <div className="sync-panel__field">
              <label>
                {t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
                <input
                  type="password"
                  value={adoptRemotePpDraft}
                  onChange={(e) => setAdoptRemotePpDraft(e.target.value)}
                  autoComplete="current-password"
                />
              </label>
            </div>
            {adoptRemoteError && (
              <p className="sync-panel__error" role="alert">
                {t('dialogs.settings.sync.errorPrefix')}: {adoptRemoteError}
              </p>
            )}
            <div className="sync-panel__actions">
              <button
                type="button"
                disabled={busyAdoptRemote}
                onClick={() => void onAdoptRemoteEncryption()}
              >
                {busyAdoptRemote
                  ? t('dialogs.settings.sync.adoptRemoteE2eRunning')
                  : t('dialogs.settings.sync.adoptRemoteE2eAction')}
              </button>
            </div>
          </section>
        )}

      {/* §19.7 — turn on encryption for an existing unencrypted dataset. */}
      {status?.configured && !status?.e2e_enabled && (
        <section aria-labelledby={enableE2eHeadingId}>
          <h3 id={enableE2eHeadingId}>
            {t('dialogs.settings.sync.enableE2eTitle')}
          </h3>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.enableE2eHint')}
          </FocusableNote>
          <FocusableNote className="sync-panel__hint sync-panel__hint--warning">
            {t('dialogs.settings.sync.enableE2eMultiDeviceWarning')}
          </FocusableNote>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.enableE2ePassphraseLabel')}
              <input
                type="password"
                value={enableNewPpDraft}
                onChange={(e) => setEnableNewPpDraft(e.target.value)}
                autoComplete="new-password"
              />
            </label>
          </div>
          {enableError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {enableError}
            </p>
          )}
          {enableOk && (
            <p className="sync-panel__hint" role="status">
              {t('dialogs.settings.sync.enableE2eOk')}
            </p>
          )}
          <div className="sync-panel__actions">
            <button
              type="button"
              disabled={busyEnable}
              onClick={() => void onEnableEncryption()}
            >
              {busyEnable
                ? t('dialogs.settings.sync.enableE2eRunning')
                : t('dialogs.settings.sync.enableE2eAction')}
            </button>
          </div>
        </section>
      )}

      {/* §19.7 — change / disable passphrase (encrypted + configured only). */}
      {status?.configured && status?.e2e_enabled && (
        <section aria-labelledby={passphraseHeadingId}>
          <h3 id={passphraseHeadingId}>
            {t('dialogs.settings.sync.passphraseChangeTitle')}
          </h3>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.passphraseChangeHint')}
          </FocusableNote>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.passphraseChangeOld')}
              <input
                type="password"
                value={oldPassphraseDraft}
                onChange={(e) => setOldPassphraseDraft(e.target.value)}
                autoComplete="current-password"
              />
            </label>
          </div>
          <div className="sync-panel__field">
            <label>
              {t('dialogs.settings.sync.passphraseChangeNew')}
              <input
                type="password"
                value={newPassphraseDraft}
                onChange={(e) => setNewPassphraseDraft(e.target.value)}
                autoComplete="new-password"
              />
            </label>
          </div>
          {passphraseChangeError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {passphraseChangeError}
            </p>
          )}
          {passphraseChangeOk && (
            <p className="sync-panel__hint" role="status">
              {t('dialogs.settings.sync.passphraseChangeOk')}
            </p>
          )}
          {disableError && (
            <p className="sync-panel__error" role="alert">
              {t('dialogs.settings.sync.errorPrefix')}: {disableError}
            </p>
          )}
          <div className="sync-panel__actions">
            <button
              type="button"
              disabled={busyPassphraseChange || busyDisable}
              onClick={() => void onChangePassphrase()}
            >
              {busyPassphraseChange
                ? t('dialogs.settings.sync.passphraseChangeRunning')
                : t('dialogs.settings.sync.passphraseChangeAction')}
            </button>
            <button
              type="button"
              disabled={busyPassphraseChange || busyDisable}
              onClick={() => void onDisableEncryption()}
            >
              {busyDisable
                ? t('dialogs.settings.sync.disableE2eRunning')
                : t('dialogs.settings.sync.disableE2eAction')}
            </button>
          </div>
          <FocusableNote className="sync-panel__hint">
            {t('dialogs.settings.sync.disableE2eHint')}
          </FocusableNote>
        </section>
      )}

      {/* §19.9 detailed Sync-Protokoll. Always rendered. */}
      <SyncProtocolSection headingId={protocolHeadingId} />

      {/* Disconnect drops the account row, its device-local half and its
          credentials — everything except the dataset's encryption key. The
          message says so, because "you can reconnect with one tap" is what
          this used to promise and it has not been true since the target became
          an account. Focus opens on Cancel. */}
      <ConfirmDialog
        isOpen={confirmDisconnect}
        onClose={() => setConfirmDisconnect(false)}
        onConfirm={() => void onDisconnect()}
        title={t('dialogs.settings.sync.adapterDisconnect')}
        message={t('dialogs.settings.sync.adapterDisconnectConfirm')}
        confirmLabel={t('dialogs.settings.sync.adapterDisconnect')}
      />
    </div>
  );
}
