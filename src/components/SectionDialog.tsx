import { useCallback, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  createSection,
  isCommandError,
  setSectionColor,
  updateSection,
} from '../api/client';
import type { Section } from '../api/types';
import { useCalendarStore } from '../state/calendarStoreContext';
import { ColorLabelSelect } from './ColorLabelSelect';
import { Modal } from './Modal';

/**
 * Create / rename a task section (DESIGN §9.7).
 *
 * `section === null` ⇒ create a new section in `listId`; otherwise rename
 * (and optionally recolor) the given section. Shared by the task-view
 * section-header menu and the sidebar list menu so both surfaces reach one
 * accessible name editor — previously section management lived only behind
 * the task editor's Section field, which wasn't discoverable.
 *
 * The colour is written separately via `set_section_color` (host-routed:
 * local sections store it on their synced row, external ones as a local
 * override) so it works for Todoist / Vikunja sections too, mirroring the
 * task editor's inline section editor.
 */
export interface SectionDialogProps {
  isOpen: boolean;
  onClose: () => void;
  listId: string;
  section: Section | null;
}

export function SectionDialog({
  isOpen,
  onClose,
  listId,
  section,
}: SectionDialogProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const { colorLabels, sectionsByList, loadSections } = useCalendarStore();
  const nameId = useId();

  const isRename = section !== null;
  const [name, setName] = useState(section?.name ?? '');
  const [colorDraft, setColorDraft] = useState<string | null>(
    section?.color_label ?? null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Re-seed whenever the target changes or the dialog reopens.
  useEffect(() => {
    if (!isOpen) return;
    setName(section?.name ?? '');
    setColorDraft(section?.color_label ?? null);
    setError(null);
  }, [isOpen, section]);

  const submit = useCallback(async () => {
    const trimmed = name.trim();
    if (!trimmed || busy) return;
    setBusy(true);
    setError(null);
    try {
      if (section) {
        await updateSection({ ...section, name: trimmed });
        if (colorDraft !== (section.color_label ?? null)) {
          await setSectionColor(section.id, listId, colorDraft);
        }
        await loadSections(listId);
        announce(t('dialogs.task.section.renamed', { name: trimmed }));
      } else {
        const created = await createSection({
          list_id: listId,
          name: trimmed,
          // Append to the end — the new section sorts after existing ones.
          position: (sectionsByList[listId] ?? []).length,
        });
        if (colorDraft) {
          await setSectionColor(created.id, listId, colorDraft);
        }
        await loadSections(listId);
        announce(t('dialogs.task.section.created', { name: trimmed }));
      }
      // Reset the draft before closing so a reused dialog instance never
      // flashes stale input (matches TaskDialog's submitSection); harmless
      // even though the modal unmounts on close.
      setName(section?.name ?? '');
      setColorDraft(section?.color_label ?? null);
      onClose();
    } catch (err) {
      if (isCommandError(err)) {
        setError(`${err.code}: ${err.message}`);
      } else {
        setError(String(err));
      }
    } finally {
      setBusy(false);
    }
  }, [
    name,
    busy,
    section,
    colorDraft,
    listId,
    sectionsByList,
    loadSections,
    announce,
    t,
    onClose,
  ]);

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={
        isRename
          ? t('dialogs.task.section.renameTitle')
          : t('dialogs.task.section.createTitle')
      }
      className="modal--form"
      dismissOnBackdrop={false}
    >
      {error && (
        <p role="alert" className="form__error">
          {error}
        </p>
      )}
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <label className="form__field" htmlFor={nameId}>
          <span className="form__label">
            {t('dialogs.task.section.nameField')}
          </span>
          <input
            id={nameId}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={t('dialogs.task.section.namePlaceholder')}
            aria-required="true"
            autoFocus
            disabled={busy}
          />
        </label>
        <ColorLabelSelect
          value={colorDraft}
          onChange={setColorDraft}
          labels={colorLabels}
          noneLabel={t('dialogs.task.section.noColor')}
          ariaLabel={t('dialogs.task.section.colorLabel')}
        />
        <div className="form__actions">
          <button
            type="button"
            className="form__action form__action--secondary"
            onClick={onClose}
            disabled={busy}
          >
            {t('dialogs.task.section.cancel')}
          </button>
          <button
            type="submit"
            className="form__action"
            disabled={busy || !name.trim()}
          >
            {isRename
              ? t('dialogs.task.section.save')
              : t('dialogs.task.section.create')}
          </button>
        </div>
      </form>
    </Modal>
  );
}
