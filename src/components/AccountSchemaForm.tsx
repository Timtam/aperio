import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

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
 *
 * ## Paths
 *
 * A `directory` or `file` field keeps its text input and gains a browse button
 * beside it. The input stays because typing a path is the reliable way in: it
 * works with the keyboard alone, it is what the SFTP key field has always
 * done, and a picker that REPLACED it would take that away. The button is the
 * convenience, not the mechanism.
 *
 * Cancelling changes nothing, deliberately — a picker that cleared the field on
 * cancel would destroy a path the user had typed, and "I changed my mind about
 * browsing" is not "I want this empty".
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
  /** The path inputs, so focus can land on the one that just changed. */
  const pathInputs = useRef<Record<string, HTMLInputElement | null>>({});
  const [browseError, setBrowseError] = useState<string | null>(null);

  const browse = useCallback(
    async (field: AccountFormField, button: HTMLButtonElement | null) => {
      setBrowseError(null);
      let picked: string | null;
      try {
        picked = (await openFileDialog({
          multiple: false,
          directory: field.kind === 'directory',
        })) as string | null;
      } catch (err) {
        // Never silent: a dialog that refuses to open would otherwise look
        // like a button that does nothing, and the way out — type the path —
        // is not obvious unless it is said.
        setBrowseError(
          t('dialogs.accounts.browseFailed', {
            message: err instanceof Error ? err.message : String(err),
          }),
        );
        button?.focus();
        return;
      }
      // Cancelled. The field keeps whatever it had, and focus goes back to the
      // button the user pressed — nothing happened, so nothing should move.
      if (picked == null) {
        button?.focus();
        return;
      }
      onChange(field.key, picked);
      // Land on the input itself: the screen reader then reads the field and
      // its NEW value, which is the confirmation. Announcing it separately
      // would say the path twice.
      pathInputs.current[field.key]?.focus();
    },
    [onChange, t],
  );

  // A build with its own credentials asks for neither half of the pair; the
  // backend then signs in with what it carries.
  const hidden = new Set<string>();
  if (spec.oauth?.builtin) {
    hidden.add(spec.oauth.client_id_field);
    if (spec.oauth.client_secret_field) {
      hidden.add(spec.oauth.client_secret_field);
    }
  }

  // Labels arrive already in the reader's language: the host resolved them
  // against the PLUGIN's own catalogue. There is nothing to translate here, and
  // nothing about somebody else's provider in the app's own strings.
  const label = (field: AccountFormField) => field.label;
  const hint = (field: AccountFormField) => field.hint;

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
        if (field.kind === 'choice') {
          // A native <select>, so the set stays closed and the browser gives
          // us keyboard behaviour and the listbox role for nothing. A text box
          // would let a typo through, and several adapters do not reject one —
          // the FTPS plugin falls back to explicit and connects differently
          // than the user asked, with nothing said.
          return (
            <label className="form__field" key={field.key}>
              <span className="form__label">{label(field)}</span>
              <select
                value={value}
                onChange={(e) => onChange(field.key, e.target.value)}
                required={field.required}
              >
                {field.options.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
              {description && <span className="form__hint">{description}</span>}
            </label>
          );
        }
        const isPath = field.kind === 'directory' || field.kind === 'file';
        return (
          <label className="form__field" key={field.key}>
            <span className="form__label">{label(field)}</span>
            <div className={isPath ? 'form__path' : undefined}>
              <input
                ref={
                  isPath
                    ? (el) => {
                        pathInputs.current[field.key] = el;
                      }
                    : undefined
                }
                type={
                  field.kind === 'secret'
                    ? 'password'
                    : field.kind === 'number'
                      ? 'number'
                      : 'text'
                }
                inputMode={
                  field.kind === 'url'
                    ? 'url'
                    : field.kind === 'number'
                      ? 'numeric'
                      : undefined
                }
                value={value}
                onChange={(e) => onChange(field.key, e.target.value)}
                autoComplete="off"
                spellCheck={false}
                required={field.required}
              />
              {isPath && (
                // Named after its field, because a form can carry two of these
                // — a folder to sync into and a key file — and "Browse" twice
                // in a row tells a screen-reader user nothing about which.
                <button
                  type="button"
                  className="form__action"
                  aria-label={t(
                    field.kind === 'directory'
                      ? 'dialogs.accounts.browseDirectoryNamed'
                      : 'dialogs.accounts.browseFileNamed',
                    { field: label(field) },
                  )}
                  onClick={(e) => void browse(field, e.currentTarget)}
                >
                  {t('dialogs.accounts.browse')}
                </button>
              )}
            </div>
            {description && <span className="form__hint">{description}</span>}
          </label>
        );
      })}
      {browseError && (
        <FocusableNote className="form__error">{browseError}</FocusableNote>
      )}
      {spec.oauth && (
        <FocusableNote className="form__hint">
          {t('dialogs.accounts.oauthFlowHint')}
        </FocusableNote>
      )}
    </>
  );
}
