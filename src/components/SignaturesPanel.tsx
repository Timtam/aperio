import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Signature } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useSignatures } from '../state/useSignatures';

/**
 * Signature blocks, managed from Settings.
 *
 * A signature is a named piece of text that goes at the end of a description —
 * a room's join details, a standing note, a department's dial-in. Mail clients
 * bind theirs to accounts; these bind to calendars, so the editor can offer the
 * right one without being asked.
 *
 * Plain text, deliberately and only: see `shared/signatures.ts` for why an
 * invitation cannot carry anything else.
 */
export function SignaturesPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { calendars } = useCalendarStore();
  const writable = calendars.filter((c) => !c.read_only);
  const { signatures, loading, save, forCalendar, bind } = useSignatures(
    writable.map((c) => c.id),
  );

  const [newName, setNewName] = useState('');
  const [newBody, setNewBody] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const listId = useId();
  const sectionRef = useRef<HTMLDivElement>(null);

  const onAdd = useCallback(async () => {
    const name = newName.trim();
    if (!name) {
      setError(t('dialogs.settings.signatures.nameRequired'));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await save([
        ...signatures,
        // `crypto.randomUUID` rather than a counter: two devices adding a
        // signature between sync rounds must not mint the same id.
        { id: crypto.randomUUID(), name, body: newBody },
      ]);
      setNewName('');
      setNewBody('');
      announce(t('dialogs.settings.signatures.added', { name }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }, [newName, newBody, signatures, save, announce, t]);

  const onChange = useCallback(
    async (id: string, patch: Partial<Signature>) => {
      await save(signatures.map((s) => (s.id === id ? { ...s, ...patch } : s)));
    },
    [signatures, save],
  );

  const onDelete = useCallback(
    async (sig: Signature) => {
      setBusy(true);
      try {
        await save(signatures.filter((s) => s.id !== sig.id));
        // Deliberately NOT unbinding the calendars that pointed at it: a
        // binding to a signature that no longer exists simply offers nothing,
        // and rewriting other people's calendar settings to tidy up would be a
        // bigger action than the one asked for.
        announce(t('dialogs.settings.signatures.deleted', { name: sig.name }));
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setBusy(false);
      }
    },
    [signatures, save, announce, t],
  );

  return (
    <div className="settings-panel" ref={sectionRef} tabIndex={-1}>
      <h3>{t('dialogs.settings.signatures.heading')}</h3>
      <p className="form__hint">{t('dialogs.settings.signatures.intro')}</p>

      {error && (
        <p className="form__error" role="alert">
          {error}
        </p>
      )}

      {loading ? (
        <p className="form__hint">{t('dialogs.settings.signatures.loading')}</p>
      ) : signatures.length === 0 ? (
        <p className="form__hint">{t('dialogs.settings.signatures.empty')}</p>
      ) : (
        signatures.map((sig) => (
          <SignatureRow
            key={sig.id}
            signature={sig}
            busy={busy}
            onChange={onChange}
            onDelete={() => void onDelete(sig)}
          />
        ))
      )}

      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          if (!busy) void onAdd();
        }}
      >
        <h4>{t('dialogs.settings.signatures.addHeading')}</h4>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.signatures.nameLabel')}
          </span>
          <input
            type="text"
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            autoComplete="off"
          />
        </label>
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.settings.signatures.bodyLabel')}
          </span>
          <textarea
            rows={4}
            value={newBody}
            onChange={(e) => setNewBody(e.target.value)}
          />
          <span className="form__hint">
            {t('dialogs.settings.signatures.bodyHint')}
          </span>
        </label>
        <button type="submit" className="form__action" aria-disabled={busy}>
          {t('dialogs.settings.signatures.add')}
        </button>
      </form>

      {signatures.length > 0 && writable.length > 0 && (
        <section aria-labelledby={`${listId}-bindings`}>
          <h4 id={`${listId}-bindings`}>
            {t('dialogs.settings.signatures.bindingsHeading')}
          </h4>
          <p className="form__hint">
            {t('dialogs.settings.signatures.bindingsHint')}
          </p>
          {writable.map((cal) => (
            <label className="form__field" key={cal.id}>
              <span className="form__label">{cal.name}</span>
              <select
                value={forCalendar(cal.id)?.id ?? ''}
                onChange={(e) => void bind(cal.id, e.target.value || null)}
              >
                <option value="">
                  {t('dialogs.settings.signatures.bindingNone')}
                </option>
                {signatures.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </select>
            </label>
          ))}
        </section>
      )}
    </div>
  );
}

/** One signature, edited in place. Writes on blur, like the phone's marker
 *  rows: no Save to hunt for, and backing out cannot lose an edit. */
function SignatureRow({
  signature,
  busy,
  onChange,
  onDelete,
}: {
  signature: Signature;
  busy: boolean;
  onChange: (id: string, patch: Partial<Signature>) => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(signature.name);
  const [body, setBody] = useState(signature.body);

  // Adopt changes that arrived from elsewhere (a sync round) while this row is
  // not being edited.
  useEffect(() => setName(signature.name), [signature.name]);
  useEffect(() => setBody(signature.body), [signature.body]);

  return (
    <div className="settings-list__row settings-list__row--stacked">
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.signatures.nameLabel')}
        </span>
        <input
          type="text"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            if (name.trim() && name !== signature.name) {
              onChange(signature.id, { name: name.trim() });
            }
          }}
        />
      </label>
      <label className="form__field">
        <span className="form__label">
          {t('dialogs.settings.signatures.bodyLabel')}
        </span>
        <textarea
          rows={4}
          value={body}
          onChange={(e) => setBody(e.target.value)}
          onBlur={() => {
            if (body !== signature.body) onChange(signature.id, { body });
          }}
        />
      </label>
      <button
        type="button"
        className="form__action form__action--destructive"
        aria-disabled={busy}
        aria-label={`${t('dialogs.settings.signatures.delete')}: ${signature.name}`}
        onClick={() => {
          if (!busy) onDelete();
        }}
      >
        {t('dialogs.settings.signatures.delete')}
      </button>
    </div>
  );
}
