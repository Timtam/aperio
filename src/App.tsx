import { useTranslation } from 'react-i18next';

import { AnnouncerProvider } from './a11y/Announcer';
import { Sidebar } from './components/Sidebar';
import { TitleBar } from './components/TitleBar';
import { Toolbar } from './components/Toolbar';
import { AgendaView } from './components/views/AgendaView';
import { DayView } from './components/views/DayView';
import { MonthView } from './components/views/MonthView';
import { TaskView } from './components/views/TaskView';
import { WeekView } from './components/views/WeekView';
import { YearView } from './components/views/YearView';
import { useRegionFocus } from './hooks/useRegionFocus';
import { useSuppressBrowserDefaults } from './hooks/useSuppressBrowserDefaults';
import { CalendarStoreProvider } from './state/CalendarStore';
import { ViewStateProvider, useViewShortcuts, useViewState } from './state/ViewState';

/**
 * Root component.
 *
 * `role="application"` lives on the outermost wrapper, *not* inside the
 * Shell, so every descendant — including the announcer's `aria-live`
 * regions — sits inside the application boundary. If the live regions
 * were rendered as siblings of `role="application"`, NVDA would drop
 * out of focus mode whenever an announcement landed, defeating the
 * point of the role (DESIGN.md section 3.2.1).
 *
 * Provider order:
 *  1. AnnouncerProvider — every child can call `useAnnouncer()`.
 *  2. CalendarStoreProvider — owns the calendars/task-lists registry
 *     plus their selection state.
 *  3. ViewStateProvider — owns the active view + anchor date.
 *
 * `useSuppressBrowserDefaults` and `useRegionFocus` are pure window
 * listeners, so they live at the root.
 */
export function App() {
  useSuppressBrowserDefaults();
  useRegionFocus();

  return (
    <div
      id="app-root"
      role="application"
      aria-label="Aperio"
      className="app-root"
    >
      <AnnouncerProvider>
        <CalendarStoreProvider>
          <ViewStateProvider>
            <Shell />
          </ViewStateProvider>
        </CalendarStoreProvider>
      </AnnouncerProvider>
    </div>
  );
}

function Shell() {
  useViewShortcuts();

  return (
    <>
      <TitleBar />
      <div className="app-body">
        <Sidebar />
        <div className="app-main" data-region="main">
          <Toolbar />
          <ActiveView />
        </div>
      </div>
    </>
  );
}

function ActiveView() {
  const { view } = useViewState();
  const { t } = useTranslation();

  switch (view) {
    case 'day':
      return <DayView />;
    case 'week':
      return <WeekView />;
    case 'month':
      return <MonthView />;
    case 'year':
      return <YearView />;
    case 'agenda':
      return <AgendaView />;
    case 'tasks':
      return <TaskView />;
    default:
      return <p role="alert">{t('app.unknownView', { view })}</p>;
  }
}
