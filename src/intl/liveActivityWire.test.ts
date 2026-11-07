// The Live Activity's wire: `shared/liveActivityIos.ts` writes an
// `ActivityContent` as JSON, the app's native module decodes it into
// `AperioEventAttributes.ContentState`, and the widget extension draws it from
// a SECOND copy of that struct (the two targets share no source). Nothing
// checks any of this before a device: a field missing in Swift fails the
// decode and starts no activity, a difference between the copies starts
// activities nothing can draw. So this test reads the Swift.

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

import { activityContent } from '../../shared/liveActivityIos';
import type { LiveActivityChain } from '../../shared/types';

const ROOT = resolve(__dirname, '../..');
const APP_COPY = 'mobile/modules/cal-ffi/ios/LiveActivityAttributes.swift';
const EXTENSION_COPY = 'mobile/targets/widget/LiveActivityAttributes.swift';

const read = (relative: string) => readFileSync(resolve(ROOT, relative), 'utf8');

/** The `var` fields declared directly in `struct name` (not in a nested one). */
function fields(source: string, name: string): string[] {
  const lines = source.split('\n');
  const start = lines.findIndex((line) => new RegExp(`\\bstruct ${name}\\b`).test(line));
  if (start < 0) return [];
  const out: string[] = [];
  let depth = 0;
  for (let i = start; i < lines.length; i += 1) {
    const line = lines[i];
    const own = depth === 1;
    const field = /^\s*var\s+([A-Za-z_][A-Za-z0-9_]*)\s*:/.exec(line);
    if (own && field) out.push(field[1]);
    for (const ch of line) {
      if (ch === '{') depth += 1;
      if (ch === '}') depth -= 1;
    }
    if (depth === 0 && i > start) break;
  }
  return out;
}

const CHAIN: LiveActivityChain = {
  keys: ['A', 'B'],
  start: '2026-11-09T09:50:00Z',
  end: '2026-11-09T11:30:00Z',
  sections: ['A', 'B'].map((key, i) => ({
    from: i === 0 ? '2026-11-09T09:50:00Z' : '2026-11-09T10:45:00Z',
    until: i === 0 ? '2026-11-09T10:45:00Z' : '2026-11-09T11:30:00Z',
    event: {
      key,
      item_id: key,
      calendar_id: 'work',
      original_start: null,
      title: key,
      location: 'Raum 2.14',
      start: '2026-11-09T10:00:00Z',
      end: '2026-11-09T11:00:00Z',
    },
    follow_up: { title: 'Review', start: '2026-11-09T11:00:00Z' },
    started: false,
    promoted: true,
    signal: [],
  })),
};

const CONTENT = activityContent(CHAIN, new Date('2026-11-09T09:55:00Z'), 'de', {
  begins: 'Beginn',
  ends: 'Ende',
  followUp: 'Danach: {title} um {time}',
  over: 'Vorbei',
});

describe('the Live Activity wire', () => {
  it('has the attributes twice, byte for byte', () => {
    expect(read(EXTENSION_COPY)).toBe(read(APP_COPY));
  });

  it('decodes exactly what the app writes', () => {
    const swift = read(APP_COPY);
    expect(fields(swift, 'ContentState').sort()).toEqual(Object.keys(CONTENT).sort());
    expect(fields(swift, 'Shown').sort()).toEqual(Object.keys(CONTENT.current).sort());
    expect(fields(swift, 'Words').sort()).toEqual(Object.keys(CONTENT.words).sort());
    expect(fields(swift, 'AperioEventAttributes')).toEqual(['chainKey']);
  });

  it('writes a placeholder pair the extension fills', () => {
    const view = read('mobile/targets/widget/EventLiveActivity.swift');
    expect(CONTENT.words.followUp).toContain('{title}');
    expect(CONTENT.words.followUp).toContain('{time}');
    expect(view).toContain('"{title}"');
    expect(view).toContain('"{time}"');
  });

  it('opens the tapped appointment through the address the router reads', () => {
    const view = read('mobile/targets/widget/EventLiveActivity.swift');
    for (const field of ['eventId', 'calendarId', 'start', 'title']) {
      expect(view).toContain(`"${field}=`);
    }
    expect(view).toContain('aperio://event?');
  });
});
