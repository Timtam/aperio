import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import type { TaskUser } from '../api/types';

/**
 * Multi-select picker for task assignees, scoped to a list's member
 * pool (DESIGN §9.7). Selected users render as removable chips; a
 * `<select>` offers the members not yet picked. The caller gates
 * rendering on whether the list has any assignable members, so this
 * component itself stays purely presentational.
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
  const selectedIds = useMemo(() => new Set(value.map((u) => u.id)), [value]);
  const available = useMemo(
    () => members.filter((m) => !selectedIds.has(m.id)),
    [members, selectedIds],
  );

  // Mark the connected account itself so "assign to me" is obvious.
  const label = (u: TaskUser) =>
    u.id === currentUserId
      ? t('dialogs.task.assignees.you', { name: u.name })
      : u.name;

  const add = (id: string) => {
    const user = members.find((m) => m.id === id);
    if (user && !selectedIds.has(user.id)) onChange([...value, user]);
  };

  return (
    <div className="assignee-picker">
      {value.length > 0 && (
        <ul className="assignee-picker__chips">
          {value.map((u) => (
            <li key={u.id} className="assignee-picker__chip">
              <span>{label(u)}</span>
              <button
                type="button"
                className="assignee-picker__remove"
                onClick={() => onChange(value.filter((x) => x.id !== u.id))}
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
