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
 * The provider order matters:
 *  1. AnnouncerProvider — every child can call `useAnnouncer()`.
 *  2. CalendarStoreProvider — owns the calendars/task-lists registry
 *     plus their selection state. Drives the data hooks.
 *  3. ViewStateProvider — owns the active view + anchor date.
 *
 * `useSuppressBrowserDefaults` and `useRegionFocus` are pure listeners
 * on the window, so they live at the top.
 *
 * DESIGN.md section 3.2.1 requires `role="application"` on the root
 * element — without it screen readers treat the WebView like a web page
 * and intercept keys we need for navigation.
 */
export function App() {
  useSuppressBrowserDefaults();
  useRegionFocus();

  return (
    <AnnouncerProvider>
      <CalendarStoreProvider>
        <ViewStateProvider>
          <Shell />
        </ViewStateProvider>
      </CalendarStoreProvider>
    </AnnouncerProvider>
  );
}

function Shell() {
  useViewShortcuts();

  return (
    <div
      id="app-root"
      role="application"
      aria-label="Aperio"
      className="app-root"
    >
      <TitleBar />
      <div className="app-body">
        <Sidebar />
        <div className="app-main" data-region="main">
          <Toolbar />
          <ActiveView />
        </div>
      </div>
    </div>
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
