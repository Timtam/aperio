import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import { RadioGroup } from '../components/RadioGroup';
import { useThemedStyles, type ThemeColors } from '../theme';
import {
  Account,
  AdapterKind,
  createAccount,
  deleteAccount,
  discoverEwsEndpoint,
  listAccounts,
  listAccountsMissingCredentials,
  renameAccount,
  setAccountSecret,
} from '../api/accounts';
import { reconnectOAuthAccount } from '../api/oauth';
import OAuthConnectForm from './OAuthConnectForm';

// Accounts management — list + add (credential kinds) + connect (OAuth kinds via
// the browser sign-in flow) + delete, over the Rust Host (statically-embedded
// adapter plugins + the keychain-bridged SecretStore). The non-OAuth kinds use
// the inline credential form below; Google / Microsoft use the host-driven
// native auth session (see OAuthConnectForm).
//
// Accessibility: every control is an addressable element with an explicit
// label; the kind picker is a RadioGroup; deletes are reachable both as a
// visible button and a custom accessibility action; results are announced and
// screen-reader focus is moved to the new row after a create/connect.

interface ConfigField {
  jsonKey: string;
  labelKey: string;
  optional?: boolean;
  autoCapitalizeNone?: boolean;
}

interface KindForm {
  configFields: ConfigField[];
  /** The credential field, when the kind needs one. */
  secret?: { labelKey: string; optional?: boolean };
  /** Non-secret config merged verbatim (e.g. CalDAV's auth_kind). */
  fixedConfig?: Record<string, string>;
}

// The kinds with a non-OAuth construction path — the ones the Host accepts.
const KIND_FORMS: Record<Exclude<AdapterKind, 'google' | 'microsoft_graph' | 'zoom' | 'teams' | 'meet' | 'webex'>, KindForm> = {
  local: { configFields: [] },
  caldav: {
    configFields: [
      { jsonKey: 'server_url', labelKey: 'dialogs.accounts.serverUrlLabel', autoCapitalizeNone: true },
      { jsonKey: 'username', labelKey: 'dialogs.accounts.usernameLabel', autoCapitalizeNone: true },
    ],
    secret: { labelKey: 'dialogs.accounts.passwordLabel' },
    fixedConfig: { auth_kind: 'basic' },
  },
  ical: {
    configFields: [
      { jsonKey: 'feed_url', labelKey: 'dialogs.accounts.feedUrlLabel', autoCapitalizeNone: true },
      { jsonKey: 'username', labelKey: 'dialogs.accounts.icalUsernameLabel', optional: true, autoCapitalizeNone: true },
    ],
    secret: { labelKey: 'dialogs.accounts.icalPasswordLabel', optional: true },
  },
  ews: {
    configFields: [
      { jsonKey: 'endpoint', labelKey: 'dialogs.accounts.ewsEndpointLabel', autoCapitalizeNone: true },
      { jsonKey: 'username', labelKey: 'dialogs.accounts.usernameLabel', autoCapitalizeNone: true },
    ],
    secret: { labelKey: 'dialogs.accounts.passwordLabel' },
  },
  vikunja: {
    configFields: [
      { jsonKey: 'server_url', labelKey: 'dialogs.accounts.vikunjaServerUrlLabel', autoCapitalizeNone: true },
    ],
    secret: { labelKey: 'dialogs.accounts.vikunjaApiTokenLabel' },
  },
  todoist: {
    configFields: [],
    secret: { labelKey: 'dialogs.accounts.todoistApiTokenLabel' },
  },
};

const OFFERED_KINDS = Object.keys(KIND_FORMS) as (keyof typeof KIND_FORMS)[];

/** OAuth kinds can't be repaired with a pasted secret — they re-run the
 *  provider sign-in (a separate reconnect flow), so the inline credential field
 *  is offered only for the password/token kinds. */
