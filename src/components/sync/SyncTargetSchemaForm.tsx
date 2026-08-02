import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { collectValues, firstMissingField } from '@aperio/shared';

import { useAnnouncer } from '../../a11y/announcerContext';
import {
  acceptRemoteDatasetValues,
  accountFormSpec,
  adoptLocalDatasetValues,
  isCommandError,
  listAdapterKinds,
  previewSftpHostKey,
  previewSyncTargetValues,
  trustSftpHostKey,
} from '../../api/client';
import type {
  AccountFormSpec,
  AdapterKindInfo,
  HostKeyPreview,
  SyncPreview,
} from '../../api/client';
import type { Account } from '../../api/types';
import { useDateFormat } from '../../intl/dateFormat';
import { fetchAccountsNeedingConnect } from '../accountsNeedingConnect';
import { AccountSchemaForm } from '../AccountSchemaForm';
import { FocusableNote } from '../../a11y/FocusableNote';
import { SyncSftpTrustDialog } from '../SyncSftpTrustDialog';
import { useSyncErrorMessage } from './syncErrorMessage';

/** The snapshot's date in the user's own format, falling back to the raw
 *  timestamp when it cannot be parsed — a malformed date is still information,
 *  and "Invalid Date" is not. */
function formatSnapshotTime(
  fmt: ReturnType<typeof useDateFormat>,
  timestamp: string,
): string {
  try {
    return fmt.format(new Date(timestamp), 'PPP');
  } catch {
    return timestamp;
  }
}

/** The devices already on the dataset, with this one marked — the difference
 *  between "somebody else's data" and "my other machine". */
function deviceNames(
  devices: readonly { id: string; name: string | null; is_this_device: boolean }[],
  t: (key: string) => string,
): string {
  return devices
    .map((d) =>
      d.is_this_device
        ? `${d.name ?? d.id} (${t('dialogs.settings.sync.previewThisDevice')})`
        : (d.name ?? d.id),
    )
    .join(', ');
}

/** What a successful connect produced — handed to the caller so it can decide
 *  what to do next (refresh its summary card, advance a wizard, prompt for
 *  missing account credentials, …). */
export interface SyncConnectOutcome {
  /** `true` when we JOINED an existing remote dataset (restore), `false` when
   *  we initialized a fresh one on an empty target (create). */
  joined: boolean;
  /** Accounts whose secrets didn't arrive with a restored dataset and need the
   *  user to re-enter credentials. Empty for a freshly-created dataset. */
  accountsNeedingConnect: Account[];
}

/**
 * Where the dataset lives, asked once for every backend.
 *
 * The form this replaces rendered a block per kind — a WebDAV block, an SFTP
 * block, an FTP block — 1539 lines of them, and a seventh backend meant a
 * seventh. Here the fields come from the plugin's own account schema, the same
 * one the Add-account dialog renders, so a new backend needs nothing here at
 * all.
 *
 * ## Why this is still its own component
 *
 * Choosing a sync target is not choosing an account. It is two decisions in a
 * row: reach the target, then — depending on whether a dataset is already
 * there — join it or start a fresh one. The second question cannot be asked
 * before the first is answered, and answering the first requires a live
 * connection to something nothing has committed to yet. That is what
 * `previewSyncTargetValues` is for, and it is why the schema form alone is not
 * enough.
 *
 * ## What the caller gets
 *
 * The same [`SyncConnectOutcome`] the older form produced, so a wizard swapping
 * one for the other changes an import and nothing else.
 */
