import { useTranslation } from 'react-i18next';
import { StyleSheet, Switch, Text, TextInput, View } from 'react-native';

import type { AccountFormField, AccountFormSpec } from '@aperio/shared';

import { useThemedStyles, type ThemeColors } from '../theme';

/**
 * The connect form for an adapter, rendered from what that adapter declared —
 * the mobile twin of the desktop `AccountSchemaForm`.
 *
 * Knows no provider. It receives the field list an adapter published in its
 * `plugin.json` and renders it, which is the whole point: adding an adapter
 * must not mean editing either frontend.
 *
 * Labels come from the adapter's `label_key` when the app has that translation,
 * and from its literal `label` otherwise — so a bundled adapter follows the
 * user's language while a third-party one still reads sensibly.
 *
 * When the build carries credentials for the provider, the two OAuth client
 * fields are not rendered at all: two empty inputs that need not be filled read
 * as "you must supply these", and with a screen reader they are two more stops
 * on the way to the button for nothing.
 */
export function AccountSchemaForm({
  spec,
  values,
  onChange,
}: {
  spec: AccountFormSpec;
  values: Record<string, string | boolean>;
  onChange: (key: string, value: string | boolean) => void;
}) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);

  const hidden = new Set<string>();
  if (spec.oauth?.builtin) {
    hidden.add(spec.oauth.client_id_field);
    if (spec.oauth.client_secret_field) {
      hidden.add(spec.oauth.client_secret_field);
    }
  }

  const resolve = (literal: string, key: string | null) =>
    key ? t(key, { defaultValue: literal }) : literal;
  const label = (field: AccountFormField) =>
    resolve(field.label, field.label_key);
  const hint = (field: AccountFormField) =>
    field.hint || field.hint_key ? resolve(field.hint ?? '', field.hint_key) : null;

  return (
    <View style={styles.group}>
      {spec.oauth && !spec.oauth.builtin && (
        <Text style={styles.note}>
          {t('dialogs.accounts.oauthOwnIntegrationHint')}
        </Text>
      )}
      {spec.fields
        .filter((field) => !hidden.has(field.key))
        .map((field) => {
          const description = hint(field);
          if (field.kind === 'bool') {
            const checked =
              typeof values[field.key] === 'boolean'
                ? (values[field.key] as boolean)
                : (field.default_bool ?? false);
            return (
              <View key={field.key} style={styles.switchRow}>
                <View style={styles.switchText}>
                  <Text style={styles.label}>{label(field)}</Text>
                  {description && <Text style={styles.hint}>{description}</Text>}
                </View>
                <Switch
                  value={checked}
                  onValueChange={(next) => onChange(field.key, next)}
                  accessibilityLabel={label(field)}
                  accessibilityHint={description ?? undefined}
                />
              </View>
            );
          }
          const value =
            typeof values[field.key] === 'string'
              ? (values[field.key] as string)
              : (field.default_text ?? '');
          return (
            <View key={field.key} style={styles.field}>
              <Text style={styles.label}>{label(field)}</Text>
              <TextInput
                style={styles.input}
                value={value}
                onChangeText={(next) => onChange(field.key, next)}
                secureTextEntry={field.kind === 'secret'}
                keyboardType={field.kind === 'url' ? 'url' : 'default'}
                autoCapitalize="none"
                autoCorrect={false}
                accessibilityLabel={label(field)}
                accessibilityHint={description ?? undefined}
              />
              {description && <Text style={styles.hint}>{description}</Text>}
            </View>
          );
        })}
      {spec.oauth && (
        <Text style={styles.note}>{t('dialogs.accounts.oauthFlowHint')}</Text>
      )}
    </View>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    group: { gap: 12 },
    field: { gap: 4 },
    label: { fontSize: 15, fontWeight: '600', color: c.textPrimary },
    hint: { fontSize: 13, color: c.textSecondary },
    note: { fontSize: 13, color: c.textSecondary },
    input: {
      borderWidth: 1,
      borderColor: c.border,
      borderRadius: 8,
      paddingHorizontal: 12,
      paddingVertical: 10,
      fontSize: 16,
      color: c.textPrimary,
      backgroundColor: c.surface,
    },
    switchRow: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
    },
    switchText: { flex: 1, gap: 4 },
  });
