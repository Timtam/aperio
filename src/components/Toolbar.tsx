import { useTranslation } from 'react-i18next';

import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/DialogState';
import { useViewState } from '../state/ViewState';
import { VIEWS, type ViewId } from '../state/viewMath';

const VIEW_SHORTCUT: Record<ViewId, string> = {
  day: '1',
  week: '2',
  month: '3',
  year: '4',
  agenda: '5',
  tasks: '6',
};

/**
 * Top toolbar above the main view.
 *
 * Holds two things:
 *  - View-switch buttons (Day / Week / Month / Year / Agenda / Tasks),
 *    mirrored by Ctrl+1..6 keyboard shortcuts wired in `useViewShortcuts`.
 *  - Navigation cluster: Today, Prev period, Next period — also bound
 *    to Ctrl+T and Ctrl+Left/Right respectively.
 *
 * The active view's button carries `aria-pressed="true"` so screen
 * readers announce the current selection. Tooltips and visible labels
 * surface the keyboard shortcut as a hint.
 */
export function Toolbar() {
  const { t } = useTranslation();
  const fmt = useDateFormat();
  const { view, setView, anchor, jumpToToday, goPrev, goNext } = useViewState();
  const { openEventDialog, openTaskDialog, openQuickAdd } = useDialogState();

  return (
    <div
      className="toolbar"
      role="toolbar"
      aria-label={t('toolbar.label')}
      data-region="toolbar"
    >
      <div role="group" aria-label={t('toolbar.viewSwitch')} className="toolbar__group">
        {VIEWS.map((v) => (
          <button
            key={v}
            type="button"
            className="toolbar__view-btn"
            aria-pressed={view === v}
            onClick={() => setView(v)}
            title={`${t(`toolbar.views.${v}`)} (Ctrl+${VIEW_SHORTCUT[v]})`}
          >
            {t(`toolbar.views.${v}`)}
          </button>
        ))}
      </div>

      <div
        role="group"
        aria-label={t('toolbar.navigation')}
        className="toolbar__group toolbar__group--nav"
      >
        <button
          type="button"
          onClick={goPrev}
          aria-label={t('toolbar.prev')}
          title={t('toolbar.prev') + ' (Ctrl+←)'}
        >
          ‹
        </button>
        <button
          type="button"
          onClick={jumpToToday}
          aria-label={t('toolbar.today')}
          title={t('toolbar.today') + ' (Ctrl+T)'}
        >
          {t('toolbar.today')}
        </button>
        <button
          type="button"
          onClick={goNext}
          aria-label={t('toolbar.next')}
          title={t('toolbar.next') + ' (Ctrl+→)'}
        >
          ›
        </button>
      </div>

      <div className="toolbar__anchor" aria-live="off">
        {fmt.format(anchor, 'PPPP')}
      </div>

      <div
        role="group"
        aria-label={t('toolbar.create')}
        className="toolbar__group toolbar__group--create"
      >
        <button
          type="button"
          onClick={() => openQuickAdd()}
          title={t('toolbar.quickAdd') + ' (N)'}
        >
          {t('toolbar.quickAdd')}
        </button>
        <button
          type="button"
          onClick={() => openEventDialog(null)}
          title={t('toolbar.newEvent') + ' (Ctrl+N)'}
        >
          {t('toolbar.newEvent')}
        </button>
        <button
          type="button"
          onClick={() => openTaskDialog(null)}
          title={t('toolbar.newTask') + ' (Ctrl+Shift+N)'}
        >
          {t('toolbar.newTask')}
        </button>
      </div>
    </div>
  );
}
