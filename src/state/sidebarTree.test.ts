import { describe, expect, it } from 'vitest';

import type { Account, Calendar, TaskList } from '../api/types';
import {
  accountTriState,
  buildSidebarTree,
  LOCAL_ACCOUNT_ID,
  parentTriState,
} from './sidebarTree';

const makeAccount = (id: string, name: string, kind = 'local'): Account => ({
  id,
  adapter_kind: kind as Account['adapter_kind'],
  display_name: name,
  config_json: '{}',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

const makeCalendar = (
  id: string,
  name: string,
  accountId: string,
  color?: string,
): Calendar => ({
  id,
  name,
  color: color ? { hex: color, source: 'native' } : null,
  read_only: false,
  default_sound: null,
  account_id: accountId,
});

const makeTaskList = (
  id: string,
  name: string,
  accountId: string,
): TaskList => ({
  id,
  name,
  color: null,
  default_sound: null,
  embedded_in_calendar: null,
  read_only: false,
  account_id: accountId,
});

describe('buildSidebarTree', () => {
  it('puts the local account first and sorts the rest alphabetically', () => {
    const accounts = [
      makeAccount('uuid-b', 'Outlook Work'),
      makeAccount(LOCAL_ACCOUNT_ID, 'Local'),
      makeAccount('uuid-a', 'Apple iCloud'),
    ];
    const tree = buildSidebarTree({
      accounts,
      calendars: [],
      taskLists: [],
      selectedCalendarIds: new Set(),
      selectedTaskListIds: new Set(),
    });
    expect(tree.map((n) => n.displayName)).toEqual([
      'Local',
      'Apple iCloud',
      'Outlook Work',
    ]);
  });

  it('groups calendars and task lists under their owning account', () => {
    const accounts = [
      makeAccount(LOCAL_ACCOUNT_ID, 'Local'),
      makeAccount('outlook', 'Outlook', 'microsoft_graph'),
    ];
    const calendars = [
      makeCalendar('cal-1', 'My Calendar', LOCAL_ACCOUNT_ID, '#1e88e5'),
      makeCalendar('cal-2', 'Work', 'outlook'),
    ];
    const taskLists = [makeTaskList('tl-1', 'Backlog', 'outlook')];
    const tree = buildSidebarTree({
      accounts,
      calendars,
      taskLists,
      selectedCalendarIds: new Set(['cal-1']),
      selectedTaskListIds: new Set(),
    });
    const local = tree.find((n) => n.accountId === LOCAL_ACCOUNT_ID)!;
    expect(local.children).toHaveLength(1);
    expect(local.children[0].kind).toBe('calendars');
    expect(local.children[0].children).toHaveLength(1);
    expect(local.children[0].children[0].name).toBe('My Calendar');
    expect(local.children[0].children[0].selected).toBe(true);
    expect(local.children[0].children[0].colorHex).toBe('#1e88e5');

    const outlook = tree.find((n) => n.accountId === 'outlook')!;
    expect(outlook.children).toHaveLength(2);
    expect(outlook.children.map((s) => s.kind)).toEqual([
      'calendars',
      'tasks',
    ]);
  });

  it('omits sections that have no children', () => {
    // EWS-style: calendar-only account with no task lists.
    const tree = buildSidebarTree({
      accounts: [
        makeAccount(LOCAL_ACCOUNT_ID, 'Local'),
        makeAccount('ews', 'Exchange', 'ews'),
      ],
      calendars: [makeCalendar('c1', 'Cal', 'ews')],
      taskLists: [],
      selectedCalendarIds: new Set(),
      selectedTaskListIds: new Set(),
    });
    const ews = tree.find((n) => n.accountId === 'ews')!;
    expect(ews.children).toHaveLength(1);
    expect(ews.children[0].kind).toBe('calendars');
  });

  it('marks an empty account so the UI can render a hint', () => {
    const tree = buildSidebarTree({
      accounts: [
        makeAccount(LOCAL_ACCOUNT_ID, 'Local'),
        makeAccount('new', 'Just added', 'caldav'),
      ],
      calendars: [],
      taskLists: [],
      selectedCalendarIds: new Set(),
      selectedTaskListIds: new Set(),
    });
    const fresh = tree.find((n) => n.accountId === 'new')!;
    expect(fresh.isEmpty).toBe(true);
    expect(fresh.children).toHaveLength(0);
  });

  it('synthesises a local account row when accounts is empty', () => {
    // Defensive: ancient DBs without the seeded local row would
    // otherwise drop their calendars from the sidebar entirely.
    const tree = buildSidebarTree({
      accounts: [],
      calendars: [makeCalendar('c1', 'Cal', LOCAL_ACCOUNT_ID)],
      taskLists: [],
      selectedCalendarIds: new Set(),
      selectedTaskListIds: new Set(),
    });
    expect(tree).toHaveLength(1);
    expect(tree[0].accountId).toBe(LOCAL_ACCOUNT_ID);
    expect(tree[0].children[0].children[0].name).toBe('Cal');
  });
});

describe('parentTriState / accountTriState', () => {
  it('returns unchecked when no leaf is selected', () => {
    expect(
      parentTriState([
        {
          key: 'a',
          kind: 'calendars',
          containerId: 'a',
          name: 'A',
          colorHex: null,
          readOnly: false,
          selected: false,
        },
      ]),
    ).toBe('unchecked');
  });

  it('returns checked when all leaves are selected', () => {
    expect(
      parentTriState([
        {
          key: 'a',
          kind: 'calendars',
          containerId: 'a',
          name: 'A',
          colorHex: null,
          readOnly: false,
          selected: true,
        },
      ]),
    ).toBe('checked');
  });

  it('returns mixed when some but not all leaves are selected', () => {
    expect(
      parentTriState([
        {
          key: 'a',
          kind: 'calendars',
          containerId: 'a',
          name: 'A',
          colorHex: null,
          readOnly: false,
          selected: true,
        },
        {
          key: 'b',
          kind: 'calendars',
          containerId: 'b',
          name: 'B',
          colorHex: null,
          readOnly: false,
          selected: false,
        },
      ]),
    ).toBe('mixed');
  });

  it('treats empty as unchecked (so an account with no children does not falsely report as selected)', () => {
    expect(parentTriState([])).toBe('unchecked');
  });

  it('rolls up across sections for accountTriState', () => {
    const tree = buildSidebarTree({
      accounts: [
        makeAccount(LOCAL_ACCOUNT_ID, 'Local'),
        makeAccount('a', 'A', 'caldav'),
      ],
      calendars: [
        makeCalendar('c1', 'Cal 1', 'a'),
        makeCalendar('c2', 'Cal 2', 'a'),
      ],
      taskLists: [makeTaskList('tl1', 'List', 'a')],
      selectedCalendarIds: new Set(['c1', 'c2']),
      selectedTaskListIds: new Set(),
    });
    // Calendars all on, tasks list off → mixed.
    const a = tree.find((n) => n.accountId === 'a')!;
    expect(accountTriState(a)).toBe('mixed');
  });
});
