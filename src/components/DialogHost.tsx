import { useDialogState } from '../state/DialogState';
import { ColorLabelDialog } from './ColorLabelDialog';
import { EventDialog } from './EventDialog';
import { QuickAddDialog } from './QuickAddDialog';
import { TaskDialog } from './TaskDialog';

/**
 * Single host component that renders whichever dialog is currently
 * active. Sits at the app root so dialogs survive view switches and
 * always portal into the same place.
 */
export function DialogHost() {
  const { mode, close } = useDialogState();

  switch (mode.kind) {
    case 'event':
      return (
        <EventDialog
          isOpen
          onClose={close}
          event={mode.event}
          defaultCalendarId={mode.calendarId}
        />
      );
    case 'task':
      return (
        <TaskDialog
          isOpen
          onClose={close}
          task={mode.task}
          defaultListId={mode.listId}
        />
      );
    case 'quickAdd':
      return <QuickAddDialog isOpen onClose={close} />;
    case 'colorLabels':
      return <ColorLabelDialog isOpen onClose={close} />;
    case 'none':
    default:
      return null;
  }
}
