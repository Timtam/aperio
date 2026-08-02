import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
  collectValues,
  firstMissingField,
  type AccountFormSpec,
} from '@aperio/shared';

import {
  accountFormSpec,
  listAdapterKinds,
  type AdapterKindInfo,
} from '../../api/accounts';
import {
  acceptRemoteDatasetValues,
  adoptLocalDatasetValues,
  previewSftpHostKey,
  previewSyncTargetValues,
  refreshExternalCache,
  trustSftpHostKey,
  type HostKeyPreview,
  type SyncPreview,
} from '../../api/sync';
import { useSyncErrorMessage } from '../../api/syncErrorMessage';
import { useThemedStyles, type ThemeColors } from '../../theme';
import { AccountSchemaForm } from '../AccountSchemaForm';
import { AppDialog } from '../AppDialog';
import { RadioGroup } from '../RadioGroup';

/** What a successful connect produced — handed to the caller so it can decide
 *  what to do next (refresh its status, advance a wizard, …). */
export interface SyncConnectOutcome {
  /** `true` when we JOINED an existing remote dataset (restore); `false` when
   *  we initialised a fresh one on an empty target (create). */
  joined: boolean;
}

/**
 * Where the dataset lives, asked once for every backend — the mobile twin of
 * the desktop `src/components/sync/SyncTargetSchemaForm.tsx`.
 *
 * The form this replaces rendered a block per kind — a WebDAV block, an SFTP
 * block, an FTP block — 1300 lines of them, and a seventh backend meant a
 * seventh. Here the fields come from the plugin's own account schema, the same
 * one the Add-account screen renders, so a new backend needs nothing here.
 *
 * ## Why this is still its own component
 *
 * Choosing a sync target is not choosing an account. It is two decisions in a
 * row: reach the target, then — depending on whether a dataset is already there
 * — join it or start a fresh one. The second question cannot be asked before
 * the first is answered, and answering the first needs a live connection to
 * something nothing has committed to yet. That is what
 * {@link previewSyncTargetValues} is for, and it is why the schema form alone is
 * not enough.
 *
 * ## Accessibility
 *
 * - The backend is a radio group, every field its own labelled stop, and the
 *   Connect button carries a different NAME while it works: on iOS
 *   `accessibilityState.busy` is not spoken, so a static label left the whole
 *   network round trip silent.
 * - A refusal is announced AND focused, both imperatively in the handler, both
 *   carrying the same sentence. Not a live region: TalkBack reads one of those
 *   on its own and would say the refusal twice, and an effect keyed on the
 *   message never re-runs when the same refusal is set twice — so a second press
 *   against the same dead server left the cursor standing where it was.
 * - An unconfirmed host key opens the trust dialog and says nothing underneath
 *   it: the dialog IS the message and takes focus onto its own title.
 */
