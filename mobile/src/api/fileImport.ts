// File import api-client (DESIGN-file-import.md), over the cal-ffi Host: the
// formats the enabled plugins read, and one file read through the plugin for
// its format. Mirrors the desktop's `file_import_formats` / `read_import_file`
// commands; nothing is written until the editor or the list creates the
// appointments.

import type { FileImport, ImportFormat } from '@aperio/shared';
import CalFfi from '../../modules/cal-ffi';

/** The formats the enabled plugins read, named in `lang`: the picker's filter. */
export const fileImportFormats = async (lang: string): Promise<ImportFormat[]> =>
  JSON.parse(await CalFfi.fileImportFormatsJson(lang)) as ImportFormat[];

/** Read the file at `path` — a filesystem path, not a URI: the picker's copy,
 *  or a shared file copied into the app. Rejects with the desktop's codes, which
 *  `importFailureSentence` turns into a sentence. */
export const readImportFile = async (path: string): Promise<FileImport> =>
  JSON.parse(await CalFfi.readImportFileJson(path)) as FileImport;
