import { useCallback, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import type { TaskUser } from '../api/types';
import { useAnnouncer } from '../a11y/announcerContext';

/**
 * Multi-select picker for task assignees, scoped to a list's member
 * pool (DESIGN §9.7). Selected users render as removable chips; a
 * `<select>` offers the members not yet picked. The caller gates
 * rendering on whether the list has any assignable members, so this
 * component itself stays purely presentational.
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
  onChange,
}: {
  members: TaskUser[];
  value: TaskUser[];
  currentUserId: string | null;
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
