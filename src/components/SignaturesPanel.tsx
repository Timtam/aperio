import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Signature } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { useSignatures } from '../state/useSignatures';

/**
 * Signature blocks, managed from Settings.
 *
 * A signature is a named piece of text that goes at the end of a description —
 * a room's join details, a standing note, a department's dial-in.
 *
 * This page WRITES them. Which calendar carries which is set on the calendar,
 * beside its default reminders, and a calendar that has one puts it on new
 * appointments by itself. Every signature stays pickable in the editors
 * regardless of the calendar, because "usually this one" is a default, not a
 * restriction.
 *
 * Plain text, deliberately and only: see `shared/signatures.ts` for why an
 * invitation cannot carry anything else.
 */
export function SignaturesPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  // Authoring only. WHICH calendar carries which signature is a property of
  // the calendar and is set in the calendar's own detail, beside its default
  // reminders — a matrix of every calendar was a poor way to answer a question
  // about one, and it only appeared once a signature existed, so the binding
  // read as missing entirely.
  const { signatures, loading, save } = useSignatures();

  const [newName, setNewName] = useState('');
  const [newBody, setNewBody] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
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
