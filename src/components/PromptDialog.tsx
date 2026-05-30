import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { Modal } from './Modal';

/**
 * Generic single-text-input prompt, shown when an action needs a name
 * before it runs (new calendar / task list / address book).
 *
 * Renders through {@link Modal}, so it inherits the focus trap, inert
 * shell, Escape-closes and focus-restore guarantees every dialog in the
 * app provides. On open the input is focused and its suggested text
 * pre-selected: a user happy with the default just presses Enter, while
 * anyone wanting their own name types straight over the selection. The
 * primary button is disabled while the (trimmed) name is empty, and
 * submit trims before handing the value back.
 */
export interface PromptDialogProps {
  isOpen: boolean;
  onClose: () => void;
  /** Called with the trimmed, non-empty name on submit. */
  onSubmit: (name: string) => void;
  title: string;
  /** Label shown above the text field. */
  label: string;
  /** Pre-filled (and pre-selected) suggested value. */
  defaultValue?: string;
  /** Label for the confirm button — pass a create-specific one; there is
   *  deliberately no default (the shared confirm label reads "Delete"). */
  submitLabel: string;
  cancelLabel?: string;
}

export function PromptDialog({
  isOpen,
  onClose,
  onSubmit,
  title,
  label,
  defaultValue = '',
  submitLabel,
  cancelLabel,
}: PromptDialogProps) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(defaultValue);

  // Reset to the suggested default each time the dialog (re)opens, then
  // focus + select so Enter accepts the suggestion and typing replaces it.
  useEffect(() => {
    if (!isOpen) return;
    setValue(defaultValue);
    queueMicrotask(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [isOpen, defaultValue]);

  const trimmed = value.trim();

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!trimmed) return;
    onSubmit(trimmed);
    onClose();
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={title}
      className="modal--prompt"
      dismissOnBackdrop={false}
    >
      <form onSubmit={handleSubmit} className="form">
        <label className="form__field">
          <span className="form__label">{label}</span>
          <input
            ref={inputRef}
            type="text"
            value={value}
            onChange={(e) => setValue(e.target.value)}
            autoComplete="off"
          />
        </label>
        <div className="form__actions">
          <button type="button" onClick={onClose} className="form__action">
            {cancelLabel ?? t('dialogs.confirm.cancel')}
          </button>
          <button
            type="submit"
            className="form__action form__action--primary"
            disabled={!trimmed}
          >
            {submitLabel}
          </button>
        </div>
      </form>
    </Modal>
  );
}
