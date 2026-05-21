import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createColorLabel,
  deleteColorLabel,
  isCommandError,
  updateColorLabel,
} from '../api/client';
import type { ColorLabel } from '../api/types';
import { useAutoFocus } from '../hooks/useAutoFocus';
import { useCalendarStore } from '../state/CalendarStore';
import { ColorComposer } from './ColorComposer';

/**
 * Color-label management panel (DESIGN.md §8.1). Rendered inside the
 * Settings dialog's `role="tabpanel"`. The modal wrapper used to live
 * here directly; it moved to SettingsDialog so all global config
 * lives behind one entry point and shares the same Escape / focus
 * trap / backdrop semantics.
 *
 * Two-mode layout (unchanged):
 *  - *List mode* (default). Listbox of existing labels — one tab stop,
 *    Arrow keys + aria-activedescendant. Enter opens edit mode.
 *  - *Edit mode*. Form for the focused label: Name + ColorComposer +
 *    Save / Cancel / Delete.
 *
 * Underneath either mode, "Add a new label" stays mounted.
 */

const DEFAULT_NEW_HEX = '#e53935';

export function ColorLabelsPanel() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { colorLabels, refreshColorLabels } = useCalendarStore();

  const [newName, setNewName] = useState('');
  const [newHex, setNewHex] = useState(DEFAULT_NEW_HEX);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);

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
        setEditingId(null);
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
        setEditingId(null);
      } catch (err) {
        reportError(err);
      }
    },
    [refreshColorLabels, announce, reportError, t],
  );

  const editingLabel =
    editingId !== null
      ? colorLabels.find((l) => l.id === editingId) ?? null
      : null;

  return (
    <div className="form">
      {editingLabel ? (
        <EditLabelSection
          label={editingLabel}
          onSave={onUpdate}
          onCancel={() => setEditingId(null)}
          onDelete={onDelete}
        />
      ) : (
        <ExistingSection
          colorLabels={colorLabels}
          onEdit={setEditingId}
        />
      )}

      <section
        role="region"
        tabIndex={0}
        aria-labelledby="color-labels-new"
        className="color-labels__section"
      >
        <h3 id="color-labels-new" className="color-labels__heading">
          {t('dialogs.colorLabels.newHeading')}
        </h3>
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
          <div className="form__field">
            <span className="form__label">
              {t('dialogs.colorLabels.fields.color')}
            </span>
            <ColorComposer
              value={newHex}
              onChange={setNewHex}
              label={t('dialogs.colorLabels.fields.color')}
            />
          </div>
          <button
            type="submit"
            aria-disabled={submitting || undefined}
            className="form__action form__action--primary"
          >
            {t('dialogs.colorLabels.create')}
          </button>
        </form>
      </section>

      {error && (
        <p role="alert" className="form__error">
          {error}
        </p>
      )}
    </div>
  );
}

interface ExistingSectionProps {
  colorLabels: ColorLabel[];
  onEdit: (id: string) => void;
}

/**
 * Listbox of existing labels. One tab stop for the whole list; arrow
 * keys move between items, Enter opens the focused label for editing.
 */
