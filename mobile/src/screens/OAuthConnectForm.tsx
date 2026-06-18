import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import { RadioGroup } from '../components/RadioGroup';
import { Account } from '../api/accounts';
import { connectOAuthAccount, type OAuthProvider } from '../api/oauth';

// The browser-sign-in half of the Accounts screen: connect a Google or Microsoft
// account via the host-driven OAuth flow (begin → native auth session →
// complete). Kept in its own file so the helper logic lives outside the screen
// (and the screen file exports only its component — react-refresh-clean).
//
// BYO client-id: the user pastes their own OAuth client credentials. Screen-
// reader-first — every field is a labelled element, the provider + Microsoft
// account-type are RadioGroups, and outcomes are announced by the parent.

type Authority = 'common' | 'consumers' | 'organizations';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

interface OAuthConnectFormProps {
  /** Announce intermediate (non-terminal) SR messages — "Signing in …", cancel. */
  announce: (message: string) => void;
  /** A connect succeeded — the parent reloads the list, moves focus, announces. */
  onConnected: (account: Account) => void;
  /** A connect failed — the parent shows + announces the (already-localised) message. */
  onError: (message: string) => void;
}

export default function OAuthConnectForm({
  announce,
  onConnected,
  onError,
}: OAuthConnectFormProps) {
  const { t } = useTranslation();

  const [provider, setProvider] = useState<OAuthProvider>('google');
  const [displayName, setDisplayName] = useState('');
  const [clientId, setClientId] = useState('');
  const [clientSecret, setClientSecret] = useState('');
  const [authority, setAuthority] = useState<Authority>('common');
  const [submitting, setSubmitting] = useState(false);

  const isMicrosoft = provider === 'microsoft_graph';

  const onChangeProvider = useCallback((next: OAuthProvider) => {
    setProvider(next);
    setClientId('');
    setClientSecret('');
    setAuthority('common');
  }, []);

  const connect = useCallback(async () => {
    const name = displayName.trim();
    const id = clientId.trim();
    const secret = clientSecret.trim();
    if (name.length === 0) {
      onError(t('dialogs.accounts.nameRequired'));
      return;
    }
    if (id.length === 0) {
      onError(t('dialogs.accounts.clientIdRequired'));
      return;
    }
    if (!isMicrosoft && secret.length === 0) {
      onError(t('dialogs.accounts.clientSecretRequired'));
      return;
    }

    setSubmitting(true);
    announce(t('mobile.oauthConnecting'));
    try {
      const result = await connectOAuthAccount({
        provider,
        displayName: name,
        clientId: id,
        clientSecret: isMicrosoft ? undefined : secret,
        authority: isMicrosoft ? authority : undefined,
      });
      if (result.kind === 'cancelled') {
        announce(t('mobile.oauthCancelled'));
        return;
      }
      setDisplayName('');
      setClientId('');
      setClientSecret('');
      onConnected(result.account);
    } catch (err) {
      const raw = errorMessage(err);
      onError(raw === 'OAUTH_NO_CODE' ? t('mobile.oauthNoCode') : raw);
    } finally {
      setSubmitting(false);
    }
  }, [
    announce,
    authority,
    clientId,
    clientSecret,
    displayName,
    isMicrosoft,
    onConnected,
    onError,
    provider,
    t,
  ]);

  return (
    <View style={styles.section}>
      <Text style={styles.heading} accessibilityRole="header">
        {t('mobile.oauthHeading')}
      </Text>

      <RadioGroup<OAuthProvider>
        label={t('mobile.oauthProviderLabel')}
        value={provider}
        options={[
          { value: 'google', label: t('dialogs.accounts.kindName.google') },
          {
            value: 'microsoft_graph',
            label: t('dialogs.accounts.kindName.microsoft_graph'),
          },
        ]}
        onChange={onChangeProvider}
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

      <View style={styles.field}>
        <Text style={styles.label}>
          {isMicrosoft
            ? t('dialogs.accounts.microsoftClientIdLabel')
            : t('dialogs.accounts.googleClientIdLabel')}
        </Text>
        <TextInput
          style={styles.input}
          value={clientId}
          onChangeText={setClientId}
          placeholder={
            isMicrosoft
              ? t('dialogs.accounts.microsoftClientIdPlaceholder')
              : t('dialogs.accounts.googleClientIdPlaceholder')
          }
          accessibilityLabel={
            isMicrosoft
              ? t('dialogs.accounts.microsoftClientIdLabel')
              : t('dialogs.accounts.googleClientIdLabel')
          }
          autoCapitalize="none"
          autoCorrect={false}
        />
        <Text style={styles.hint}>
          {isMicrosoft
            ? t('dialogs.accounts.microsoftClientIdHint')
            : t('dialogs.accounts.googleClientIdHint')}
        </Text>
      </View>

      {isMicrosoft ? (
        <RadioGroup<Authority>
          label={t('dialogs.accounts.microsoftAuthorityLabel')}
          value={authority}
          options={[
            {
              value: 'common',
              label: t('dialogs.accounts.microsoftAuthorityCommon'),
            },
            {
              value: 'consumers',
              label: t('dialogs.accounts.microsoftAuthorityConsumers'),
            },
            {
              value: 'organizations',
              label: t('dialogs.accounts.microsoftAuthorityOrganizations'),
            },
          ]}
          onChange={setAuthority}
        />
      ) : (
        <View style={styles.field}>
          <Text style={styles.label}>
            {t('dialogs.accounts.googleClientSecretLabel')}
          </Text>
          <TextInput
            style={styles.input}
            value={clientSecret}
            onChangeText={setClientSecret}
            placeholder={t('dialogs.accounts.googleClientSecretPlaceholder')}
            accessibilityLabel={t('dialogs.accounts.googleClientSecretLabel')}
            secureTextEntry
            autoCapitalize="none"
            autoCorrect={false}
          />
          <Text style={styles.hint}>
            {t('dialogs.accounts.googleClientSecretHint')}
          </Text>
        </View>
      )}

      <Text style={styles.hint}>
        {isMicrosoft
          ? t('dialogs.accounts.microsoftFlowHint')
          : t('dialogs.accounts.googleFlowHint')}
      </Text>

      <Pressable
        accessibilityRole="button"
        accessibilityState={{ disabled: submitting, busy: submitting }}
        accessibilityLabel={t('mobile.oauthSignIn')}
        disabled={submitting}
        onPress={() => void connect()}
        style={({ pressed }) => [
          styles.button,
          pressed && styles.buttonPressed,
          submitting && styles.buttonDisabled,
        ]}
      >
        <Text style={styles.buttonText}>
          {submitting ? t('mobile.oauthConnecting') : t('mobile.oauthSignIn')}
        </Text>
      </Pressable>
    </View>
  );
}

const styles = StyleSheet.create({
  section: { gap: 14, marginTop: 8 },
  heading: { fontSize: 20, fontWeight: '700', color: '#10131a' },
  field: { gap: 6 },
  label: { fontSize: 15, fontWeight: '600', color: '#2b3240' },
  hint: { fontSize: 13, color: '#5b6573' },
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
  button: {
    marginTop: 8,
    paddingVertical: 14,
    borderRadius: 10,
    backgroundColor: '#1d4ed8',
    alignItems: 'center',
  },
  buttonPressed: { backgroundColor: '#1740a8' },
  buttonDisabled: { backgroundColor: '#9aa9c9' },
  buttonText: { fontSize: 16, fontWeight: '700', color: '#ffffff' },
});
