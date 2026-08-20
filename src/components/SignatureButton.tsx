import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { applySignature, signatureIn } from '@aperio/shared';

import { useAnnouncer } from '../a11y/announcerContext';
import { useSignatures } from '../state/useSignatures';
import { Modal } from './Modal';

/**
 * Put a signature at the end of a description.
 *
 * One press when the calendar has a bound signature — that is the case this
 * exists for, and it should not cost a dialog. The dialog is for the
 * exceptions: a different signature, or taking one back out.
 *
 * Insertion is idempotent and replaces rather than appends (see
 * `shared/signatures.ts`), so pressing twice, or switching calendars and
 * pressing again, leaves exactly one block.
 */
export function SignatureButton({
  boundTo,
  description,
  onChange,
  className = 'form__action',
}: {
  /** The container whose bound signature is offered by default — a calendar
   *  id today. Empty means "no binding": the button always asks, which is
   *  what the task editor does, since a task belongs to a LIST and lists
   *  carry no binding yet. */
  boundTo: string;
  description: string;
  onChange: (next: string) => void;
  className?: string;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { signatures, forCalendar } = useSignatures(boundTo ? [boundTo] : []);
  const [open, setOpen] = useState(false);

  if (signatures.length === 0) return null;

  const bound = boundTo ? forCalendar(boundTo) : null;
  const present = signatureIn(description) !== null;

  const insert = (body: string, name: string) => {
    onChange(applySignature(description, body));
    announce(t('dialogs.signature.inserted', { name }));
    setOpen(false);
  };

  return (
    <>
      <button
        type="button"
        className={className}
        // The bound signature is named in the button, so the common case needs
        // no dialog and no memory of what this calendar is set to.
        aria-label={
          bound
            ? t('dialogs.signature.insertNamed', { name: bound.name })
            : t('dialogs.signature.choose')
        }
        onClick={() => {
          if (bound && !present) insert(bound.body, bound.name);
          else setOpen(true);
        }}
      >
        {t('dialogs.signature.short')}
      </button>

      <Modal
        isOpen={open}
        onClose={() => setOpen(false)}
        title={t('dialogs.signature.title')}
        className="modal--form modal--narrow"
      >
        <div className="quick-date__choices" role="group">
          {signatures.map((sig) => (
            <button
              key={sig.id}
              type="button"
              className="form__action"
              onClick={() => insert(sig.body, sig.name)}
            >
              {sig.name}
            </button>
          ))}
          {present && (
            <button
              type="button"
              className="form__action form__action--destructive"
              onClick={() => {
                onChange(applySignature(description, ''));
                announce(t('dialogs.signature.removed'));
                setOpen(false);
              }}
            >
              {t('dialogs.signature.remove')}
            </button>
          )}
        </div>
        <div className="modal__actions">
          <button
            type="button"
            className="form__action"
            onClick={() => setOpen(false)}
          >
            {t('dialogs.cancel')}
          </button>
        </div>
      </Modal>
    </>
  );
}