export function SyncTargetSchemaForm({
  onConnected,
}: {
  onConnected: (outcome: SyncConnectOutcome) => void;
}) {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
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

  const errorRef = useRef<Text>(null);
  const passphraseRef = useRef<TextInput>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const focusOn = useCallback((node: Text | TextInput | null) => {
    const tag = node ? findNodeHandle(node) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  /** Say a refusal once and park focus on it so it stays re-readable. Both
   *  imperative: setting the SAME refusal twice makes React bail out of the
   *  re-render, so an effect keyed on it would never fire the second time. */
  const showError = useCallback(
    (message: string) => {
      setError(message);
      announce(message);
      requestAnimationFrame(() => focusOn(errorRef.current));
    },
    [announce, focusOn],
  );

  // Only the adapters that can hold a dataset, from the host's own capability
  // list. Never a list of names here.
  useEffect(() => {
    let cancelled = false;
    listAdapterKinds()
      .then((all) => {
        if (cancelled) return;
        const usable = all.filter((k) => k.can_sync);
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
    accountFormSpec(kind, i18n.language)
      .then((s) => {
        if (!cancelled) setSpec(s);
      })
      .catch((err) => {
        if (!cancelled) showError(messageForError(err));
      });
    return () => {
      cancelled = true;
    };
  }, [i18n.language, kind, messageForError, showError]);

  const onChange = useCallback((key: string, value: string | boolean) => {
    setValues((prev) => ({ ...prev, [key]: value }));
  }, []);

  const missing = useMemo(
    () => (spec ? firstMissingField(spec, values) : null),
    [spec, values],
  );

  const kindOptions = useMemo(
    () =>
      kinds.map((k) => ({
        value: k.kind as string,
        label: t(`dialogs.accounts.kindName.${k.kind}`, {
          defaultValue: k.name,
        }),
      })),
    [kinds, t],
  );

  const openTrustFor = useCallback(
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
      showError(
        t('dialogs.settings.sync.targetFieldRequired', {
          label: missing.label,
        }),
      );
      return;
    }
    setBusy(true);
    setError(null);
    // Say that the probe STARTED: on iOS the button's changed label is not
    // re-read for the element that already has focus, so without this the whole
    // test_connection + fetch_meta round trip passed in silence.
    announce(t('dialogs.settings.sync.adapterConnecting'));
    try {
      const sent = collectValues(spec, values);
      const p = await previewSyncTargetValues(kind, sent);
      setPreview(p);

      const device = deviceName.trim() || null;
      const pp = passphrase.trim() || null;

      if (p.kind === 'existing') {
        // An encrypted dataset cannot be joined without its passphrase, and
        // refusing here — before anything is written — keeps the failure in the
        // form the user is looking at.
        if (p.e2e_enabled && !pp) {
          setError(t('dialogs.settings.sync.e2eRemoteRequiresPassphrase'));
          announce(t('dialogs.settings.sync.e2eRemoteRequiresPassphrase'));
          requestAnimationFrame(() => focusOn(passphraseRef.current));
          return;
        }
        await acceptRemoteDatasetValues(kind, sent, device, pp);
        void refreshExternalCache().catch(() => undefined);
        announce(t('dialogs.settings.sync.onboardRestoreOk'));
        onConnected({ joined: true });
        return;
      }

      await adoptLocalDatasetValues(kind, sent, device, pp);
      announce(t('dialogs.settings.sync.onboardCreateOk'));
      onConnected({ joined: false });
    } catch (err) {
      // The one refusal whose repair is a GESTURE rather than a sentence. The
      // code crosses this boundary now (`StoreError::Sync` → a coded native
      // exception), so it is asked by CODE and not by matching prose.
      const code =
        err && typeof err === 'object' && 'code' in err
          ? (err as { code?: unknown }).code
          : null;
      if (code === 'host_key_not_trusted') {
        const hostPort = String(
          err instanceof Error ? err.message : err,
        ).match(/for (\S+) has not/)?.[1];
        if (hostPort) {
          void openTrustFor(hostPort);
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
    focusOn,
    kind,
    messageForError,
    missing,
    onConnected,
    openTrustFor,
    passphrase,
    showError,
    spec,
    t,
    values,
  ]);

  const acceptTrust = useCallback(() => {
    const pending = trust;
    setTrust(null);
    if (pending == null) return;
    void trustSftpHostKey(pending.hostPort, pending.preview.fingerprint).then(
      () => connect(),
      (err) => showError(messageForError(err)),
    );
  }, [connect, messageForError, showError, trust]);

  const cancelTrust = useCallback(() => {
    setTrust(null);
    // Declining the fingerprint means the target still cannot be reached, and
    // the dialog that said so is gone — so say it on the screen, in ONE
    // sentence carrying both halves (two racing announcements lose one).
    showError(
      `${t('dialogs.settings.sync.sftpTrustCancelled')} ${t(
        'dialogs.settings.sync.targetHostKeyUntrusted',
      )}`,
    );
  }, [showError, t]);

  const connectLabel = busy
    ? t('dialogs.settings.sync.adapterConnecting')
    : t('dialogs.settings.sync.targetConnect');

  return (
    <View style={styles.form}>
      <RadioGroup<string>
        label={t('dialogs.settings.sync.adapterKind')}
        value={kind}
        options={kindOptions}
        onChange={setKind}
      />

      {spec && (
        <AccountSchemaForm spec={spec} values={values} onChange={onChange} />
      )}

      <View style={styles.field}>
        <Text style={styles.label}>
          {t('dialogs.settings.sync.deviceName')}
        </Text>
        <TextInput
          style={styles.input}
          value={deviceName}
          onChangeText={setDeviceName}
          autoCapitalize="none"
          autoCorrect={false}
          accessibilityLabel={t('dialogs.settings.sync.deviceName')}
        />
      </View>

      <View style={styles.field}>
        <Text style={styles.label}>
          {t('dialogs.settings.sync.adoptRemoteE2ePassphraseLabel')}
        </Text>
        <TextInput
          ref={passphraseRef}
          style={styles.input}
          value={passphrase}
          onChangeText={setPassphrase}
          secureTextEntry
          autoCapitalize="none"
          autoCorrect={false}
          accessibilityLabel={t(
            'dialogs.settings.sync.adoptRemoteE2ePassphraseLabel',
          )}
          accessibilityHint={t('dialogs.settings.sync.connectEmptyReveal')}
        />
        <Text style={styles.hint}>
          {t('dialogs.settings.sync.connectEmptyReveal')}
        </Text>
      </View>

      {preview?.kind === 'existing' && (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.sync.onboardPreviewExisting')}
        </Text>
      )}

      {/* Deliberately NOT a live region: every refusal here is already
          announced by `showError`, which then lands focus on this node. */}
      {error != null && (
        <Text ref={errorRef} style={styles.error} accessibilityRole="text">
          {error}
        </Text>
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={connectLabel}
        accessibilityState={{ disabled: busy || !spec, busy }}
        disabled={busy || !spec}
        onPress={() => void connect()}
        style={({ pressed }) => [styles.primaryButton, pressed && styles.pressed]}
      >
        <Text style={styles.primaryButtonText}>{connectLabel}</Text>
      </Pressable>

      {/* §19.5 — the fingerprint decision, in the app's focus-trapping popup so
          the numbers cannot be confirmed by a stray tap on the screen behind.
          Same pin store as the account picker's, keyed by host:port, so a
          fingerprint confirmed on either path is confirmed for both. */}
      <AppDialog
        visible={trust != null}
        title={
          trust?.preview.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustChangedTitle')
            : t('dialogs.settings.sync.sftpTrustNewTitle')
        }
        message={
          trust?.preview.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustChangedBody')
            : t('dialogs.settings.sync.sftpTrustNewBody')
        }
        confirmLabel={
          trust?.preview.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustAcceptChanged')
            : t('dialogs.settings.sync.sftpTrustAcceptNew')
        }
        cancelLabel={t('dialogs.settings.sync.sftpTrustCancel')}
        destructive={trust?.preview.status.kind === 'changed'}
        onConfirm={acceptTrust}
        onCancel={cancelTrust}
      >
        <Text style={styles.trustField}>
          {t('dialogs.settings.sync.sftpTrustHostLabel')}:{' '}
          {trust?.hostPort ?? ''}
        </Text>
        {trust?.preview.status.kind === 'changed' && (
          <Text style={styles.trustField}>
            {t('dialogs.settings.sync.sftpTrustStoredLabel')}:{' '}
            {trust.preview.status.stored}
          </Text>
        )}
        <Text style={styles.trustField}>
          {t('dialogs.settings.sync.sftpTrustPresentedLabel')}:{' '}
          {trust?.preview.fingerprint ?? ''}
        </Text>
        <Text style={styles.hint}>
          {t('dialogs.settings.sync.sftpTrustVerifyHint')}
        </Text>
      </AppDialog>
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    form: { gap: 14 },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    input: {
      borderWidth: 1,
      borderColor: c.border,
      borderRadius: 10,
      paddingHorizontal: 12,
      paddingVertical: 12,
      fontSize: 16,
      color: c.textPrimary,
      backgroundColor: c.surface,
    },
    primaryButton: {
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    primaryButtonText: {
      fontSize: 16,
      fontWeight: '700',
      color: c.textOnAccent,
    },
    trustField: { fontSize: 14, color: c.textPrimary, fontFamily: 'monospace' },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
    pressed: { opacity: 0.7 },
  });
