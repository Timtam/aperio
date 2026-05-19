import { useTranslation } from 'react-i18next';

import { AnnouncerProvider } from './a11y/Announcer';
import { DialogHost } from './components/DialogHost';
import { Sidebar } from './components/Sidebar';
import { TitleBar } from './components/TitleBar';
import { Toolbar } from './components/Toolbar';
import { AgendaView } from './components/views/AgendaView';
import { DayView } from './components/views/DayView';
import { MonthView } from './components/views/MonthView';
import { TaskView } from './components/views/TaskView';
import { WeekView } from './components/views/WeekView';
import { YearView } from './components/views/YearView';
import { useDialogShortcuts } from './hooks/useDialogShortcuts';
import { useRegionFocus } from './hooks/useRegionFocus';
import { useSuppressBrowserDefaults } from './hooks/useSuppressBrowserDefaults';
import { CalendarStoreProvider } from './state/CalendarStore';
import { DialogStateProvider } from './state/DialogState';
import { ViewStateProvider, useViewShortcuts, useViewState } from './state/ViewState';

/**
 * Root component.
 *
 * Provider order:
 *  1. AnnouncerProvider
 *  2. CalendarStoreProvider — owns calendars/task-lists + selection.
 *  3. ViewStateProvider — owns active view + anchor date.
 *  4. DialogStateProvider — owns which dialog (if any) is open.
 *
 * `role="application"` stays on the outermost wrapper so portaled
 * dialogs and announcer live regions all sit inside the application
 * boundary.
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
            <DialogStateProvider>
              <Shell />
              <DialogHost />
            </DialogStateProvider>
          </ViewStateProvider>
        </CalendarStoreProvider>
      </AnnouncerProvider>
    </div>
  );
}

function Shell() {
  useViewShortcuts();
  useDialogShortcuts();

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
