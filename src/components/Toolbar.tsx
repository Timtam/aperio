import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { localDateKey } from '../intl/dateKey';
import { useDateFormat } from '../intl/dateFormat';
import { useDialogState } from '../state/dialogStateContext';
import { useTaskCascadeEnabled } from '../state/taskCascadeContext';
import { useViewState } from '../state/viewStateContext';
import { VIEWS, type ViewId } from '../state/viewMath';
import { CacheRefreshIndicator } from './CacheRefreshIndicator';
import { SyncStatusIndicator } from './SyncStatusIndicator';

const VIEW_SHORTCUT: Record<ViewId, string> = {
  day: '1',
  week: '2',
  month: '3',
  year: '4',
  agenda: '5',
  tasks: '6',
  contacts: '7',
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
  const announce = useAnnouncer();
  const { view, setView, anchor, jumpToToday, goPrev, goNext } = useViewState();
  const { openQuickAdd, openQuickAddTask, openSearch } = useDialogState();
  const { dayViewMode, setDayViewMode } = useTaskCascadeEnabled();

  // The grid↔list layout toggle is only meaningful on the day + week
  // surfaces (the only views that render the hour-grid / list). It hides
  // entirely elsewhere so it doesn't add noise to month / year / tasks.
  const showDayViewToggle = view === 'day' || view === 'week';

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

      {showDayViewToggle && (
        <div
          role="group"
          aria-label={t('toolbar.dayViewMode.label')}
          className="toolbar__group toolbar__group--day-view"
        >
          <button
            type="button"
            className="toolbar__view-btn"
            aria-pressed={dayViewMode === 'grid'}
            onClick={() => {
              // No-op re-click of the already-active mode stays silent — the
              // setter + announce only fire on an actual change. The live
              // region speaks the RESULT state ("Hour grid layout"), not the
              // imperative toGrid string (which reads as an instruction).
              if (dayViewMode !== 'grid') {
                setDayViewMode('grid');
                announce(t('toolbar.dayViewMode.announceGrid'));
              }
            }}
            title={t('toolbar.dayViewMode.toGrid')}
          >
            {t('toolbar.dayViewMode.grid')}
          </button>
          <button
            type="button"
            className="toolbar__view-btn"
            aria-pressed={dayViewMode === 'list'}
            onClick={() => {
              // See the grid button: silent on a no-op re-click; the live
              // region speaks the RESULT state ("Compact list layout").
              if (dayViewMode !== 'list') {
                setDayViewMode('list');
                announce(t('toolbar.dayViewMode.announceList'));
              }
            }}
            title={t('toolbar.dayViewMode.toList')}
          >
            {t('toolbar.dayViewMode.list')}
          </button>
        </div>
      )}

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
          onClick={() => openSearch()}
          aria-label={t('toolbar.search')}
          title={t('toolbar.search') + ' (Ctrl+F)'}
        >
          🔎
        </button>
        {/* One create button per kind. Each opens the quick-add (the "schnell
            anlegen" editor), which expands to the full editor via "weitere
            Details" — so the old standalone quick-add buttons are gone. The
            full editor stays one keystroke away (Ctrl+Shift+N / Alt+Shift+N). */}
        <button
          type="button"
          onClick={() => openQuickAdd({ defaultDate: localDateKey(anchor) })}
          title={t('toolbar.newEvent') + ' (Ctrl+N)'}
        >
          {t('toolbar.newEvent')}
        </button>
        <button
          type="button"
          onClick={() => openQuickAddTask()}
          title={t('toolbar.newTask') + ' (Alt+N)'}
        >
          {t('toolbar.newTask')}
        </button>
      </div>

      {/* Sync status sits at the right edge of the toolbar so it
          stays visible across views without competing with the
          create/search cluster. §19.9 mandates "permanently visible
          in the status bar"; the toolbar IS the status bar in
          Aperio's layout. */}
      <div className="toolbar__group toolbar__group--status">
        <CacheRefreshIndicator />
        <SyncStatusIndicator />
      </div>
    </div>
  );
}
