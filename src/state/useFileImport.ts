import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { importedForEditor, skippedSentence } from '@aperio/shared';

import { fileImportFormats, readImportContent, readImportFile } from '../api/client';
import type { FileImportOutcome } from './DialogState';
import { useDialogState } from './dialogStateContext';

/**
 * Bring a calendar file in (DESIGN-file-import.md): from the open dialog
 * (the toolbar's "Import …", Ctrl+O) or dropped on the window.
 *
 * What a plugin read decides what opens: one appointment fills the event
 * editor, as it stands, with the import's note as its first stop; more than
 * one, none, or a failure open the import dialog.
 */
export function useFileImport(): {
  importFromDisk: () => Promise<void>;
  importDropped: (file: File) => Promise<void>;
} {
  const { t } = useTranslation();
  const { openEventDialog, openFileImport } = useDialogState();

  const present = useCallback(
    (fileName: string, outcome: FileImportOutcome) => {
      if ('read' in outcome && outcome.read.events.length === 1) {
        const { event, invitationSetAside } = importedForEditor(outcome.read.events[0]);
        const note = [
          skippedSentence(outcome.read.skipped, t),
          invitationSetAside ? t('fileImport.invitationSetAside') : null,
        ]
          .filter(Boolean)
          .join(' ');
        openEventDialog(null, { importFrom: event, importNote: note || null });
        return;
      }
      if ('failed' in outcome) {
        // The sentence names the kind of failure; the plugin's own words, a
        // parser's English, belong in the log.
        // eslint-disable-next-line no-console
        console.warn('reading a file for an import failed', outcome.failed);
      }
      openFileImport(fileName, outcome);
    },
    [t, openEventDialog, openFileImport],
  );

  const importFromDisk = useCallback(async () => {
    let path: string | string[] | null;
    try {
      const formats = await fileImportFormats();
      const extensions = [...new Set(formats.flatMap((format) => format.extensions))];
      path = await openFileDialog({
        multiple: false,
        directory: false,
        title: t('fileImport.dialogTitle'),
        filters:
          extensions.length === 0
            ? []
            : [
                { name: t('fileImport.filterAll'), extensions },
                ...formats.map((format) => ({
                  name: format.name,
                  extensions: format.extensions,
                })),
              ],
      });
    } catch (err) {
      present('', { failed: err });
      return;
    }
    if (typeof path !== 'string') return; // cancelled
    const fileName = path.split(/[\\/]/).pop() ?? path;
    try {
      present(fileName, { read: await readImportFile(path) });
    } catch (err) {
      present(fileName, { failed: err });
    }
  }, [t, present]);

  const importDropped = useCallback(
    async (file: File) => {
      try {
        const content = toBase64(await readBytes(file));
        present(file.name, { read: await readImportContent(file.name, content) });
      } catch (err) {
        present(file.name, { failed: err });
      }
    },
    [present],
  );

  return { importFromDisk, importDropped };
}

/** A dropped file's bytes. `FileReader` rather than `Blob.arrayBuffer()`,
 *  which not every engine the tests run in has. */
function readBytes(file: Blob): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(new Uint8Array(reader.result as ArrayBuffer));
    reader.onerror = () => reject(reader.error);
    reader.readAsArrayBuffer(file);
  });
}

/** Bytes as base64, in chunks: `String.fromCharCode` over a whole calendar
 *  export would overflow the call stack. */
export function toBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunk = 0x8000;
  for (let at = 0; at < bytes.length; at += chunk) {
    binary += String.fromCharCode(...bytes.subarray(at, at + chunk));
  }
  return btoa(binary);
}
