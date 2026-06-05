import { useTranslation } from 'react-i18next';

import { AnnouncerProvider } from './a11y/Announcer';
import { DayStartReviewChecker } from './components/DayStartReviewChecker';
import { DeadlinePinChecker } from './components/DeadlinePinChecker';
import { DialogHost } from './components/DialogHost';
import { FocusBar } from './components/FocusBar';
import { Sidebar } from './components/Sidebar';
import { TitleBar } from './components/TitleBar';
import { Toolbar } from './components/Toolbar';
import { AgendaView } from './components/views/AgendaView';
import { ContactsView } from './components/views/ContactsView';
import { DayView } from './components/views/DayView';
import { MonthView } from './components/views/MonthView';
import { TaskView } from './components/views/TaskView';
import { WeekView } from './components/views/WeekView';
import { YearView } from './components/views/YearView';
import { useDialogShortcuts } from './hooks/useDialogShortcuts';
import { useRegionFocus } from './hooks/useRegionFocus';
import { useSuppressBrowserDefaults } from './hooks/useSuppressBrowserDefaults';
import { useStoredLanguage } from './intl/useStoredLanguage';
import { CacheSyncListener } from './state/CacheSyncListener';
import { CalendarStoreProvider } from './state/CalendarStore';
import { DialogStateProvider } from './state/DialogState';
import { useDialogState } from './state/dialogStateContext';
import { TaskCascadeProvider } from './state/TaskCascadeProvider';
import { ToastProvider } from './state/ToastProvider';
import { ViewStateProvider } from './state/ViewState';
import { useViewShortcuts, useViewState } from './state/viewStateContext';

/**
 * Root component.
 *
 * Provider order:
 *  1. AnnouncerProvider
 *  2. CalendarStoreProvider — owns calendars/task-lists + selection.
 *  3. ViewStateProvider — owns active view + anchor date.
 *  4. DialogStateProvider — owns which dialog (if any) is open.
 *  5. TaskCascadeProvider — owns the parent/subtask status-coupling
 *     preference. Sits inside the dialog provider so the Settings
 *     panel can read it via context.
 *  6. ToastProvider — owns the bottom-right toast stack (currently
 *     the Undo handle for silent carry-over batches). Sits inside
 *     TaskCascadeProvider so the DayStartReviewChecker can pull
 *     both via context, and renders its toast stack alongside
 *     DialogHost (outside the inert shell).
 *
 * `role="application"` stays on the outermost wrapper so portaled
 * dialogs and announcer live regions all sit inside the application
 * boundary.
 */
export function App() {
  useSuppressBrowserDefaults();
  useRegionFocus();
  // Apply the persisted/synced language choice on start (defaults to the
  // system language, set synchronously by i18n init).
  useStoredLanguage();

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
              <TaskCascadeProvider>
                <ToastProvider>
                  <Shell />
                  <DialogHost />
                  {/* Mount-once gates that run the day-start review
                      flows. Both render nothing themselves.

                      Order matters:
                        - DayStartReviewChecker mounts first. It runs
                          the silent carry-over batch (if the user
                          opted into auto-today / auto-backlog) and/or
                          opens the unified review dialog when there
                          are overdue deadlines or slipped schedules
                          left to discuss.
                        - DeadlinePinChecker mounts LAST so it has the
                          final write. It silently pins every task with
                          `deadline_date == today` to today, taking
                          precedence over whatever carry-over wrote a
                          moment earlier. */}
                  <DayStartReviewChecker />
                  <DeadlinePinChecker />
                  {/* Bridges backend `cache-updated` pushes to data
                      invalidation so external views refresh when a
                      background snapshot refresh lands. Renders nothing. */}
                  <CacheSyncListener />
                </ToastProvider>
              </TaskCascadeProvider>
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
        <div className="app-main">
          <Toolbar />
          <FocusBar />
          {/* Programmatic focus target for "land here after a
              transient mode exits" (e.g. closing the focus banner)
              AND the third stop in the F6 region cycle (sidebar →
              toolbar → view → sidebar). tabIndex=-1 keeps it out of
              normal Tab order but lets .focus() succeed; the view
              inside owns its own tab stops, and useRegionFocus will
              prefer the first natively-focusable descendant before
              falling back to this wrapper. */}
          <div
            className="app-active-view"
            data-region="view"
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
    case 'contacts':
      return <ContactsView />;
    default:
      return <p role="alert">{t('app.unknownView', { view })}</p>;
  }
}
