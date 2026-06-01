import { describe, expect, it } from 'vitest';

import type { Account, Calendar, TaskList } from '../api/types';
import {
  accountTriState,
  buildSidebarTree,
  flattenLeaves,
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
  color_label: null,
  read_only: false,
  default_sound: null,
  account_id: accountId,
});

const makeTaskList = (
  id: string,
  name: string,
  accountId: string,
  parentId: string | null = null,
): TaskList => ({
  id,
  name,
  color: null,
  color_label: null,
  default_sound: null,
  embedded_in_calendar: null,
  read_only: false,
  account_id: accountId,
  parent_id: parentId,
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

describe('nested task-list tree', () => {
  it('nests child task lists under their parent in the Tasks section', () => {
    const tree = buildSidebarTree({
      accounts: [makeAccount('vik', 'Vikunja', 'vikunja')],
      calendars: [],
      taskLists: [
        makeTaskList('p', 'Parent', 'vik'),
        makeTaskList('c', 'Child', 'vik', 'p'),
        makeTaskList('top', 'Top-level', 'vik'),
      ],
      selectedCalendarIds: new Set(),
      selectedTaskListIds: new Set(),
    });
    const vik = tree.find((a) => a.accountId === 'vik')!;
    const tasks = vik.children.find((s) => s.kind === 'tasks');
    expect(tasks).toBeDefined();
    // Two roots (Parent, Top-level), name-sorted.
    expect(tasks!.children.map((l) => l.name)).toEqual(['Parent', 'Top-level']);
    const parent = tasks!.children.find((l) => l.containerId === 'p');
    expect(parent!.children.map((l) => l.containerId)).toEqual(['c']);
  });

  it('flat backends produce a depth-0 forest (no children)', () => {
    const tree = buildSidebarTree({
      accounts: [makeAccount(LOCAL_ACCOUNT_ID, 'Local')],
      calendars: [],
      taskLists: [
        makeTaskList('a', 'A', LOCAL_ACCOUNT_ID),
        makeTaskList('b', 'B', LOCAL_ACCOUNT_ID),
      ],
      selectedCalendarIds: new Set(),
      selectedTaskListIds: new Set(),
    });
    const tasks = tree[0].children.find((s) => s.kind === 'tasks');
    expect(tasks!.children.every((l) => l.children.length === 0)).toBe(true);
  });

  it('flattenLeaves walks the whole subtree; tri-state counts descendants', () => {
    const tree = buildSidebarTree({
      accounts: [makeAccount('vik', 'Vikunja', 'vikunja')],
      calendars: [],
      taskLists: [
        makeTaskList('p', 'Parent', 'vik'),
        makeTaskList('c', 'Child', 'vik', 'p'),
      ],
      selectedCalendarIds: new Set(),
      // Only the nested child is selected → the section is "mixed",
      // which requires tri-state to look past the root leaves.
      selectedTaskListIds: new Set(['c']),
    });
    const vik = tree.find((a) => a.accountId === 'vik')!;
    const tasks = vik.children.find((s) => s.kind === 'tasks')!;
    expect(flattenLeaves(tasks.children).map((l) => l.containerId)).toEqual([
      'p',
      'c',
    ]);
    expect(parentTriState(tasks.children)).toBe('mixed');
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
          children: [],
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
          children: [],
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
          children: [],
        },
        {
          key: 'b',
          kind: 'calendars',
          containerId: 'b',
          name: 'B',
          colorHex: null,
          readOnly: false,
          selected: false,
          children: [],
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
