import { describe, expect, it } from 'vitest';

import { eventInstanceKey } from './eventKey';

describe('eventInstanceKey', () => {
  it('gives the same shared event distinct keys per calendar', () => {
    // Google reuses one event id across every attendee/shared-calendar copy, so
    // the same group event arrives under several calendar_ids with an identical
    // id. Each must get a distinct row key.
    const a = eventInstanceKey({ id: 'grp@2026-06-14T09:00:00Z', calendar_id: 'alice@x' });
    const b = eventInstanceKey({ id: 'grp@2026-06-14T09:00:00Z', calendar_id: 'bob@x' });
    const c = eventInstanceKey({ id: 'grp@2026-06-14T09:00:00Z', calendar_id: 'carol@x' });
    expect(new Set([a, b, c]).size).toBe(3);
  });

  it('is stable and identical for the same (calendar, event)', () => {
    const ev = { id: 'evt-1', calendar_id: 'primary' };
    expect(eventInstanceKey(ev)).toBe(eventInstanceKey({ ...ev }));
  });

  it('keeps genuinely different events on one calendar distinct', () => {
    const one = eventInstanceKey({ id: 'a', calendar_id: 'primary' });
    const two = eventInstanceKey({ id: 'b', calendar_id: 'primary' });
    expect(one).not.toBe(two);
  });
});
