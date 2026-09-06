import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import type { TaskUser } from '../api/types';

/**
 * The picker offers exactly what the SOURCE can hold.
 *
 * Todoist stores one `assignee_id`. Its adapter takes `assignees[0]` and warns
 * about the rest ({@link ../../crates/adapter-todoist/src/tasks.rs} —
 * `first_assignee_id`), so a second person picked here was dropped on write:
 * the save reported success and the name was gone at the next refresh. The
 * editor has to know the limit BEFORE the user spends a choice on it.
 */

vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));

const ANNA: TaskUser = { id: 'u1', name: 'Anna', email: null } as TaskUser;
const BERND: TaskUser = { id: 'u2', name: 'Bernd', email: null } as TaskUser;
const MEMBERS = [ANNA, BERND];

async function picker(
  mode: 'single' | 'multiple',
  value: TaskUser[],
  onChange = vi.fn(),
) {
  const { AssigneePicker } = await import('./AssigneePicker');
  render(
    <AssigneePicker
      members={MEMBERS}
      value={value}
      currentUserId={null}
      mode={mode}
      onChange={onChange}
    />,
  );
  return onChange;
}

describe('AssigneePicker → a source that holds ONE person', () => {
  it('offers a single choice, not a way to add a second', async () => {
    await picker('single', [ANNA]);
    // One control, one focus stop. No chips, so no "add another" affordance
    // whose result the source would throw away.
    const controls = screen.getAllByRole('combobox');
    expect(controls).toHaveLength(1);
    expect((controls[0] as HTMLSelectElement).value).toBe('u1');
    expect(screen.queryByRole('button', { name: /entfernen|remove/i })).toBeNull();
  });

  it('replaces the assignee instead of appending one', async () => {
    const onChange = await picker('single', [ANNA]);
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'u2' } });
    // The whole bug in one assertion: picking Bernd must not yield [Anna, Bernd].
    expect(onChange).toHaveBeenCalledWith([BERND]);
  });

  it('lets the task go back to nobody', async () => {
    const onChange = await picker('single', [ANNA]);
    fireEvent.change(screen.getByRole('combobox'), { target: { value: '' } });
    expect(onChange).toHaveBeenCalledWith([]);
  });
});

describe('AssigneePicker → a source that holds several', () => {
  it('still adds rather than replaces', async () => {
    const onChange = await picker('multiple', [ANNA]);
    // Chips for the chosen, a select for the rest — unchanged behaviour.
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'u2' } });
    expect(onChange).toHaveBeenCalledWith([ANNA, BERND]);
    expect(
      screen.getByRole('button', { name: /Anna entfernen|Remove Anna/i }),
    ).toBeTruthy();
  });
});
