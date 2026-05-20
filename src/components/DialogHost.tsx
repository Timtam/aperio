import { useDialogState } from '../state/DialogState';
import { ColorLabelDialog } from './ColorLabelDialog';
import { EventDialog } from './EventDialog';
import { MoveCopyDialog } from './MoveCopyDialog';
import { QuickAddDialog } from './QuickAddDialog';
import { SearchDialog } from './SearchDialog';
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
    case 'colorLabels':
      return <ColorLabelDialog isOpen onClose={close} />;
    case 'search':
      return <SearchDialog isOpen onClose={close} />;
    case 'moveCopy':
      return (
        <MoveCopyDialog isOpen onClose={close} target={mode.target} />
      );
    case 'none':
    default:
      return null;
  }
}
