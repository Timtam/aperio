import { describe, expect, it } from 'vitest';

import {
  isSeriesOccurrence,
  occurrenceIsoOf,
  seriesIdOf,
} from '@aperio/shared';

/** The shape the recurrence helpers read off an event row. */
const event = (id: string, rrule?: string) => ({
  id,
  start: '2026-06-14T13:00:00Z',
  end: '2026-06-14T14:00:00Z',
  recurrence: rrule ? { rrule, exceptions: [] } : null,
});

describe('delete scope for a CalDAV event', () => {
  it('does not mistake an @ in the UID for an occurrence marker', () => {
    // The week view used to gate the occurrence-or-series dialog on
    // `id.includes('@')`. A CalDAV UID is routinely `something@domain` —
    // Aperio mints its own as `…@aperio`, iCloud has its own conventions — so
    // every plain single event asked a question that has no answer for it.
    const plain = event('/cal/home/abc.ics|3F2504E0-4F89@aperio');
    expect(isSeriesOccurrence(plain)).toBe(false);
    expect(occurrenceIsoOf(plain)).toBeNull();
  });

  it('keeps the whole UID as the delete target', () => {
    // The same split truncated the id the series delete was sent to: the part
    // after the `@` was thrown away, so the request named a resource that does
    // not exist.
    const id = '/cal/home/abc.ics|3F2504E0-4F89@aperio';
    expect(seriesIdOf(event(id))).toBe(id);
  });

  it('still recognises a RECURRENCE-ID override', () => {
    // The marker that really does mean "one instance of a series", even when
    // the UID in front of it contains an @ of its own.
    const override = event(
      '/cal/home/abc.ics|3F2504E0-4F89@aperio::rid::2026-06-14T13:00:00Z',
    );
    expect(isSeriesOccurrence(override)).toBe(true);
    expect(occurrenceIsoOf(override)).toBe('2026-06-14T13:00:00Z');
    expect(seriesIdOf(override)).toBe('/cal/home/abc.ics|3F2504E0-4F89@aperio');
  });
});
