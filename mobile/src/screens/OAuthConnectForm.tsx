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

import { RadioGroup } from '../components/RadioGroup';
import { useThemedStyles, type ThemeColors } from '../theme';
import { Account } from '../api/accounts';
import { connectOAuthAccount, type OAuthProvider } from '../api/oauth';

// The browser-sign-in half of the Accounts screen: connect a Google or Microsoft
// account via the host-driven OAuth flow (begin → native auth session →
// complete). Kept in its own file so the helper logic lives outside the screen
// (and the screen file exports only its component — react-refresh-clean).
//
// BYO client-id: the user pastes their own OAuth client credentials. Screen-
// reader-first — every field is a labelled element, the provider + Microsoft
// account-type are RadioGroups, the form owns its own error region (announced +
// SR-focused so a blind user can re-read it without hunting), and after the
// native auth session closes SR focus is moved to a known element (the new
// account row on success, via the parent; the sign-in button on cancel).

type Authority = 'common' | 'consumers' | 'organizations';

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

interface OAuthConnectFormProps {
  /** A connect succeeded — the parent reloads the list, moves focus, announces. */
  onConnected: (account: Account) => void;
  /** When set, the provider is fixed (the parent's picker already chose it), so
   *  the in-form provider RadioGroup is hidden. */
  lockedProvider?: OAuthProvider;
}

export default function OAuthConnectForm({
  onConnected,
  lockedProvider,
}: OAuthConnectFormProps) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const [provider, setProvider] = useState<OAuthProvider>(lockedProvider ?? 'google');
  const [displayName, setDisplayName] = useState('');
  const [clientId, setClientId] = useState('');
  const [clientSecret, setClientSecret] = useState('');
  const [authority, setAuthority] = useState<Authority>('common');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const buttonRef = useRef<View>(null);
  const errorRef = useRef<Text>(null);
  const headingRef = useRef<Text>(null);

  const isMicrosoft = provider === 'microsoft_graph';

  // The native auth session dismissing leaves RN screen-reader focus undefined,
  // so move it deliberately: to the error region when one appears (so it can be
  // re-read), else to the sign-in button on a cancel.
  useEffect(() => {
    if (error == null) return;
    const tag = errorRef.current ? findNodeHandle(errorRef.current) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, [error]);

  // When the form opens (the parent's picker chose this provider), land the
  // screen reader on the heading so the user knows the provider dialog appeared.
  useEffect(() => {
    const tag = headingRef.current ? findNodeHandle(headingRef.current) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  const focusButton = useCallback(() => {
    const tag = buttonRef.current ? findNodeHandle(buttonRef.current) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

  const onChangeProvider = useCallback((next: OAuthProvider) => {
    setProvider(next);
    setClientId('');
    setClientSecret('');
    setAuthority('common');
    setError(null);
  }, []);

  const connect = useCallback(async () => {
    const name = displayName.trim();
    const id = clientId.trim();
    const secret = clientSecret.trim();
    setError(null);
    if (name.length === 0) {
      setError(t('dialogs.accounts.nameRequired'));
      return;
    }
    if (id.length === 0) {
      setError(t('dialogs.accounts.clientIdRequired'));
      return;
    }
    if (!isMicrosoft && secret.length === 0) {
      setError(t('dialogs.accounts.clientSecretRequired'));
      return;
    }

    setSubmitting(true);
    AccessibilityInfo.announceForAccessibility(t('mobile.oauthConnecting'));
    try {
      const result = await connectOAuthAccount({
        provider,
        displayName: name,
        clientId: id,
        clientSecret: isMicrosoft ? undefined : secret,
        authority: isMicrosoft ? authority : undefined,
      });
      if (result.kind === 'cancelled') {
        AccessibilityInfo.announceForAccessibility(t('mobile.oauthCancelled'));
        focusButton();
        return;
      }
      setDisplayName('');
      setClientId('');
      setClientSecret('');
      onConnected(result.account);
    } catch (err) {
      const raw = errorMessage(err);
      setError(
        raw === 'OAUTH_NO_CODE'
          ? t('mobile.oauthNoCode')
          : t('mobile.error', { message: raw }),
      );
    } finally {
      setSubmitting(false);
    }
  }, [
    authority,
    clientId,
    clientSecret,
    displayName,
    focusButton,
    isMicrosoft,
    onConnected,
    provider,
    t,
  ]);

  return (
    <View style={styles.section}>
      <Text ref={headingRef} style={styles.heading} accessibilityRole="header">
        {t('mobile.oauthHeading')}
      </Text>

      {lockedProvider == null && (
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
      )}

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

      {error != null && (
        <Text
          ref={errorRef}
          accessible
          accessibilityRole="text"
          accessibilityLiveRegion="assertive"
          style={styles.error}
        >
          {error}
        </Text>
      )}

      <Pressable
        ref={buttonRef}
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

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    section: { gap: 14, marginTop: 8 },
    heading: { fontSize: 20, fontWeight: '700', color: c.textPrimary },
    field: { gap: 6 },
    label: { fontSize: 15, fontWeight: '600', color: c.textLabel },
    hint: { fontSize: 13, color: c.textSecondary },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
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
    button: {
      marginTop: 8,
      paddingVertical: 14,
      borderRadius: 10,
      backgroundColor: c.accent,
      alignItems: 'center',
    },
    buttonPressed: { backgroundColor: c.accentPressed },
    buttonDisabled: { backgroundColor: c.accentDisabled },
    buttonText: { fontSize: 16, fontWeight: '700', color: c.textOnAccent },
  });
