import { useTranslation } from 'react-i18next';

import { FocusableNote } from '../a11y/FocusableNote';
import type { AccountFormField, AccountFormSpec } from '@aperio/shared';

/**
 * The connect form for an adapter, rendered from what that adapter declared.
 *
 * This component knows no provider. It receives the field list an adapter
 * published in its `plugin.json` and renders it — which is the whole point:
 * adding an adapter must not mean editing the frontend. An adapter Aperio's
 * authors have never seen gets the same form as the ones that ship with it.
 *
 * ## Labels
 *
 * Each field carries a literal `label` and, optionally, a `label_key`. Bundled
 * adapters set the key, so their strings live in the app's locale files where
 * translations belong, and follow the user's language. A third-party adapter
 * that ships no translations falls back to its literal, which is honest: a
 * label in the plugin author's English beats a missing-key marker.
 *
 * ## The OAuth credential pair
 *
 * When the build carries credentials for the provider, the two client fields
 * are not rendered at all. Showing two empty inputs that need not be filled
 * reads as "you must supply these" — and for a screen-reader user, two more
 * stops on the way to the button for nothing.
 */
export function AccountSchemaForm({
  spec,
  values,
  onChange,
}: {
  spec: AccountFormSpec;
  /** Current values, keyed by field key. Missing = the declared default. */
  values: Record<string, string | boolean>;
  onChange: (key: string, value: string | boolean) => void;
}) {
  const { t } = useTranslation();

  // A build with its own credentials asks for neither half of the pair; the
  // backend then signs in with what it carries.
  const hidden = new Set<string>();
  if (spec.oauth?.builtin) {
    hidden.add(spec.oauth.client_id_field);
    if (spec.oauth.client_secret_field) {
      hidden.add(spec.oauth.client_secret_field);
    }
  }

  /** A declared string wins when the app has a translation for it. */
  const text = (literal: string, key: string | null) =>
    key ? t(key, { defaultValue: literal }) : literal;

  const label = (field: AccountFormField) => text(field.label, field.label_key);
  const hint = (field: AccountFormField) =>
    field.hint || field.hint_key ? text(field.hint ?? '', field.hint_key) : null;

  const visible = spec.fields.filter((f) => !hidden.has(f.key));

  return (
    <>
      {spec.oauth && !spec.oauth.builtin && (
        <FocusableNote className="form__hint">
          {t('dialogs.accounts.oauthOwnIntegrationHint')}
        </FocusableNote>
      )}
      {visible.map((field) => {
        if (field.kind === 'bool') {
          const checked =
            typeof values[field.key] === 'boolean'
              ? (values[field.key] as boolean)
              : (field.default_bool ?? false);
          const description = hint(field);
          return (
            <div key={field.key}>
              <label className="form__checkbox">
                <input
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => onChange(field.key, e.target.checked)}
                />
                <span>{label(field)}</span>
              </label>
              {description && (
                <FocusableNote className="form__hint">
                  {description}
                </FocusableNote>
              )}
            </div>
          );
        }
        const value =
          typeof values[field.key] === 'string'
            ? (values[field.key] as string)
            : (field.default_text ?? '');
        const description = hint(field);
        return (
          <label className="form__field" key={field.key}>
            <span className="form__label">{label(field)}</span>
            <input
              type={field.kind === 'secret' ? 'password' : 'text'}
              inputMode={field.kind === 'url' ? 'url' : undefined}
              value={value}
              onChange={(e) => onChange(field.key, e.target.value)}
              autoComplete="off"
              spellCheck={false}
              required={field.required}
            />
            {description && <span className="form__hint">{description}</span>}
          </label>
        );
      })}
      {spec.oauth && (
        <FocusableNote className="form__hint">
          {t('dialogs.accounts.oauthFlowHint')}
        </FocusableNote>
      )}
    </>
  );
}