export function SyncTargetSchemaForm({
  onConnected,
}: {
  onConnected: (outcome: SyncConnectOutcome) => void;
}) {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const announce = useAnnouncer();
  const messageForError = useSyncErrorMessage();

  const [kinds, setKinds] = useState<AdapterKindInfo[]>([]);
  const [kind, setKind] = useState('');
  const [spec, setSpec] = useState<AccountFormSpec | null>(null);
  const [values, setValues] = useState<Record<string, string | boolean>>({});
  const [deviceName, setDeviceName] = useState('');
  const [passphrase, setPassphrase] = useState('');
  const [preview, setPreview] = useState<SyncPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [trust, setTrust] = useState<{
    hostPort: string;
    preview: HostKeyPreview;
  } | null>(null);

  const errorRef = useRef<HTMLParagraphElement>(null);
  const passphraseRef = useRef<HTMLInputElement>(null);

  /** Say a refusal once, by landing on it — the node carries the sentence as
   *  its accessible name, so announcing it as well would speak it twice.
   *  Imperative rather than an effect: the same message twice in a row is a
   *  no-op re-render, and an effect would not fire the second time. */
  const showError = useCallback((message: string) => {
    setError(message);
    requestAnimationFrame(() => {
      errorRef.current?.focus({ preventScroll: true });
    });
  }, []);

  // Only the adapters that can hold a dataset, from the host's own capability
  // list. Never a list of names here.
  useEffect(() => {
    let cancelled = false;
    listAdapterKinds()
      .then((all) => {
        if (cancelled) return;
        // `can_sync`, and then either creatable or already there. A kind its
        // plugin only adopted is neither — no adapter left to make one with —
        // and its existing accounts are chosen on the sync screen instead,
        // which is a different question and a different list. The built-in
        // store IS `implicit`: picking it needs no account created, because the
        // account is the one every device already has.
        const usable = all.filter((k) => k.can_sync && (k.offered || k.implicit));
        setKinds(usable);
        setKind((current) =>
          usable.some((k) => k.kind === current)
            ? current
            : (usable[0]?.kind ?? ''),
        );
      })
      .catch((err) => showError(messageForError(err)));
    return () => {
      cancelled = true;
    };
  }, [messageForError, showError]);

  // The chosen backend's fields. Values reset with the kind: a URL is not a
  // host, and carrying one across would leave the form describing a target
  // nobody asked for.
  useEffect(() => {
    if (!kind) return;
    let cancelled = false;
    setSpec(null);
    setValues({});
    setPreview(null);
    accountFormSpec(kind)
      .then((s) => {
        if (!cancelled) setSpec(s);
      })
      .catch((err) => {
        if (!cancelled) showError(messageForError(err));
      });
    return () => {
      cancelled = true;
    };
  }, [kind, messageForError, showError]);

  const onChange = useCallback((key: string, value: string | boolean) => {
    setValues((prev) => ({ ...prev, [key]: value }));
  }, []);

  const missing = useMemo(
    () => (spec ? firstMissingField(spec, values) : null),
    [spec, values],
  );

  const onCheckHostKey = useCallback(
    async (hostPort: string) => {
      const [host, port] = hostPort.split(':');
      setError(null);
      try {
        const p = await previewSftpHostKey(host, Number(port) || 22);
        setTrust({ hostPort, preview: p });
      } catch (err) {
        showError(messageForError(err));
      }
    },
    [messageForError, showError],
  );

  const connect = useCallback(async () => {
    if (!spec || !kind || busy) return;
    if (missing) {
      showError(t('dialogs.settings.sync.targetFieldRequired', {
        label: missing.label,
      }));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const sent = collectValues(spec, values);
      const p = await previewSyncTargetValues(kind, sent);
      setPreview(p);

      const device = deviceName.trim() || null;
      const pp = passphrase.trim() || null;

      if (p.kind === 'existing') {
        // An encrypted dataset cannot be joined without its passphrase, and
        // refusing here — before anything is written — keeps the failure in
        // the form the user is looking at.
        if (p.e2e_enabled && !pp) {
          showError(t('dialogs.settings.sync.e2eRemoteRequiresPassphrase'));
          requestAnimationFrame(() => {
            passphraseRef.current?.focus({ preventScroll: true });
          });
          return;
        }
        await acceptRemoteDatasetValues(kind, sent, device, pp);
        const needing = (await fetchAccountsNeedingConnect()) ?? [];
        announce(t('dialogs.settings.sync.onboardRestoreOk'), 'assertive');
        onConnected({ joined: true, accountsNeedingConnect: needing });
        return;
      }

      await adoptLocalDatasetValues(kind, sent, device, pp);
      announce(t('dialogs.settings.sync.onboardCreateOk'), 'assertive');
      onConnected({ joined: false, accountsNeedingConnect: [] });
    } catch (err) {
      if (isCommandError(err) && err.code === 'host_key_not_trusted') {
        // Not a dead end and not a message: the fingerprint has to be looked
        // at and accepted, so the gesture is offered right here.
        const hostPort = String(err.message).match(/for (\S+) has not/)?.[1];
        if (hostPort) {
          void onCheckHostKey(hostPort);
          return;
        }
      }
      showError(messageForError(err));
    } finally {
      setBusy(false);
    }
  }, [
    announce,
    busy,
    deviceName,
    kind,
    messageForError,
    missing,
    onCheckHostKey,
    onConnected,
    passphrase,
    showError,
    spec,
    t,
    values,
  ]);

  return (
    <div className="sync-target-form">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.sync.adapterKind')}
        </span>
        <select
          value={kind}
          onChange={(e) => setKind(e.target.value)}
          disabled={kinds.length === 0}
        >
          {kinds.map((k) => (
            <option key={k.kind} value={k.kind}>
              {t(`dialogs.accounts.kindName.${k.kind}`, { defaultValue: k.name })}
            </option>
          ))}
        </select>
      </label>

      {spec && (
        <AccountSchemaForm spec={spec} values={values} onChange={onChange} />
      )}

      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.sync.deviceName')}
        </span>
        <input
          value={deviceName}
          onChange={(e) => setDeviceName(e.target.value)}
          autoComplete="off"
        />
      </label>

      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
        </span>
        <input
          ref={passphraseRef}
          type="password"
          value={passphrase}
          onChange={(e) => setPassphrase(e.target.value)}
          autoComplete="current-password"
        />
        <span className="form__hint">
          {t('dialogs.settings.sync.connectEmptyReveal')}
        </span>
      </label>

      {preview?.kind === 'existing' && (
        <div className="sync-panel__preview">
          <FocusableNote className="form__hint">
            {t('dialogs.settings.sync.onboardPreviewExisting')}
          </FocusableNote>
          {/* What is actually over there: when it was last compacted, and which
              devices are already on it. A dataset the user does not recognise is
              the one thing that should stop them joining, and "a dataset is
              already there" does not let them tell. */}
          <FocusableNote className="form__hint">
            {preview.snapshot_timestamp !== null
              ? t('dialogs.settings.sync.previewExisting', {
                  time: formatSnapshotTime(fmt, preview.snapshot_timestamp),
                })
              : t('dialogs.settings.sync.previewNeverCompacted')}
          </FocusableNote>
          <FocusableNote className="form__hint">
            {t('dialogs.settings.sync.previewDevices', {
              count: preview.devices.length,
              names: deviceNames(preview.devices, (key) => t(key)),
            })}
          </FocusableNote>
        </div>
      )}

      {error && (
        <FocusableNote
          ref={errorRef}
          className="sync-panel__error form__error"
        >
          {error}
        </FocusableNote>
      )}

      <div className="form__actions">
        <button
          type="button"
          aria-disabled={busy || !spec}
          aria-busy={busy}
          onClick={() => {
            if (!busy && spec) void connect();
          }}
        >
          {busy
            ? t('dialogs.settings.sync.adapterConnecting')
            : t('dialogs.settings.sync.targetConnect')}
        </button>
      </div>

      {trust && (
        <SyncSftpTrustDialog
          isOpen
          preview={trust.preview}
          onAccept={(fingerprint) => {
            const hostPort = trust.hostPort;
            setTrust(null);
            void trustSftpHostKey(hostPort, fingerprint).then(
              () => connect(),
              (err) => showError(messageForError(err)),
            );
          }}
          onCancel={() => {
            setTrust(null);
            // Restate the reason: the dialog closing is not an answer, and
            // leaving the form silent would look like the attempt succeeded.
            showError(t('dialogs.settings.sync.targetHostKeyUntrusted'));
          }}
        />
      )}
    </div>
  );
}
