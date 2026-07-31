import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  Alert,
  findNodeHandle,
  PermissionsAndroid,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { useThemedStyles, type ThemeColors } from '../theme';
import {
  Account,
  AdapterKind,
  AdapterKindInfo,
  accountFormSpec,
  createAccount,
  listAdapterKinds,
  deleteAccount,
  listAccounts,
  listAccountsMissingCredentials,
  renameAccount,
  requestDeviceCalendarAccess,
  resetAccountSync,
  runAccountAction,
  setAccountSecret,
  testAccountValues,
} from '../api/accounts';
import type { AccountFormAction } from '@aperio/shared';
import { getUserPref, setUserPref } from '../api/prefs';
import {
  connectSchemaAccount,
  reconnectOAuthAccount,
} from '../api/oauth';
import { refreshExternalCache } from '../api/sync';
import { useRefreshErrors } from '../state/useRefreshErrors';
import {
  collectValues,
  firstMissingField,
  type AccountFormSpec,
} from '@aperio/shared';

import { AccountSchemaForm } from '../components/AccountSchemaForm';

/// Not an adapter's action — the probe every schema-declaring adapter gets, so
/// it shares the one busy slot with the declared ones.
const TEST_ACTION_KEY = '__test__';
import { AppDialog } from '../components/AppDialog';
import ContactsPrivacyNoticeModal from '../components/ContactsPrivacyNoticeModal';
import { FormScrollView } from '../components/FormScrollView';

// Accounts management — list + add (credential kinds) + connect (OAuth kinds via
// the browser sign-in flow) + delete, over the Rust Host (statically-embedded
// adapter plugins + the keychain-bridged SecretStore). The non-OAuth kinds use
// the form the adapter itself declares; Google / Microsoft use the host-driven
// native auth session (see OAuthConnectForm).
//
// Accessibility: every control is an addressable element with an explicit
// label; the kind picker is a RadioGroup; deletes are reachable both as a
// visible button and a custom accessibility action; results are announced and
// screen-reader focus is moved to the new row after a create/connect.

/** Kinds the host implements itself, so no plugin declares them. `local` is the
 *  built-in store (created at bootstrap, never offered); `device_calendar` is
 *  offered separately because it is added through an OS permission grant rather
 *  than a form. */
const HOST_INTERNAL_KINDS: ReadonlySet<AdapterKind> = new Set([
  'local',
  'device_calendar',
]);

/** The device-local calendar adapter ships on both phone platforms — iOS
 *  (EventKit: calendars + reminders) and Android (CalendarProvider: calendars
 *  only; no system reminders app). Gates the extra "This device" picker entry,
 *  which adds the account through an OS permission grant rather than a
 *  credential form. */
const DEVICE_KIND_AVAILABLE =
  Platform.OS === 'ios' || Platform.OS === 'android';

/** Request Android's calendar read+write runtime permissions. iOS routes its
 *  grant through the native EventKit prompt instead (requestDeviceCalendarAccess). */
async function requestAndroidCalendarPermission(): Promise<boolean> {
  const result = await PermissionsAndroid.requestMultiple([
    PermissionsAndroid.PERMISSIONS.READ_CALENDAR,
    PermissionsAndroid.PERMISSIONS.WRITE_CALENDAR,
  ]);
  return (
    result[PermissionsAndroid.PERMISSIONS.READ_CALENDAR] ===
      PermissionsAndroid.RESULTS.GRANTED &&
    result[PermissionsAndroid.PERMISSIONS.WRITE_CALENDAR] ===
      PermissionsAndroid.RESULTS.GRANTED
  );
}

/** Adapter kinds whose ContactsFeature pulls remote address-book data — the
 *  ones that trigger the one-shot privacy notice on first connect. Mirrors the
 *  desktop CONTACTS_CAPABLE_KINDS. */
const CONTACTS_CAPABLE_KINDS: ReadonlySet<AdapterKind> = new Set<AdapterKind>([
  'google',
  'microsoft_graph',
  'ews',
  'caldav',
]);