function ExistingSection({ colorLabels, onEdit }: ExistingSectionProps) {
  const { t } = useTranslation();
  const [focusIndex, setFocusIndex] = useState(0);
  const idPrefix = useId();
  const optionId = (i: number) => `${idPrefix}-option-${i}`;

  // Clamp focus when the list shrinks (e.g. after a delete).
  useEffect(() => {
    if (focusIndex >= colorLabels.length) {
      setFocusIndex(Math.max(0, colorLabels.length - 1));
    }
  }, [colorLabels.length, focusIndex]);

  // Focus the listbox (or its section as fallback for an empty list)
  // on mount. This fires both on initial open and on every return from
  // edit mode. At initial open Modal's own focus effect runs after this
  // one and overrides it — the section ends up focused so NVDA hears
  // the heading + description. On subsequent mounts (edit → list)
  // Modal does not re-fire, so the listbox keeps the focus and the
  // user can immediately resume navigating with arrows.
  const sectionRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  useEffect(() => {
    (listRef.current ?? sectionRef.current)?.focus({ preventScroll: true });
  }, []);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (colorLabels.length === 0) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setFocusIndex((i) => Math.min(i + 1, colorLabels.length - 1));
        return;
      case 'ArrowUp':
        e.preventDefault();
        setFocusIndex((i) => Math.max(i - 1, 0));
        return;
      case 'Home':
        e.preventDefault();
        setFocusIndex(0);
        return;
      case 'End':
        e.preventDefault();
        setFocusIndex(colorLabels.length - 1);
        return;
      case 'Enter':
      case ' ':
      case 'Spacebar': {
        e.preventDefault();
        const label = colorLabels[focusIndex];
        if (label) onEdit(label.id);
        return;
      }
      default:
        return;
    }
  };

  // When the list has entries the <ul> carries the tab stop and
  // announces "listbox" plus the active option. When the list is empty
  // the section itself takes the stop so the screen reader can still
  // announce the heading + description. tabIndex=-1 keeps the section
  // programmatically focusable as a fallback target in either case.
  const sectionTabIndex = colorLabels.length === 0 ? 0 : -1;

  return (
    <section
      ref={sectionRef}
      role="region"
      tabIndex={sectionTabIndex}
      aria-labelledby="color-labels-existing"
      aria-describedby={
        colorLabels.length === 0 ? 'color-labels-empty' : undefined
      }
      className="color-labels__section"
    >
      <h3 id="color-labels-existing" className="color-labels__heading">
        {t('dialogs.colorLabels.existingHeading', {
          count: colorLabels.length,
        })}
      </h3>
      {colorLabels.length === 0 ? (
        <p id="color-labels-empty" className="color-labels__empty">
          {t('dialogs.colorLabels.emptyHint')}
        </p>
      ) : (
        <ul
          ref={listRef}
          role="listbox"
          tabIndex={0}
          aria-label={t('dialogs.colorLabels.listLabel')}
          aria-activedescendant={optionId(focusIndex)}
          onKeyDown={handleKeyDown}
          className="color-labels"
        >
          {colorLabels.map((label, i) => {
            const focused = i === focusIndex;
            return (
              <li
                key={label.id}
                id={optionId(i)}
                role="option"
                aria-selected={focused}
                aria-label={t('dialogs.colorLabels.optionLabel', {
                  name: label.name,
                  hex: label.hex,
                })}
                className={
                  'color-labels__row' +
                  (focused ? ' color-labels__row--focused' : '')
                }
                onClick={() => {
                  setFocusIndex(i);
                  onEdit(label.id);
                }}
              >
                <span
                  className="color-labels__swatch"
                  aria-hidden="true"
                  style={{ background: label.hex }}
                />
                <span className="color-labels__name">{label.name}</span>
                <span className="color-labels__hex" aria-hidden="true">
                  {label.hex}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

interface EditLabelSectionProps {
  label: ColorLabel;
  onSave: (label: ColorLabel) => void | Promise<void>;
  onCancel: () => void;
  onDelete: (label: ColorLabel) => void | Promise<void>;
}

function EditLabelSection({
  label,
  onSave,
  onCancel,
  onDelete,
}: EditLabelSectionProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(label.name);
  const [hex, setHex] = useState(label.hex);
  // Autofocus on mount so opening an edit (via Enter on a listbox row)
  // lands keyboard focus right on the name input.
  const nameRef = useAutoFocus<HTMLInputElement>();

  // Sync from outside when the parent picks a different label.
  useEffect(() => {
    setName(label.name);
    setHex(label.hex);
  }, [label.id, label.name, label.hex]);

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    onSave({ ...label, name: name.trim(), hex });
  };

  return (
    <section
      role="region"
      aria-labelledby="color-labels-edit"
      className="color-labels__section"
    >
      <h3 id="color-labels-edit" className="color-labels__heading">
        {t('dialogs.colorLabels.editHeading', { name: label.name })}
      </h3>
      <form onSubmit={onSubmit} className="color-labels__edit">
        <label className="form__field">
          <span className="form__label">
            {t('dialogs.colorLabels.fields.name')}
          </span>
          <input
            ref={nameRef}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            autoComplete="off"
            required
          />
        </label>
        <div className="form__field">
          <span className="form__label">
            {t('dialogs.colorLabels.fields.color')}
          </span>
          <ColorComposer
            value={hex}
            onChange={setHex}
            label={t('dialogs.colorLabels.fields.color')}
          />
        </div>
        <div className="form__actions">
          <button
            type="button"
            onClick={() => onDelete(label)}
            className="form__action form__action--danger"
          >
            {t('dialogs.colorLabels.deleteAction')}
          </button>
          <button
            type="button"
            onClick={onCancel}
            className="form__action"
          >
            {t('dialogs.cancel')}
          </button>
          <button
            type="submit"
            className="form__action form__action--primary"
          >
            {t('dialogs.save')}
          </button>
        </div>
      </form>
    </section>
  );
}
