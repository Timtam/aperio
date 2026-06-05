import { useEffect, useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  createColorLabel,
  getOrCreateAdHocColorLabel,
  isCommandError,
} from '../api/client';
import type { ColorLabel } from '../api/types';
import { useCalendarStore } from '../state/calendarStoreContext';
import { ColorComposer } from './ColorComposer';
import { Modal } from './Modal';

const DEFAULT_HEX = '#e53935';
const HEX_RE = /^#[0-9a-fA-F]{6}$/;

export interface ColorPickerModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** Seed color for the composer (e.g. the item/container's current color). */
  initialHex?: string | null;
  /** Called with the chosen/created color label once the user applies. */
  onResolve: (label: ColorLabel) => void | Promise<void>;
}

/**
 * Compose an arbitrary custom color and apply it. The accessible
 * {@link ColorComposer} (hex text input + hidden native swatch) is the
 * input. "Apply" alone resolves the color to a hidden, hex-deduplicated
 * ad-hoc color label (so the palette stays clean); ticking "save to
 * palette" + a name creates a normal named label instead. Either way the
 * chosen label id is handed back via `onResolve` — the caller decides
 * whether to set a form field (event/task dialog) or write a container
 * binding (sidebar).
 *
 * Rendered as a *local* `<Modal>` by each host, so an opener that is itself
 * a dialog (the event/task editor) keeps its form state mounted underneath.
 */
export function ColorPickerModal({
  isOpen,
  onClose,
  initialHex,
  onResolve,
}: ColorPickerModalProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { refreshColorLabels } = useCalendarStore();

  const [hex, setHex] = useState(initialHex || DEFAULT_HEX);
  const [saveToPalette, setSaveToPalette] = useState(false);
  const [name, setName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // Reset the form each time the modal opens.
  useEffect(() => {
    if (isOpen) {
      setHex(initialHex || DEFAULT_HEX);
      setSaveToPalette(false);
      setName('');
      setError(null);
    }
  }, [isOpen, initialHex]);

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(null);
    if (!HEX_RE.test(hex)) {
      setError(t('common.colorComposer.invalidHex'));
      return;
    }
    const trimmedName = name.trim();
    if (saveToPalette && !trimmedName) {
      setError(t('dialogs.colorPicker.nameRequired'));
      return;
    }
    setSubmitting(true);
    try {
      const label = saveToPalette
        ? await createColorLabel({ name: trimmedName, hex })
        : await getOrCreateAdHocColorLabel(hex);
      // Make the new/used label resolvable for the live swatch before the
      // caller binds it.
      await refreshColorLabels();
      await onResolve(label);
      announce(t('dialogs.colorPicker.applied'));
      onClose();
    } catch (err) {
      if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
      else setError(String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.colorPicker.title')}
      className="modal--color-picker"
      dismissOnBackdrop={false}
    >
      <form className="form" onSubmit={(e) => void onSubmit(e)}>
        <p className="form__hint">{t('dialogs.colorPicker.intro')}</p>
        <div className="form__field">
          <span className="form__label">
            {t('dialogs.colorPicker.colorLabel')}
          </span>
          <ColorComposer
            value={hex}
            onChange={setHex}
            label={t('dialogs.colorPicker.colorLabel')}
          />
        </div>
        <label className="color-picker__save-toggle">
          <input
            type="checkbox"
            checked={saveToPalette}
            onChange={(e) => setSaveToPalette(e.target.checked)}
          />
          <span>{t('dialogs.colorPicker.saveToPalette')}</span>
        </label>
        {saveToPalette && (
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.colorPicker.nameLabel')}
            </span>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              autoComplete="off"
            />
          </label>
        )}
        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}
        <div className="form__actions">
          <button type="button" className="form__action" onClick={onClose}>
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            className="form__action form__action--primary"
            aria-disabled={submitting || undefined}
          >
            {t('dialogs.colorPicker.applyAction')}
          </button>
        </div>
      </form>
    </Modal>
  );
}
