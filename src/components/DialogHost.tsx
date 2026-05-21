import { useDialogState } from '../state/DialogState';
import { DayStartReviewDialog } from './DayStartReviewDialog';
import { EventDialog } from './EventDialog';
import { MoveCopyDialog } from './MoveCopyDialog';
import { PlanTaskDialog } from './PlanTaskDialog';
import { QuickAddDialog } from './QuickAddDialog';
import { RemindersDialog } from './RemindersDialog';
import { SearchDialog } from './SearchDialog';
import { SettingsDialog } from './SettingsDialog';
import { TaskDialog } from './TaskDialog';

/**
 * Single host component that renders whichever dialog is currently
 * active. Sits at the app root so dialogs survive view switches and
 * always portal into the same place.
 */
export function DialogHost() {
  const { mode, close, invalidateData } = useDialogState();

  switch (mode.kind) {
    case 'event':
      return (
        <EventDialog
          isOpen
          onClose={close}
          event={mode.event}
          defaultCalendarId={mode.calendarId}
          defaultDate={mode.defaultDate}
        />
      );
    case 'task':
      return (
        <TaskDialog
          isOpen
          onClose={close}
          task={mode.task}
          defaultListId={mode.listId}
          defaultDate={mode.defaultDate}
        />
      );
    case 'quickAdd':
      return <QuickAddDialog isOpen onClose={close} />;
    case 'settings':
      return (
        <SettingsDialog
          isOpen
          onClose={close}
          initialTab={mode.initialTab}
        />
      );
    case 'search':
      return <SearchDialog isOpen onClose={close} />;
    case 'reminders':
      return <RemindersDialog isOpen onClose={close} />;
    case 'moveCopy':
      return (
        <MoveCopyDialog isOpen onClose={close} target={mode.target} />
      );
    case 'planTask':
      return (
        <PlanTaskDialog
          isOpen
          onClose={close}
          task={mode.task}
          onPlanned={invalidateData}
        />
      );
    case 'dayStartReview':
      return <DayStartReviewDialog isOpen onClose={close} />;
    case 'none':
    default:
      return null;
  }
}
