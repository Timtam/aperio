import { useId, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { TaskList } from '../api/types';
import { useBacklogLists } from '../state/useBacklogLists';
import { CheckList } from './CheckList';
import { Modal } from './Modal';

/**
 * Which task lists the backlog shows — a filter of its own, separate from the
 * sidebar's.
 *
 * The sidebar switch takes a list out of EVERYTHING: the calendar days, the
 * pickers, the backlog. That is the wrong granularity for a long household
 * list, whose dated tasks are still wanted on the days they fall on while its
 * hundred undated ones swamp the planning column. Users were keeping the
 * sidebar permanently open just to flip lists in and out — spending the width
 * the backlog needed in the first place.
 *
 * Only lists the sidebar currently shows are offered here: a list switched off
 * globally is not fetched at all, so ticking it would promise something this
 * filter cannot deliver.
 *
 * Every tick applies immediately and is stored per DEVICE, so the dialog's one
 * button is a close, not a save.
 */
export function BacklogListsDialog({
  isOpen,
  onClose,
  taskLists,
}: {
  isOpen: boolean;
  onClose: () => void;
  /** The lists the sidebar currently shows, in its own order. */
  taskLists: readonly TaskList[];
}) {
  const { t } = useTranslation();
  const { shows, setShown, showAll } = useBacklogLists();
  const introId = useId();
  const introRef = useRef<HTMLParagraphElement | null>(null);

  const hiddenCount = taskLists.filter((l) => !shows(l.id)).length;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('views.backlog.listsTitle')}
      className="modal--form modal--narrow"
      initialFocusRef={introRef}
      describedById={introId}
    >
      <p id={introId} ref={introRef} tabIndex={-1} className="form__hint">
        {t('views.backlog.listsIntro')}
      </p>

      {taskLists.length === 0 ? (
        <p className="form__hint">{t('views.backlog.listsEmpty')}</p>
      ) : (
        <CheckList
          items={taskLists.map((l) => ({
            id: l.id,
            name: l.name,
            checked: shows(l.id),
          }))}
          onToggle={(item) => setShown(item.id, !item.checked)}
        />
      )}

      <div className="modal__actions">
        {hiddenCount > 0 && (
          <button type="button" className="form__action" onClick={showAll}>
            {t('views.backlog.listsShowAll')}
          </button>
        )}
        <button type="button" className="form__action" onClick={onClose}>
          {t('dialogs.close')}
        </button>
      </div>
    </Modal>
  );
}