/** Frontend-only flag: once acknowledged, the privacy notice never re-appears. */
const PREF_PRIVACY_NOTICE_ACK = 'contacts.privacyNoticeAcknowledged';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function AccountsScreen() {
  const { t, i18n } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // Per-account refresh errors: an auth-suspected account gets the same
  // Reconnect affordance as a missing-credential one (a present-but-wrong
  // password is invisible to the keychain probe).
  const { errorsByAccount } = useRefreshErrors();

  const [accounts, setAccounts] = useState<Account[]>([]);
  // Ids of external accounts whose required keychain secret is absent — the
  // credential-repair set (a token expired, or the row synced from another
  // device without its device-local secret).
  const [missingIds, setMissingIds] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Inline account rename (id being edited + its draft name).
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState('');
  // Inline credential repair (id being repaired + the new secret draft).
  const [repairId, setRepairId] = useState<string | null>(null);
  const [repairSecret, setRepairSecret] = useState('');
  // The one-shot contacts privacy notice: the kind of the just-connected
  // contacts-capable account (null = closed). Frontend-only gating.
  const [privacyNoticeFor, setPrivacyNoticeFor] = useState<AdapterKind | null>(null);

  // Add-form state.
  const [displayName, setDisplayName] = useState('');
  const [submitting, setSubmitting] = useState(false);
  // Which button is running, so exactly one can be busy and the busy one can
  // rename itself. `null` when nothing is.
  const [busyAction, setBusyAction] = useState<string | null>(null);
  const [testMessage, setTestMessage] = useState<string | null>(null);
  // Add flow: 'list' shows the connected accounts + an "Add account" button;
  // 'picker' a provider menu; 'credential'/'oauth' the chosen provider's form —
  // replacing the old always-mounted credential + OAuth forms (one long view).
  const [mode, setMode] = useState<
    'list' | 'picker' | 'credential' | 'device' | 'schema'
  >('list');
  // The connect form for a schema-declaring adapter, as that ADAPTER declares
  // it — plus the values collected for it. Nothing in this screen knows what
  // any of the fields mean.
  const [formSpec, setFormSpec] = useState<AccountFormSpec | null>(null);
  const [formValues, setFormValues] = useState<
    Record<string, string | boolean>
  >({});
  const [schemaKind, setSchemaKind] = useState<AdapterKind | null>(null);
  // Which adapters this build can connect, straight from the host — the picker
  // does not carry the list, because installed plugins decide it.
  const [availableKinds, setAvailableKinds] = useState<AdapterKindInfo[]>([]);

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  // After connecting a contacts-capable account, show the one-shot privacy
  // notice unless it's already been acknowledged (frontend-only pref). Mirrors
  // the desktop AccountsPanel gate.
  const maybeShowPrivacyNotice = useCallback(async (connectedKind: AdapterKind) => {
    if (!CONTACTS_CAPABLE_KINDS.has(connectedKind)) return;
    try {
      const ack = await getUserPref(PREF_PRIVACY_NOTICE_ACK);
      if (ack !== 'true') setPrivacyNoticeFor(connectedKind);
    } catch {
      // Pref read failed — err on showing the notice (privacy-forward).
      setPrivacyNoticeFor(connectedKind);
    }
  }, []);

  const acknowledgePrivacyNotice = useCallback(() => {
    void setUserPref(PREF_PRIVACY_NOTICE_ACK, 'true').catch(() => {});
    setPrivacyNoticeFor(null);
  }, []);

  const load = useCallback(async () => {
    try {
      const [accs, missing] = await Promise.all([
        listAccounts(),
        listAccountsMissingCredentials(),
      ]);
      setAccounts(accs);
      setMissingIds(new Set(missing.map((a) => a.id)));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setLoading(false);
    }
  }, [announce, t]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    let cancelled = false;
    listAdapterKinds()
      .then((kinds) => {
        if (!cancelled) {
          setAvailableKinds(kinds.filter((k) => !HOST_INTERNAL_KINDS.has(k.kind)));
        }
      })
      .catch(() => {
        // A failed probe leaves the picker with only the device entry, which is
        // honest: the screen genuinely does not know what else is available.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Move screen-reader focus to the newly created row once the list re-renders.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [accounts]);

  const resetForm = useCallback(() => {
    setDisplayName('');
    setFormValues({});
  }, []);

  // Picker → the chosen provider's form: OAuth kinds open the browser-sign-in
  // form locked to that provider; the rest open the credential form.
  const onPickProvider = useCallback(
    (picked: AdapterKind) => {
      setError(null);
      if (picked === 'device_calendar') {
        // No credentials — the OS permission prompt IS the auth step.
        setMode('device');
        return;
      }
      // Does this adapter declare its own connect form? If so it is rendered
      // from the declaration and connected through the generic path; if not it
      // falls back to the older per-kind table above. Asking the host is what
      // keeps this screen free of per-adapter knowledge.
      void accountFormSpec(picked, i18n.language)
        .then((spec) => {
          if (spec) {
            setSchemaKind(picked);
            setFormSpec(spec);
            setFormValues({});
            setMode('schema');
          } else {
            // No declaration means no plugin serves this kind, so there is
            // nothing to connect to. Say so rather than opening a form that
            // would fail on submit.
            setError(t('dialogs.accounts.kindUnavailable'));
          }
        })
        .catch((err) => {
          setError(errorMessage(err));
        });
    },
    [i18n.language, t],
  );

  /**
   * Run an action the adapter declared, and merge what it answers back into
   * the form. The host checks the requirements too — this copy only saves a
   * round trip and lets the message name the field the user is standing in.
   */
  const runDeclaredAction = useCallback(
    async (action: AccountFormAction) => {
      if (!schemaKind) return;
      setError(null);
      setTestMessage(null);
      for (const requirement of action.requires) {
        const value = formValues[requirement.field];
        if (typeof value !== 'string' || !value.trim()) {
          setError(requirement.message);
          announce(requirement.message);
          return;
        }
      }
      setBusyAction(action.key);
      try {
        const filled = await runAccountAction(
          schemaKind,
          action.key,
          formValues,
        );
        setFormValues((prev) => ({ ...prev, ...filled }));
        if (action.success) {
          setTestMessage(action.success);
          announce(action.success);
        }
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      } finally {
        setBusyAction(null);
      }
    },
    [announce, formValues, schemaKind, t],
  );

  /**
   * Probe the entered credentials without saving. The host splits these values
   * with the same schema the connect call uses, so a test and a connect cannot
   * disagree about what a field means.
   */
  const testSchemaConnection = useCallback(async () => {
    if (!schemaKind) return;
    setError(null);
    setTestMessage(null);
    setBusyAction(TEST_ACTION_KEY);
    try {
      await testAccountValues(schemaKind, formValues);
      const ok = t('dialogs.accounts.testWorks');
      setTestMessage(ok);
      announce(ok);
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setBusyAction(null);
    }
  }, [announce, formValues, schemaKind, t]);

  /** Connect an adapter that declared its own form. */
  const addFromSchema = useCallback(async () => {
    const name = displayName.trim();
    if (name.length === 0) {
      setError(t('dialogs.accounts.nameRequired'));
      announce(t('dialogs.accounts.nameRequired'));
      return;
    }
    if (!formSpec || !schemaKind) return;
    const missing = firstMissingField(formSpec, formValues);
    if (missing) {
      // Already resolved by the host, in the plugin's own words.
      const message = t('dialogs.accounts.fieldRequired', {
        field: missing.label,
      });
      setError(message);
      announce(message);
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      // A browser sign-in hands the screen to a native auth session, which is a
      // silent hand-off for a screen reader unless we say so.
      if (formSpec.oauth != null) announce(t('mobile.oauthConnecting'));
      const result = await connectSchemaAccount({
        kind: schemaKind,
        displayName: name,
        values: collectValues(formSpec, formValues),
        hasOauth: formSpec.oauth != null,
      });
      if (result.kind === 'cancelled') {
        // Dismissing the session leaves RN screen-reader focus undefined, so
        // say what happened rather than returning into silence.
        announce(t('mobile.oauthCancelled'));
        return;
      }
      resetForm();
      setFormValues({});
      setMode('list');
      await load();
      pendingFocusId.current = result.account.id;
      announce(t('dialogs.accounts.created', { name }));
      void refreshExternalCache().catch(() => undefined);
      await maybeShowPrivacyNotice(schemaKind);
    } catch (err) {
      // OAUTH_NO_CODE is a typed sentinel, not a sentence — the deleted form
      // used to map it and nothing did afterwards, so the user was shown the
      // token itself.
      const raw = errorMessage(err);
      const message =
        raw === 'OAUTH_NO_CODE' ? t('mobile.oauthNoCode') : raw;
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setSubmitting(false);
    }
  }, [
    announce,
    displayName,
    formSpec,
    formValues,
    load,
    maybeShowPrivacyNotice,
    resetForm,
    schemaKind,
    t,
  ]);

  const cancelAdd = useCallback(() => {
    resetForm();
    setError(null);
    setFormSpec(null);
    setFormValues({});
    setSchemaKind(null);
    setMode('list');
  }, [resetForm]);


  // Add the device-local calendar + reminders account: run the OS permission
  // prompt (the adapter's "auth"), then create the row on a grant. No name field
  // — it gets a fixed localized name; the account is device-local (never synced).
  const addDevice = useCallback(async () => {
    setError(null);
    setSubmitting(true);
    try {
      // iOS routes the grant through the native EventKit prompt (calendars +
      // reminders); Android requests the CalendarProvider runtime permissions.
      const granted =
        Platform.OS === 'android'
          ? await requestAndroidCalendarPermission()
          : await requestDeviceCalendarAccess(true, true);
      if (!granted) {
        const message = t('dialogs.accounts.deviceAccessDenied');
        setError(message);
        announce(message);
        return;
      }
      const created = await createAccount({
        adapter_kind: 'device_calendar',
        display_name: t('dialogs.accounts.deviceDefaultName'),
        config_json: '{}',
      });
      setMode('list');
      await load();
      pendingFocusId.current = created.id;
      announce(t('dialogs.accounts.created', { name: created.display_name }));
      // Warm the device calendar's events into the cache now (see `add`).
      void refreshExternalCache().catch(() => undefined);
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setSubmitting(false);
    }
  }, [announce, load, t]);

  const remove = useCallback(
    (account: Account) => {
      // Deleting an account drops its device-local credentials + its synced
      // containers from view — confirm first (destructive, irreversible bar a
      // re-add). The swipe action + the row's Delete button both route here.
      Alert.alert(
        t('dialogs.accounts.deleteConfirmTitle', { name: account.display_name }),
        t('dialogs.accounts.deleteConfirmMessage'),
        [
          { text: t('mobile.cancel'), style: 'cancel' },
          {
            text: t('dialogs.accounts.delete'),
            style: 'destructive',
            onPress: () => {
              void (async () => {
                setError(null);
                try {
                  await deleteAccount(account.id);
                  await load();
                  announce(t('mobile.deleted', { title: account.display_name }));
                } catch (err) {
                  const message = errorMessage(err);
                  setError(message);
                  announce(t('mobile.error', { message }));
                }
              })();
            },
          },
        ],
      );
    },
    [announce, load, t],
  );

  const startRename = useCallback((account: Account) => {
    setEditingId(account.id);
    setEditName(account.display_name);
  }, []);

  const cancelRename = useCallback(() => {
    setEditingId(null);
    setEditName('');
  }, []);

  const saveRename = useCallback(
    async (account: Account) => {
      const name = editName.trim();
      if (name.length === 0) {
        setError(t('dialogs.accounts.nameRequired'));
        announce(t('dialogs.accounts.nameRequired'));
        return;
      }
      setError(null);
      try {
        await renameAccount(account.id, name);
        setEditingId(null);
        pendingFocusId.current = account.id;
        await load();
        announce(t('dialogs.accounts.renamed', { name }));
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, editName, load, t],
  );

  const startRepair = useCallback((account: Account) => {
    setRepairId(account.id);
    setRepairSecret('');
  }, []);

  const cancelRepair = useCallback(() => {
    setRepairId(null);
    setRepairSecret('');
  }, []);

  // Re-run the provider sign-in for an OAuth account whose token expired — fresh
  // tokens land under the existing account id (its calendars / overrides stay).
  // A browser dismiss / declined consent is silent; a real failure surfaces.
  const reconnectOauth = useCallback(
    async (account: Account) => {
      setError(null);
      try {
        announce(t('mobile.oauthConnecting'));
        const result = await reconnectOAuthAccount(account);
        if (result.kind === 'cancelled') {
          announce(t('mobile.oauthCancelled'));
          return;
        }
        pendingFocusId.current = account.id;
        await load();
        announce(
          t('dialogs.accounts.credentialUpdated', { name: account.display_name }),
        );
        // Retry the failing containers right away — storing the grant alone
        // never contacts the provider, so the refresh-error warning would
        // otherwise outlive the repair until the next scheduled pass.
        void refreshExternalCache().catch(() => undefined);
      } catch (err) {
        const raw = errorMessage(err);
        const message =
          raw === 'OAUTH_NO_CODE' ? t('mobile.oauthNoCode') : raw;
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, load, t],
  );

  // Force a full cold re-sync of one external account: clears its delta tokens +
  // cached window, then kicks a warm pass so each container re-bootstraps from
  // the provider. The recovery path for a "stuck" external cache (a bootstrap
  // that cached an incomplete set as complete → events that exist on the device
  // never show here). Credentials are untouched — no re-auth.
  const resyncAccount = useCallback(
    async (account: Account) => {
      setError(null);
      try {
        await resetAccountSync(account.id);
        announce(
          t('dialogs.accounts.resyncStarted', { name: account.display_name }),
        );
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, t],
  );

  const saveRepair = useCallback(
    async (account: Account) => {
      const value = repairSecret.trim();
      if (value.length === 0) {
        setError(t('dialogs.accounts.newCredentialLabel'));
        announce(t('dialogs.accounts.newCredentialLabel'));
        return;
      }
      setError(null);
      try {
        await setAccountSecret(account.id, value);
        setRepairId(null);
        setRepairSecret('');
        pendingFocusId.current = account.id;
        await load();
        announce(
          t('dialogs.accounts.credentialUpdated', { name: account.display_name }),
        );
        // Same rationale as reconnectOauth: retry now so the error surface
        // reflects the repair (or a still-wrong password) promptly.
        void refreshExternalCache().catch(() => undefined);
      } catch (err) {
        // The secret stays in place on a registration failure — let the user
        // retry without re-typing (the field is still open).
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, load, repairSecret, t],
  );

  return (
    <>
    <FormScrollView style={styles.screen} contentContainerStyle={styles.content}>
      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

      {/* Existing accounts — hidden while the add flow is open so VoiceOver
          can't swipe into the account rows behind the add form (issue #6). */}
      {mode === 'list' && (
        <>
          <Text style={styles.heading} accessibilityRole="header">
            {t('dialogs.accounts.existingHeading')}
          </Text>
          {loading ? (
        <Text style={styles.muted} accessibilityLabel={t('dialogs.accounts.loading')}>
          {t('dialogs.accounts.loading')}
        </Text>
      ) : accounts.length === 0 ? (
        <Text style={styles.muted}>{t('dialogs.accounts.empty')}</Text>
      ) : (
        <View accessibilityRole="list" style={styles.list}>
          {accounts.map((account) => {
            const isLocal = account.adapter_kind === 'local';
            const kindName = t(`dialogs.accounts.kindName.${account.adapter_kind}`);
            const missing = missingIds.has(account.id);
            // Whether a repair is a paste or a sign-in is the ADAPTER's
            // statement, carried on the kind listing. Unknown kinds (a plugin
            // that was disabled since the account was made) fall to the paste
            // path, which fails visibly rather than opening a browser at
            // nothing.
            const oauth =
              availableKinds.find((k) => k.kind === account.adapter_kind)
                ?.declares_oauth === true;
            // A present-but-WRONG credential (revoked app password, expired
            // OAuth grant) never lands in `missing` — the refresh-error
            // surface flags it, and the same Reconnect affordance is the fix.
            const authSuspected =
              errorsByAccount.get(account.id)?.auth_suspected === true;
            const needsReconnect = missing || authSuspected;
            // Fold the credential state into the row's single SR label; a
            // "Reconnect" affordance follows for both kinds (OAuth re-runs the
            // provider sign-in; others reveal the inline secret field).
            const rowLabel = missing
              ? `${account.display_name}, ${kindName}, ${t('dialogs.accounts.missingBadge')}`
              : authSuspected
                ? `${account.display_name}, ${kindName}, ${t('dialogs.accounts.refreshErrors.badge')}`
                : `${account.display_name}, ${kindName}`;
            if (repairId === account.id) {
              return (
                <View key={account.id} style={styles.row}>
                  <TextInput
                    style={styles.editInput}
                    value={repairSecret}
                    onChangeText={setRepairSecret}
                    accessibilityLabel={t('dialogs.accounts.newCredentialLabel')}
                    secureTextEntry
                    autoCapitalize="none"
                    autoCorrect={false}
                    autoFocus
                    returnKeyType="done"
                    onSubmitEditing={() => void saveRepair(account)}
                  />
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('mobile.save')}
                    onPress={() => void saveRepair(account)}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>{t('mobile.save')}</Text>
                  </Pressable>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('mobile.cancel')}
                    onPress={cancelRepair}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>{t('mobile.cancel')}</Text>
                  </Pressable>
                </View>
              );
            }
            if (editingId === account.id) {
              return (
                <View key={account.id} style={styles.row}>
                  <TextInput
                    style={styles.editInput}
                    value={editName}
                    onChangeText={setEditName}
                    accessibilityLabel={t('dialogs.accounts.renameLabel', {
                      name: account.display_name,
                    })}
                    autoFocus
                    returnKeyType="done"
                    onSubmitEditing={() => void saveRename(account)}
                  />
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('mobile.save')}
                    onPress={() => void saveRename(account)}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>{t('mobile.save')}</Text>
                  </Pressable>
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={t('mobile.cancel')}
                    onPress={cancelRename}
                    style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.smallButtonText}>{t('mobile.cancel')}</Text>
                  </Pressable>
                </View>
              );
            }
            return (
              <View
                key={account.id}
                ref={(node) => {
                  rowTags.current[account.id] = node ? findNodeHandle(node) : null;
                }}
                accessible
                accessibilityRole="text"
                accessibilityLabel={rowLabel}
                accessibilityActions={
                  isLocal
                    ? undefined
                    : [
                        ...(needsReconnect
                          ? [{ name: 'reconnect', label: t('dialogs.accounts.reconnect') }]
                          : []),
                        { name: 'rename', label: t('mobile.rename') },
                        { name: 'resync', label: t('dialogs.accounts.forceResyncShort') },
                        { name: 'delete', label: t('dialogs.accounts.delete') },
                      ]
                }
                onAccessibilityAction={(e) => {
                  if (e.nativeEvent.actionName === 'delete') void remove(account);
                  else if (e.nativeEvent.actionName === 'rename') startRename(account);
                  else if (e.nativeEvent.actionName === 'resync') void resyncAccount(account);
                  else if (e.nativeEvent.actionName === 'reconnect') {
                    // OAuth re-runs the provider sign-in; others reveal the
                    // inline credential field.
                    if (oauth) void reconnectOauth(account);
                    else startRepair(account);
                  }
                }}
                style={styles.row}
              >
                <View style={styles.rowText}>
                  <Text style={styles.accountName}>{account.display_name}</Text>
                  <Text style={styles.accountKind}>{kindName}</Text>
                  {missing ? (
                    <Text style={styles.badge} importantForAccessibility="no">
                      {t('dialogs.accounts.missingBadge')}
                    </Text>
                  ) : authSuspected ? (
                    <Text style={styles.badge} importantForAccessibility="no">
                      {t('dialogs.accounts.refreshErrors.badge')}
                    </Text>
                  ) : null}
                </View>
                {!isLocal && (
                  <>
                    {needsReconnect && (
                      <Pressable
                        accessibilityRole="button"
                        accessibilityLabel={`${t('dialogs.accounts.reconnect')}: ${account.display_name}`}
                        onPress={() =>
                          oauth ? void reconnectOauth(account) : startRepair(account)
                        }
                        style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                      >
                        <Text style={styles.smallButtonText}>
                          {t('dialogs.accounts.reconnect')}
                        </Text>
                      </Pressable>
                    )}
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={t('dialogs.accounts.renameLabel', {
                        name: account.display_name,
                      })}
                      onPress={() => startRename(account)}
                      style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.smallButtonText}>{t('mobile.rename')}</Text>
                    </Pressable>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={t('dialogs.accounts.forceResync', {
                        name: account.display_name,
                      })}
                      onPress={() => void resyncAccount(account)}
                      style={({ pressed }) => [styles.smallButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.smallButtonText}>
                        {t('dialogs.accounts.forceResyncShort')}
                      </Text>
                    </Pressable>
                    <Pressable
                      accessibilityRole="button"
                      accessibilityLabel={`${t('dialogs.accounts.delete')}: ${account.display_name}`}
                      onPress={() => void remove(account)}
                      style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
                    >
                      <Text style={styles.deleteButtonText}>{t('dialogs.accounts.delete')}</Text>
                    </Pressable>
                  </>
                )}
              </View>
            );
          })}
        </View>
      )}
        </>
      )}

      {/* Add flow — "Add account" → a provider picker → the chosen provider's
          form. Only one stage renders at a time; the old screen mounted both
          the credential and OAuth add-forms inline, making it very long. */}
      {mode === 'list' && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('dialogs.accounts.addHeading')}
          onPress={() => setMode('picker')}
          style={({ pressed }) => [
            styles.addButton,
            styles.addHeading,
            pressed && styles.addButtonPressed,
          ]}
        >
          <Text style={styles.addButtonText}>
            {t('dialogs.accounts.addHeading')}
          </Text>
        </Pressable>
      )}

      <AppDialog
        visible={mode === 'picker'}
        title={t('dialogs.accounts.addHeading')}
        cancelLabel={t('mobile.cancel')}
        onCancel={cancelAdd}
      >
        {/* Whatever the host reported. A bundled adapter gets its translated
            name; anything else falls back to the plugin's own, which beats a
            missing-key marker. */}
        {availableKinds.map((entry) => {
          const label = t(`dialogs.accounts.kindName.${entry.kind}`, {
            defaultValue: entry.name,
          });
          return (
            <Pressable
              key={entry.kind}
              accessibilityRole="button"
              accessibilityLabel={label}
              onPress={() => onPickProvider(entry.kind)}
              style={({ pressed }) => [
                styles.secondaryButton,
                pressed && styles.pressed,
              ]}
            >
              <Text style={styles.secondaryButtonText}>{label}</Text>
            </Pressable>
          );
        })}
        {DEVICE_KIND_AVAILABLE && (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.accounts.kindName.device_calendar')}
            onPress={() => onPickProvider('device_calendar')}
            style={({ pressed }) => [
              styles.secondaryButton,
              pressed && styles.pressed,
            ]}
          >
            <Text style={styles.secondaryButtonText}>
              {t('dialogs.accounts.kindName.device_calendar')}
            </Text>
          </Pressable>
        )}
      </AppDialog>

      {/* An adapter that declares its own connect form renders it straight
          from the declaration — no branch here, and none needed when the next
          adapter arrives. */}
      <AppDialog
        visible={mode === 'schema' && formSpec != null}
        title={
          schemaKind
            ? t(`dialogs.accounts.kindName.${schemaKind}`, {
                defaultValue: schemaKind,
              })
            : ''
        }
        confirmLabel={t('dialogs.accounts.add')}
        cancelLabel={t('mobile.cancel')}
        onConfirm={() => void addFromSchema()}
        onCancel={cancelAdd}
        busy={submitting}
      >
        <View style={styles.field}>
          <Text style={styles.label}>{t('dialogs.accounts.nameLabel')}</Text>
          <TextInput
            style={styles.input}
            value={displayName}
            onChangeText={setDisplayName}
            placeholder={t('dialogs.accounts.namePlaceholder')}
            accessibilityLabel={t('dialogs.accounts.nameLabel')}
          />
        </View>
        {formSpec && (
          <AccountSchemaForm
            spec={formSpec}
            values={formValues}
            onChange={(key, value) =>
              setFormValues((prev) => ({ ...prev, [key]: value }))
            }
          />
        )}
        {/* One button per action the adapter declared, then the probe every
            schema-declaring adapter gets. No adapter is named here. */}
        {(formSpec?.actions ?? []).map((action) => (
          <Pressable
            key={action.key}
            accessibilityRole="button"
            accessibilityLabel={action.label}
            accessibilityHint={action.hint ?? undefined}
            accessibilityState={{ disabled: busyAction != null || submitting }}
            disabled={busyAction != null || submitting}
            onPress={() => void runDeclaredAction(action)}
            style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
          >
            <Text style={styles.addButtonText}>
              {busyAction === action.key && action.busy_label
                ? action.busy_label
                : action.label}
            </Text>
          </Pressable>
        ))}
        {formSpec != null && (
          <Pressable
            accessibilityRole="button"
            accessibilityLabel={t('dialogs.accounts.testConnection')}
            accessibilityState={{ disabled: busyAction != null || submitting }}
            disabled={busyAction != null || submitting}
            onPress={() => void testSchemaConnection()}
            style={({ pressed }) => [styles.addButton, pressed && styles.pressed]}
          >
            <Text style={styles.addButtonText}>
              {busyAction === TEST_ACTION_KEY
                ? t('dialogs.accounts.testing')
                : t('dialogs.accounts.testConnection')}
            </Text>
          </Pressable>
        )}
        {testMessage != null && (
          <Text style={styles.hint} accessibilityLiveRegion="polite">
            {testMessage}
          </Text>
        )}
        {error != null && <Text style={styles.error}>{error}</Text>}
      </AppDialog>

      <AppDialog
        visible={mode === 'device'}
        title={t('dialogs.accounts.kindName.device_calendar')}
        confirmLabel={t('dialogs.accounts.deviceGrantButton')}
        cancelLabel={t('mobile.cancel')}
        onConfirm={() => void addDevice()}
        onCancel={cancelAdd}
        busy={submitting}
      >
        <Text style={styles.deviceGrantBody}>
          {t(
            Platform.OS === 'android'
              ? 'dialogs.accounts.deviceGrantBodyAndroid'
              : 'dialogs.accounts.deviceGrantBody',
          )}
        </Text>
      </AppDialog>
    </FormScrollView>
    {/* One-shot contacts privacy notice (app-modal; overlays the screen). */}
    <ContactsPrivacyNoticeModal
      adapterKind={privacyNoticeFor}
      onAcknowledge={acknowledgePrivacyNotice}
    />
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    screen: { flex: 1, backgroundColor: c.background },
    content: { padding: 16, gap: 14 },
    heading: { fontSize: 20, fontWeight: '700', color: c.textPrimary },
    addHeading: { marginTop: 8 },
    list: { gap: 12 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowText: { flex: 1, gap: 2 },
    accountName: { fontSize: 18, color: c.textPrimary, fontWeight: '600' },
    accountKind: { fontSize: 14, color: c.textSecondary },
    badge: { fontSize: 13, fontWeight: '700', color: c.danger },
    hint: { fontSize: 13, color: c.textSecondary, lineHeight: 18 },
    deleteButton: {
      paddingVertical: 10,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.dangerBorder,
      backgroundColor: c.dangerBg,
    },
    deleteButtonText: { fontSize: 15, fontWeight: '600', color: c.danger },
    editInput: {
      flex: 1,
      fontSize: 17,
      color: c.textPrimary,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.accent,
      backgroundColor: c.background,
    },
    smallButton: {
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    smallButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    pressed: { opacity: 0.7 },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    deviceGrantBody: { fontSize: 15, lineHeight: 21, color: c.textPrimary },
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
    addButton: {
      marginTop: 8,
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    addButtonPressed: { backgroundColor: c.accentPressed },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
    secondaryButton: {
      paddingVertical: 14,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    secondaryButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    discoverButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    discoverButtonDisabled: { opacity: 0.5 },
    discoverButtonText: { fontSize: 15, fontWeight: '600', color: c.link },
    muted: { fontSize: 15, color: c.textSecondary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
