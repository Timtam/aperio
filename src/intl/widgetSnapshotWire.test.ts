import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

import { buildWidgetSnapshot, type RecurringEventLike } from '@aperio/shared';

import type { Task } from '../api/types';

// The widget snapshot crosses a language boundary no compiler sees both sides
// of: `shared/widgetSnapshot.ts` writes the JSON, and Swift structs in
// `mobile/targets/widget/` decode it. Adding a field on one side and forgetting
// the other compiles cleanly here, passes every other test, and then fails
// twenty-five minutes into an iOS build — which is exactly how
// `strings.complete` shipped to CI missing from the Swift side.
//
// So this reads the actual Swift and checks it declares everything the builder
// actually emits. It is a text check on purpose: the alternative is a code
// generator, and a generator for three small structs would be more machinery
// than the problem deserves.

function swift(relative: string): string {
  // Relative to the repo root, which is where vitest runs. `import.meta.url`
  // does not survive the transform intact here.
  return readFileSync(resolve(process.cwd(), 'mobile/targets/widget', relative), 'utf8');
}

/** Property names declared in `struct <name>`. */
function declaredFields(source: string, name: string): Set<string> {
  const start = source.indexOf(`struct ${name}`);
  expect(start, `struct ${name} not found`).toBeGreaterThanOrEqual(0);
  const open = source.indexOf('{', start);
  // The structs are flat — no nested braces — so the first closing brace ends it.
  const end = source.indexOf('\n}', open);
  const body = source.slice(open, end);
  const fields = new Set<string>();
  for (const match of body.matchAll(/^\s*let\s+([A-Za-z_][A-Za-z0-9_]*)\s*:/gm)) {
    fields.add(match[1]!);
  }
  return fields;
}

interface TestEvent extends RecurringEventLike {
  calendar_id: string;
  title: string;
  all_day: boolean;
}

/** A snapshot with EVERY field populated, including the optional ones — an
 *  absent field would silently pass a check for "the Swift side knows it". */
function fullSnapshot() {
  const baseTask: Task = {
    id: 't1',
    list_id: 'list',
    title: 'thing',
    description: null,
    status: 'open',
    priority: 'medium',
    effort: 'medium',
    scheduled_date: '2026-08-04',
    scheduled_time: '09:00:00',
    deadline_date: null,
    deadline_time: null,
    deadline_reminder_days: null,
    recurrence: null,
    resurface_date: null,
    series_id: null,
    parent_id: null,
    section_id: null,
    color_label: null,
    reminders: [],
    assignees: [],
    sound: null,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    completed_at: null,
    etag: null,
  };
  const event: TestEvent = {
    id: 'e1',
    calendar_id: 'cal',
    title: 'Standup',
    start: new Date(2026, 7, 4, 9, 0).toISOString(),
    end: new Date(2026, 7, 4, 9, 30).toISOString(),
    all_day: false,
    recurrence: null,
  };
  return buildWidgetSnapshot<TestEvent>({
    events: [event],
    tasks: [baseTask],
    now: new Date(2026, 7, 3, 7, 0),
    horizonDays: 7,
    limit: 20,
    locale: 'de',
    strings: {
      empty: 'e',
      noTimed: 'n',
      stale: 's',
      allDay: 'a',
      complete: 'c',
      today: 't',
      runningUntil: 'r',
      kindEvent: 'ev',
      kindTask: 'tk',
    },
    eventColorOf: () => '#3b82f6',
    taskColorOf: () => '#3b82f6',
    calendarIdOf: (e) => e.calendar_id,
    titleOf: (e) => e.title,
    allDayOf: (e) => e.all_day,
  });
}

describe('the widget snapshot decodes on the Swift side', () => {
  const source = swift('Snapshot.swift');
  const snapshot = fullSnapshot();

  it('declares every envelope field the builder writes', () => {
    const declared = declaredFields(source, 'WidgetSnapshot');
    for (const key of Object.keys(snapshot)) {
      expect(declared, `WidgetSnapshot is missing \`${key}\``).toContain(key);
    }
  });

  it('declares every string the builder writes', () => {
    const declared = declaredFields(source, 'WidgetStrings');
    for (const key of Object.keys(snapshot.strings)) {
      expect(declared, `WidgetStrings is missing \`${key}\``).toContain(key);
    }
  });

  it('declares every item field the builder writes', () => {
    const declared = declaredFields(source, 'WidgetItem');
    // The fixture yields a timed event (carrying `end` and `color`) and a
    // completable task, so the union covers the optional fields too.
    const keys = new Set(snapshot.items.flatMap((item) => Object.keys(item)));
    expect(keys.size, 'fixture stopped covering the optional fields').toBeGreaterThan(7);
    for (const key of keys) {
      expect(declared, `WidgetItem is missing \`${key}\``).toContain(key);
    }
  });

  it('gives the no-data fallback a value for every string', () => {
    // A missing one here is not a decode failure but a compile failure: Swift's
    // memberwise initialiser takes every property, so `fallbackStrings` has to
    // name them all.
    const fallback = swift('Formatting.swift');
    const block = fallback.slice(fallback.indexOf('var fallbackStrings'));
    for (const key of Object.keys(snapshot.strings)) {
      expect(block, `fallbackStrings does not set \`${key}\``).toContain(`${key}:`);
    }
  });
});
