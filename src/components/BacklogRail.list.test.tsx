import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { Task, TaskList } from '../api/types';

/**
 * The backlog's three sections as ONE listbox with a group each (Toni,
 * 2026-10-31): one tab stop, the arrows go through every task across the
 * sections, Page Up / Page Down jump between sections.
 */

vi.mock('@tauri-apps/api/core', () => ({ invoke: () => Promise.resolve([]) }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: () => Promise.resolve(() => {}),
  emit: () => Promise.resolve(),
}));

const LIST = { id: 'L1', name: 'Arbeit' } as unknown as TaskList;
const task = (id: string, title: string): Task =>
  ({
    id,
    list_id: 'L1',
    parent_id: null,
    title,
    description: null,
    status: 'open',
    priority: 'medium',
    effort: 'medium',
    scheduled_date: null,
    deadline_date: null,
    section_id: null,
    color_label: null,
    reminders: [],
    assignees: [],
  }) as unknown as Task;
const A = task('t-a', 'Steuer');
const B = task('t-b', 'Reifen');
const C = task('t-c', 'Garten');
const D = task('t-d', 'Keller');

const hooks = vi.hoisted(() => ({
  openTaskDialog: vi.fn(),
  tasks: { tasks: [] as Task[], taskListById: new Map() },
  store: { sectionColorById: new Map() },
  cascade: { visualEffortSizing: false, priorityScale: 'three' },
  menu: { openForTask: () => Promise.resolve() },
  dialogs: {} as Record<string, unknown>,
}));
hooks.dialogs = {
  openTaskDialog: hooks.openTaskDialog,
  openPlanTask: () => {},
  invalidateData: () => {},
};
hooks.tasks = { tasks: [A, B, C, D], taskListById: new Map([[LIST.id, LIST]]) };
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/useTasks', () => ({ useTasks: () => hooks.tasks }));
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => hooks.store }));
vi.mock('../state/taskCascadeContext', () => ({ useTaskCascadeEnabled: () => hooks.cascade }));
vi.mock('../state/useChipContextMenu', () => ({ useChipContextMenu: () => hooks.menu }));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => hooks.dialogs }));

afterEach(() => {
  document.body.innerHTML = '';
  hooks.openTaskDialog.mockClear();
});

async function open() {
  const { BacklogList } = await import('./BacklogRail');
  render(
    <>
      <h2 id="rail">Backlog (4)</h2>
      <BacklogList
        labelledBy="rail"
        labelById={new Map()}
        todayKey="2026-10-31"
        sections={[
          { key: 'this-week', heading: 'Diese Woche (1)', items: [A] },
          { key: 'next-week', heading: 'Nächste Woche (0)', items: [] },
          { key: 'rest', heading: 'Weiteres (3)', items: [B, C, D] },
        ]}
      />
    </>,
  );
  const list = screen.getByRole('listbox', { name: 'Backlog (4)' });
  const active = () => {
    const id = list.getAttribute('aria-activedescendant');
    return id ? document.getElementById(id)?.getAttribute('aria-label') ?? '' : '';
  };
  const press = (key: string) => fireEvent.keyDown(list, { key });
  return { list, active, press };
}

describe('the backlog list', () => {
  it('is one tab stop with a named group per section that has tasks', async () => {
    const { list } = await open();
    expect(screen.getAllByRole('listbox')).toHaveLength(1);
    expect(list.getAttribute('tabindex')).toBe('0');
    expect(list.hasAttribute('data-region-target')).toBe(true);
    const groups = screen.getAllByRole('group');
    expect(groups.map((g) => g.getAttribute('aria-labelledby') && document.getElementById(g.getAttribute('aria-labelledby')!)?.textContent)).toEqual([
      'Diese Woche (1)',
      'Weiteres (3)',
    ]);
    expect(screen.getAllByRole('option')).toHaveLength(4);
  });

  it('moves through every task across the sections', async () => {
    const { active, press } = await open();
    expect(active()).toMatch(/^Steuer/);
    press('ArrowDown');
    expect(active()).toMatch(/^Reifen/);
    press('ArrowUp');
    expect(active()).toMatch(/^Steuer/);
    press('End');
    expect(active()).toMatch(/^Keller/);
    press('Home');
    expect(active()).toMatch(/^Steuer/);
  });

  it('jumps a section with Page Down and Page Up', async () => {
    const { active, press } = await open();
    press('PageDown');
    expect(active()).toMatch(/^Reifen/);
    press('PageDown');
    expect(active()).toMatch(/^Keller/);
    press('PageUp');
    expect(active()).toMatch(/^Reifen/);
    press('PageUp');
    expect(active()).toMatch(/^Steuer/);
  });

  it('opens the active task', async () => {
    const { press } = await open();
    press('ArrowDown');
    press('ArrowDown');
    press('Enter');
    expect(hooks.openTaskDialog).toHaveBeenCalledWith(C);
  });
});
