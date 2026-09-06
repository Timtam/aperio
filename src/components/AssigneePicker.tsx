import { useCallback, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { TaskAssignment } from '@aperio/shared';

import type { TaskUser } from '../api/types';
import { useAnnouncer } from '../a11y/announcerContext';

/**
 * Picker for task assignees, scoped to a list's member pool (DESIGN §9.7).
 *
 * Two shapes, chosen by what the SOURCE can hold. `multiple` renders the
 * selected users as removable chips with a `<select>` offering the rest;
 * `single` renders one `<select>` and nothing else, because a source that
 * keeps one assignee must not be asked for two.
 *
 * That distinction is the point rather than a refinement. Todoist stores a
 * single `assignee_id`; its adapter takes the first and warns about the rest,
 * so a second person picked here was dropped on write — the save reported
 * success and the name was gone at the next refresh, which is the silent kind
 * of loss. The caller passes the list's declared `task_assignment` mode.
 *
 * Removal parks focus before the chip unmounts. The Modal's focus net
 * explicitly does not cover a focused element that UNMOUNTS — the node is
 * disconnected before its focusout can bubble — so without this the ×
 * button took the screen reader to <body> and NVDA fell out of the
 * dialog. The AttendeePicker next door has always done this; the two
 * should not disagree about what removing a person costs.
 */
export function AssigneePicker({
  members,
  value,
  currentUserId,
  mode = 'multiple',
  onChange,
}: {
  members: TaskUser[];
  value: TaskUser[];
  currentUserId: string | null;
  /** What the source can hold. `none` is not passed — the caller does not
   *  render the picker at all then. */
  mode?: TaskAssignment;
  onChange: (next: TaskUser[]) => void;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const selectedIds = useMemo(() => new Set(value.map((u) => u.id)), [value]);
  const available = useMemo(
    () => members.filter((m) => !selectedIds.has(m.id)),
    [members, selectedIds],
  );

  // One ref per chip's remove button, in render order, plus the add-select
  // and a last-resort container. Focus lands on whichever of those still
  // exists after the removal.
  const removeRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const selectRef = useRef<HTMLSelectElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  // Mark the connected account itself so "assign to me" is obvious.
  const label = (u: TaskUser) =>
    u.id === currentUserId
      ? t('dialogs.task.assignees.you', { name: u.name })
      : u.name;

  const add = (id: string) => {
    const user = members.find((m) => m.id === id);
    if (user && !selectedIds.has(user.id)) onChange([...value, user]);
  };

  const removeAt = useCallback(
    (index: number) => {
      const removed = value[index];
      if (!removed) return;
      onChange(value.filter((x) => x.id !== removed.id));
      announce(t('dialogs.task.assignees.removed', { name: removed.name }));
      // After React commits: the chip that slid into this slot, else the one
      // before it, else the add-select (which the removed member just
      // re-entered), else the container. Same continuation the attendee
      // picker offers — remove three people without re-navigating once.
      queueMicrotask(() => {
        const next =
          removeRefs.current[index] ??
          removeRefs.current[index - 1] ??
          selectRef.current ??
          rootRef.current;
        next?.focus();
      });
    },
    [value, onChange, announce, t],
  );

  // One assignee at most: one control, one focus stop, and no way to pick a
  // second person the source would drop. Empty value = unassigned, which is a
  // real choice here rather than the absence of one.
  if (mode === 'single') {
    return (
      <div className="assignee-picker" ref={rootRef} tabIndex={-1}>
        <select
          className="assignee-picker__single"
          value={value[0]?.id ?? ''}
          onChange={(e) => {
            const picked = members.find((m) => m.id === e.target.value);
            onChange(picked ? [picked] : []);
          }}
        >
          <option value="">{t('dialogs.task.assignees.nobody')}</option>
          {members.map((m) => (
            <option key={m.id} value={m.id}>
              {label(m)}
            </option>
          ))}
        </select>
      </div>
    );
  }

  return (
    <div className="assignee-picker" ref={rootRef} tabIndex={-1}>
      {value.length > 0 && (
        <ul className="assignee-picker__chips">
          {value.map((u, i) => (
            <li key={u.id} className="assignee-picker__chip">
              <span>{label(u)}</span>
              <button
                type="button"
                ref={(el) => {
                  removeRefs.current[i] = el;
                }}
                className="assignee-picker__remove"
                onClick={() => removeAt(i)}
                aria-label={t('dialogs.task.assignees.remove', {
                  name: u.name,
                })}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}
      {available.length > 0 && (
        <select
          ref={selectRef}
          className="assignee-picker__add"
          value=""
          onChange={(e) => {
            if (e.target.value) add(e.target.value);
          }}
        >
          <option value="">{t('dialogs.task.assignees.add')}</option>
          {available.map((m) => (
            <option key={m.id} value={m.id}>
              {label(m)}
            </option>
          ))}
        </select>
      )}
    </div>
  );
}
