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
  /** What the host served for the list before this write, as the store
   *  read it last (review round 16 of #128). */
  before?: readonly Section[];
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

/** The rows as the host serves them: alike when the same sections stand. */
function sameRows(a: readonly Section[], b: readonly Section[]): boolean {
  const key = (rows: readonly Section[]) =>
    JSON.stringify(
      rows
        .map((s) => [s.id, s.name, s.order, s.color_label ?? null])
        .sort((x, y) => String(x[0]).localeCompare(String(y[0]))),
    );
  return key(a) === key(b);
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
 * order they were made, and those writes. A write is done with once a read
 * shows it — or, where the store knows what the host served before it, once
 * the host serves anything else: its refresh landed, and what the provider
 * keeps now stands, whatever another device did meanwhile.
 */
export function overRead(
  read: readonly Section[],
  writes: readonly SectionWrite[],
): { rows: Section[]; open: SectionWrite[] } {
  const open = writes.filter((w) =>
    w.before ? sameRows(read, w.before) && !shows(read, w) : !shows(read, w),
  );
  let rows = [...read];
  for (const write of open) {
    rows = rows.filter((s) => s.id !== write.upsert?.id && s.id !== write.remove);
    if (write.upsert) rows.push(write.upsert);
  }
  return { rows, open };
}
