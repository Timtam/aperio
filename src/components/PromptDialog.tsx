import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';

import type { ColorLabel } from '../api/types';
import { ColorLabelSelect } from './ColorLabelSelect';
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
  /** Called with the trimmed, non-empty name on submit. When a
   *  `colorField` is configured the chosen color-label id (or `null` for
   *  "no color") is passed as the second argument; otherwise it is
   *  `undefined`. */
  onSubmit: (name: string, colorLabelId?: string | null) => void;
  title: string;
  /** Label shown above the text field. */
  label: string;
  /** Pre-filled (and pre-selected) suggested value. */
  defaultValue?: string;
  /** Label for the confirm button — pass a create-specific one; there is
   *  deliberately no default (the shared confirm label reads "Delete"). */
  submitLabel: string;
  cancelLabel?: string;
  /** When set, also render a color-label picker (with a live swatch)
   *  under the name field — used by the "new calendar / address book"
   *  flow so the container's color is chosen from the SAME predefined
   *  color-labels as everything else (unified palette), and seen up
   *  front. The picked label's id is handed back on submit. */
  colorField?: {
    label: string;
    labels: ColorLabel[];
    noneLabel: string;
    defaultLabelId: string | null;
  };
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
  colorField,
}: PromptDialogProps) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement>(null);
  const wasOpenRef = useRef(false);
  const [value, setValue] = useState(defaultValue);
  const colorDefault = colorField?.defaultLabelId ?? null;
  const [colorLabelId, setColorLabelId] = useState<string | null>(colorDefault);

  // Seed the suggested default (and focus + select so Enter accepts it and
  // typing replaces it) exactly once per open — on the closed→open TRANSITION,
  // not on every `defaultValue` change while open. `defaultValue` is derived
  // from the container count / active language, so a background catalog refresh
  // or a locale switch would otherwise re-fire this and clobber the name the
  // user is typing, re-selecting the field under them.
  useEffect(() => {
    if (!isOpen) {
      wasOpenRef.current = false;
      return;
    }
    if (wasOpenRef.current) return; // already open: leave the in-progress input alone
    wasOpenRef.current = true;
    setValue(defaultValue);
    setColorLabelId(colorDefault);
    queueMicrotask(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [isOpen, defaultValue, colorDefault]);

  const trimmed = value.trim();

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!trimmed) return;
    onSubmit(trimmed, colorField ? colorLabelId : undefined);
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
        {colorField && (
          <label className="form__field">
            <span className="form__label">{colorField.label}</span>
            <ColorLabelSelect
              value={colorLabelId}
              onChange={setColorLabelId}
              labels={colorField.labels}
              noneLabel={colorField.noneLabel}
            />
          </label>
        )}
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
