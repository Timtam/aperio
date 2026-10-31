import type { Section } from '@aperio/shared';

/**
 * The phone's section writes that landed and that a read of their list does
 * not show yet (decision 277, review round 15 of #128). After a write the
 * host only invalidates an external list: its read serves the rows from
 * before until the refresh it starts lands, and a refresh a busy provider
 * turns away leaves them so.
 */
export interface SectionWrite {
  upsert?: Section;
  remove?: string;
}

/** The section a write names. */
function writtenId(write: SectionWrite): string | undefined {
  return write.upsert?.id ?? write.remove;
}

/** `writes` with `write` added; a later write of a section takes the earlier
 *  one's place. */
export function withWrite(writes: readonly SectionWrite[], write: SectionWrite): SectionWrite[] {
  const id = writtenId(write);
  return [...writes.filter((w) => writtenId(w) !== id), write];
}

/** Whether `read` shows `write` already: the section as written, or gone. */
function shows(read: readonly Section[], write: SectionWrite): boolean {
  if (write.upsert) {
    const upsert = write.upsert;
    return read.some((s) => s.id === upsert.id && s.name === upsert.name);
  }
  return !read.some((s) => s.id === write.remove);
}

/**
 * A read of a list with the writes it does not show yet laid over it, in the
 * order they were made, and those writes: the ones it shows are done with.
 */
export function overRead(
  read: readonly Section[],
  writes: readonly SectionWrite[],
): { rows: Section[]; open: SectionWrite[] } {
  const open = writes.filter((w) => !shows(read, w));
  let rows = [...read];
  for (const write of open) {
    rows = rows.filter((s) => s.id !== write.upsert?.id && s.id !== write.remove);
    if (write.upsert) rows.push(write.upsert);
  }
  return { rows, open };
}
