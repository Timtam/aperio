import { useCallback, useState, type FormEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createColorLabel,
  deleteColorLabel,
  isCommandError,
  updateColorLabel,
} from '../api/client';
import type { ColorLabel } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { Modal } from './Modal';

/**
 * Color-label management dialog (DESIGN.md section 8.1).
 *
 * Shows the current label list with inline rename + delete and a
 * single-row form for creating new labels. Every change re-fetches the
 * store list so other dialogs (event / task selectors) see the same
 * data without a separate cache invalidation step.
 */
export interface ColorLabelDialogProps {
  isOpen: boolean;
  onClose: () => void;
}

const DEFAULT_NEW_HEX = '#e53935';

export function ColorLabelDialog({ isOpen, onClose }: ColorLabelDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { colorLabels, refreshColorLabels } = useCalendarStore();

  const [newName, setNewName] = useState('');
  const [newHex, setNewHex] = useState(DEFAULT_NEW_HEX);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const reportError = useCallback((err: unknown) => {
    if (isCommandError(err)) {
      setError(`${err.code}: ${err.message}`);
    } else {
      setError(String(err));
    }
  }, []);

  const onCreate = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      setError(null);
      const trimmed = newName.trim();
      if (!trimmed) {
        setError(t('dialogs.colorLabels.nameRequired'));
        return;
      }
      setSubmitting(true);
      try {
        await createColorLabel({ name: trimmed, hex: newHex });
        await refreshColorLabels();
        announce(t('dialogs.colorLabels.created', { name: trimmed }));
        setNewName('');
        setNewHex(DEFAULT_NEW_HEX);
      } catch (err) {
        reportError(err);
      } finally {
        setSubmitting(false);
      }
    },
    [newName, newHex, refreshColorLabels, announce, reportError, t],
  );

  const onUpdate = useCallback(
    async (label: ColorLabel) => {
      setError(null);
      try {
        await updateColorLabel(label);
        await refreshColorLabels();
        announce(t('dialogs.colorLabels.updated', { name: label.name }));
      } catch (err) {
        reportError(err);
      }
    },
    [refreshColorLabels, announce, reportError, t],
  );

  const onDelete = useCallback(
    async (label: ColorLabel) => {
      setError(null);
      try {
        await deleteColorLabel(label.id);
        await refreshColorLabels();
        announce(t('dialogs.colorLabels.deleted', { name: label.name }));
      } catch (err) {
        reportError(err);
      }
    },
    [refreshColorLabels, announce, reportError, t],
  );

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.colorLabels.title')}
      className="modal--form"
    >
      <div className="form">
        <ul role="list" className="color-labels">
          {colorLabels.length === 0 && (
            <li role="presentation" className="color-labels__empty">
              {t('dialogs.colorLabels.empty')}
            </li>
          )}
          {colorLabels.map((label) => (
            <LabelRow
              key={label.id}
              label={label}
              onUpdate={onUpdate}
              onDelete={onDelete}
            />
          ))}
        </ul>

        <form onSubmit={onCreate} className="color-labels__create">
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.colorLabels.fields.name')}
            </span>
            <input
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              autoComplete="off"
              required
            />
          </label>
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.colorLabels.fields.color')}
            </span>
            <input
              type="color"
              value={newHex}
              onChange={(e) => setNewHex(e.target.value)}
              aria-label={t('dialogs.colorLabels.fields.color')}
            />
          </label>
          <button
            type="submit"
            disabled={submitting}
            className="form__action form__action--primary"
          >
            {t('dialogs.colorLabels.create')}
          </button>
        </form>

        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}

        <div className="form__actions">
          <button
            type="button"
            onClick={onClose}
            className="form__action"
          >
            {t('dialogs.close')}
          </button>
        </div>
      </div>
    </Modal>
  );
}

interface LabelRowProps {
  label: ColorLabel;
  onUpdate: (label: ColorLabel) => Promise<void>;
  onDelete: (label: ColorLabel) => Promise<void>;
}

function LabelRow({ label, onUpdate, onDelete }: LabelRowProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(label.name);
  const [hex, setHex] = useState(label.hex);

  const dirty = name !== label.name || hex !== label.hex;

  return (
    <li role="listitem" className="color-labels__row">
      <span
        className="color-labels__swatch"
        aria-hidden="true"
        style={{ background: hex }}
      />
      <input
        type="text"
        value={name}
        onChange={(e) => setName(e.target.value)}
        aria-label={t('dialogs.colorLabels.renameLabel', { name: label.name })}
      />
      <input
        type="color"
        value={hex}
        onChange={(e) => setHex(e.target.value)}
        aria-label={t('dialogs.colorLabels.colorLabel', { name: label.name })}
      />
      <button
        type="button"
        disabled={!dirty}
        onClick={() => onUpdate({ ...label, name: name.trim(), hex })}
        className="form__action"
      >
        {t('dialogs.save')}
      </button>
      <button
        type="button"
        onClick={() => onDelete(label)}
        aria-label={t('dialogs.colorLabels.deleteLabel', { name: label.name })}
        className="form__action form__action--danger color-labels__delete"
      >
        ✕
      </button>
    </li>
  );
}