const isOAuthKind = (kind: AdapterKind): boolean =>
  kind === 'google' || kind === 'microsoft_graph';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function AccountsScreen() {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

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

  // Add-form state.
  const [kind, setKind] = useState<keyof typeof KIND_FORMS>('caldav');
  const [displayName, setDisplayName] = useState('');
  const [config, setConfig] = useState<Record<string, string>>({});
  const [secret, setSecret] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [discovering, setDiscovering] = useState(false);

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

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

  // Move screen-reader focus to the newly created row once the list re-renders.
  useEffect(() => {
    if (pendingFocusId.current == null) return;
    const id = pendingFocusId.current;
    pendingFocusId.current = null;
    const tag = rowTags.current[id];
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [accounts]);

  const form = KIND_FORMS[kind];

  const resetForm = useCallback(() => {
    setDisplayName('');
    setConfig({});
    setSecret('');
  }, []);

  const onChangeKind = useCallback((next: keyof typeof KIND_FORMS) => {
    setKind(next);
    setConfig({});
    setSecret('');
  }, []);

  const add = useCallback(async () => {
    const name = displayName.trim();
    if (name.length === 0) {
      setError(t('dialogs.accounts.nameRequired'));
      announce(t('dialogs.accounts.nameRequired'));
      return;
    }
    setError(null);
    setSubmitting(true);
    try {
      const configObject: Record<string, string> = { ...(form.fixedConfig ?? {}) };
      for (const field of form.configFields) {
        const value = (config[field.jsonKey] ?? '').trim();
        if (value.length > 0) configObject[field.jsonKey] = value;
      }
      const trimmedSecret = secret.trim();
      const created = await createAccount({
        adapter_kind: kind,
        display_name: name,
        config_json: JSON.stringify(configObject),
        secret: form.secret && trimmedSecret.length > 0 ? trimmedSecret : null,
      });
      resetForm();
      await load();
      pendingFocusId.current = created.id;
      announce(t('dialogs.accounts.created', { name }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setSubmitting(false);
    }
  }, [announce, config, displayName, form, kind, load, resetForm, secret, t]);

  // EWS Autodiscover: derive the endpoint from the email + password (the EWS
  // form's username field holds the email) and pre-fill the endpoint + username,
  // mirroring the desktop "Discover URL" button. Network call → the typed plugin
  // message surfaces on failure so the user can enter the endpoint manually.
  const discover = useCallback(async () => {
    const email = (config.username ?? '').trim();
    const password = secret.trim();
    if (email.length === 0) {
      setError(t('dialogs.accounts.ewsDiscoverNeedsEmail'));
      announce(t('dialogs.accounts.ewsDiscoverNeedsEmail'));
      return;
    }
    if (password.length === 0) {
      setError(t('dialogs.accounts.ewsDiscoverNeedsPassword'));
      announce(t('dialogs.accounts.ewsDiscoverNeedsPassword'));
      return;
    }
    setError(null);
    setDiscovering(true);
    try {
      const result = await discoverEwsEndpoint(email, password);
      setConfig((c) => ({
        ...c,
        endpoint: result.ews_url,
        username: result.account_email,
      }));
      announce(t('dialogs.accounts.ewsDiscoverOk', { url: result.ews_url }));
    } catch (err) {
      const message = errorMessage(err);
      setError(message);
      announce(t('mobile.error', { message }));
    } finally {
      setDiscovering(false);
    }
  }, [announce, config, secret, t]);

  const remove = useCallback(
    async (account: Account) => {
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
        const result = await reconnectOAuthAccount(account);
        if (result.kind === 'cancelled') return;
        pendingFocusId.current = account.id;
        await load();
        announce(
          t('dialogs.accounts.credentialUpdated', { name: account.display_name }),
        );
      } catch (err) {
        const message = errorMessage(err);
        setError(message);
        announce(t('mobile.error', { message }));
      }
    },
    [announce, load, t],
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

  // OAuth (browser sign-in) success — the form owns the begin/exchange dance, its
  // own validation/error region, and the cancel announcement; the screen only
  // reloads the list, moves SR focus to the new row, and announces the result.
  const onOAuthConnected = useCallback(
    async (account: Account) => {
      setError(null);
      await load();
      pendingFocusId.current = account.id;
      announce(t('dialogs.accounts.created', { name: account.display_name }));
    },
    [announce, load, t],
  );

  return (
    <ScrollView
      style={styles.screen}
      contentContainerStyle={styles.content}
      keyboardShouldPersistTaps="handled"
    >
      {/* Existing accounts */}
      <Text style={styles.heading} accessibilityRole="header">
        {t('dialogs.accounts.existingHeading')}
      </Text>

      {error != null && (
        <Text style={styles.error} accessibilityRole="text" accessibilityLiveRegion="assertive">
          {error}
        </Text>
      )}

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
            const oauth = isOAuthKind(account.adapter_kind);
            // Fold the credential state into the row's single SR label; a
            // "Reconnect" affordance follows for both kinds (OAuth re-runs the
            // provider sign-in; others reveal the inline secret field).
            const rowLabel = missing
              ? `${account.display_name}, ${kindName}, ${t('dialogs.accounts.missingBadge')}`
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
                        ...(missing
                          ? [{ name: 'reconnect', label: t('dialogs.accounts.reconnect') }]
                          : []),
                        { name: 'rename', label: t('mobile.rename') },
                        { name: 'delete', label: t('dialogs.accounts.delete') },
                      ]
                }
                onAccessibilityAction={(e) => {
                  if (e.nativeEvent.actionName === 'delete') void remove(account);
                  else if (e.nativeEvent.actionName === 'rename') startRename(account);
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
                  {missing && (
                    <Text style={styles.badge} importantForAccessibility="no">
                      {t('dialogs.accounts.missingBadge')}
                    </Text>
                  )}
                </View>
                {!isLocal && (
                  <>
                    {missing && (
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

      {/* Add an account */}
      <Text style={[styles.heading, styles.addHeading]} accessibilityRole="header">
        {t('dialogs.accounts.addHeading')}
      </Text>

      <RadioGroup<keyof typeof KIND_FORMS>
        label={t('dialogs.accounts.kindLabel')}
        value={kind}
        options={OFFERED_KINDS.map((k) => ({
          value: k,
          label: t(`dialogs.accounts.kindName.${k}`),
        }))}
        onChange={onChangeKind}
      />

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

      {form.configFields.map((field) => (
        <View key={field.jsonKey} style={styles.field}>
          <Text style={styles.label}>{t(field.labelKey)}</Text>
          <TextInput
            style={styles.input}
            value={config[field.jsonKey] ?? ''}
            onChangeText={(v) => setConfig((c) => ({ ...c, [field.jsonKey]: v }))}
            accessibilityLabel={t(field.labelKey)}
            autoCapitalize={field.autoCapitalizeNone ? 'none' : 'sentences'}
            autoCorrect={!field.autoCapitalizeNone}
          />
        </View>
      ))}

      {form.secret != null && (
        <View style={styles.field}>
          <Text style={styles.label}>{t(form.secret.labelKey)}</Text>
          <TextInput
            style={styles.input}
            value={secret}
            onChangeText={setSecret}
            accessibilityLabel={t(form.secret.labelKey)}
            secureTextEntry
            autoCapitalize="none"
            autoCorrect={false}
          />
        </View>
      )}

      {kind === 'ews' && (
        <Pressable
          accessibilityRole="button"
          accessibilityState={{ disabled: discovering, busy: discovering }}
          accessibilityLabel={t('dialogs.accounts.ewsDiscover')}
          accessibilityHint={t('dialogs.accounts.ewsDiscoverSrHint')}
          disabled={discovering}
          onPress={() => void discover()}
          style={({ pressed }) => [
            styles.discoverButton,
            pressed && styles.pressed,
            discovering && styles.discoverButtonDisabled,
          ]}
        >
          <Text style={styles.discoverButtonText}>
            {discovering
              ? t('dialogs.accounts.ewsDiscovering')
              : t('dialogs.accounts.ewsDiscover')}
          </Text>
        </Pressable>
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: submitting }}
        accessibilityLabel={t('dialogs.accounts.add')}
        disabled={submitting}
        onPress={() => void add()}
        style={({ pressed }) => [
          styles.addButton,
          pressed && styles.addButtonPressed,
          submitting && styles.addButtonDisabled,
        ]}
      >
        <Text style={styles.addButtonText}>{t('dialogs.accounts.add')}</Text>
      </Pressable>

      {/* Connect a provider (browser sign-in) */}
      <OAuthConnectForm onConnected={(account) => void onOAuthConnected(account)} />
    </ScrollView>
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
    addButtonDisabled: { backgroundColor: c.accentDisabled },
    addButtonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
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
