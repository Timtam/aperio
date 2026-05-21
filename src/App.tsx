import { useTranslation } from 'react-i18next';

import { AnnouncerProvider } from './a11y/Announcer';
import { DialogHost } from './components/DialogHost';
import { FocusBar } from './components/FocusBar';
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
import { DialogStateProvider, useDialogState } from './state/DialogState';
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
  const { mode } = useDialogState();

  // While a dialog is open the rest of the app is taken out of the
  // accessibility tree. `aria-modal` on the dialog alone is not enough
  // — NVDA's heuristics will still let focus on a form control inside
  // the dialog fall back to browse mode if it can see content outside.
  // Hiding the shell with `aria-hidden` plus `inert` gives the screen
  // reader a hard signal that only the dialog subtree is live.
  const inert = mode.kind !== 'none';
  const inertProps = inert ? ({ inert: '' } as { inert: '' }) : {};

  return (
    <div
      className="app-shell"
      aria-hidden={inert || undefined}
      {...inertProps}
    >
      <TitleBar />
      <div className="app-body">
        <Sidebar />
        <div className="app-main" data-region="main">
          <Toolbar />
          <FocusBar />
          {/* Programmatic focus target for "land here after a
              transient mode exits" (e.g. closing the focus banner).
              tabIndex=-1 keeps it out of normal Tab order but lets
              .focus() succeed; views inside can still own their own
              tab stops. */}
          <div
            className="app-active-view"
            data-active-view-root
            tabIndex={-1}
          >
            <ActiveView />
          </div>
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
