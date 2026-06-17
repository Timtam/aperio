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
import {
  Account,
  AdapterKind,
  createAccount,
  deleteAccount,
  listAccounts,
} from '../api/accounts';

// Accounts management — list + add (non-OAuth kinds) + delete, over the Rust
// Host (statically-embedded adapter plugins + the keychain-bridged SecretStore).
// OAuth kinds (Google / Microsoft / the VC adapters) need the interactive
// browser flow and arrive in a later phase, so they're not offered here.
//
// Accessibility: every control is an addressable element with an explicit
// label; the kind picker is a RadioGroup; deletes are reachable both as a
// visible button and a custom accessibility action; results are announced and
// screen-reader focus is moved to the new row after a create.

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

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export default function AccountsScreen() {
  const { t } = useTranslation();

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // Add-form state.
  const [kind, setKind] = useState<keyof typeof KIND_FORMS>('caldav');
  const [displayName, setDisplayName] = useState('');
  const [config, setConfig] = useState<Record<string, string>>({});
  const [secret, setSecret] = useState('');
  const [submitting, setSubmitting] = useState(false);

  const rowTags = useRef<Record<string, number | null>>({});
  const pendingFocusId = useRef<string | null>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const load = useCallback(async () => {
    try {
      setAccounts(await listAccounts());
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
            return (
              <View
                key={account.id}
                ref={(node) => {
                  rowTags.current[account.id] = node ? findNodeHandle(node) : null;
                }}
                accessible
                accessibilityRole="text"
                accessibilityLabel={`${account.display_name}, ${kindName}`}
                accessibilityActions={
                  isLocal ? undefined : [{ name: 'delete', label: t('dialogs.accounts.delete') }]
                }
                onAccessibilityAction={(e) => {
                  if (e.nativeEvent.actionName === 'delete') void remove(account);
                }}
                style={styles.row}
              >
                <View style={styles.rowText}>
                  <Text style={styles.accountName}>{account.display_name}</Text>
                  <Text style={styles.accountKind}>{kindName}</Text>
                </View>
                {!isLocal && (
                  <Pressable
                    accessibilityRole="button"
                    accessibilityLabel={`${t('dialogs.accounts.delete')}: ${account.display_name}`}
                    onPress={() => void remove(account)}
                    style={({ pressed }) => [styles.deleteButton, pressed && styles.pressed]}
                  >
                    <Text style={styles.deleteButtonText}>{t('dialogs.accounts.delete')}</Text>
                  </Pressable>
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
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: '#ffffff' },
  content: { padding: 16, gap: 14 },
  heading: { fontSize: 20, fontWeight: '700', color: '#10131a' },
  addHeading: { marginTop: 8 },
  list: { gap: 12 },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  rowText: { flex: 1, gap: 2 },
  accountName: { fontSize: 18, color: '#10131a', fontWeight: '600' },
  accountKind: { fontSize: 14, color: '#5b6573' },
  deleteButton: {
    paddingVertical: 10,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#d9b3b0',
    backgroundColor: '#fbeceb',
  },
  deleteButtonText: { fontSize: 15, fontWeight: '600', color: '#b42318' },
  pressed: { opacity: 0.7 },
  field: { gap: 6 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  input: {
    fontSize: 17,
    color: '#10131a',
    paddingVertical: 12,
    paddingHorizontal: 14,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f8fafc',
  },
  addButton: {
    marginTop: 8,
    paddingVertical: 14,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  addButtonPressed: { backgroundColor: '#1740a8' },
  addButtonDisabled: { backgroundColor: '#9aa9c9' },
  addButtonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
  muted: { fontSize: 15, color: '#5b6573' },
  error: { fontSize: 15, fontWeight: '600', color: '#b42318' },
});
