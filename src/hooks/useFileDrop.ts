import { useEffect } from 'react';

import { useDialogState } from '../state/dialogStateContext';
import { useFileImport } from '../state/useFileImport';

/**
 * A calendar file dragged from the Explorer or Finder onto the window opens it
 * for an import (DESIGN-file-import.md). No file association: only a drop
 * onto Aperio while it runs.
 *
 * Tauri's own drop handling is off (`dragDropEnabled: false`, so the views'
 * HTML drag and drop works on Windows), and the drop arrives as an HTML drop:
 * a `File` with its name and bytes, no path. Only a drag that carries files
 * is taken; moving an appointment or a task between views carries none. While
 * a dialog is open, nothing is taken.
 */
export function useFileDrop(): void {
  const { mode } = useDialogState();
  const { importDropped } = useFileImport();
  const idle = mode.kind === 'none';

  useEffect(() => {
    if (!idle) return;
    const carriesFiles = (e: DragEvent) =>
      Array.from(e.dataTransfer?.types ?? []).includes('Files');
    const onDragOver = (e: DragEvent) => {
      if (!carriesFiles(e)) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
    };
    const onDrop = (e: DragEvent) => {
      if (!carriesFiles(e)) return;
      e.preventDefault();
      const file = e.dataTransfer?.files[0];
      if (file) void importDropped(file);
    };
    window.addEventListener('dragover', onDragOver);
    window.addEventListener('drop', onDrop);
    return () => {
      window.removeEventListener('dragover', onDragOver);
      window.removeEventListener('drop', onDrop);
    };
  }, [idle, importDropped]);
}
