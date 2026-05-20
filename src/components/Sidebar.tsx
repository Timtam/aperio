import { useCallback, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  clearContainerNameOverride,
  createCalendar,
  createTaskList,
  deleteCalendar,
  isCommandError,
  setContainerNameOverride,
  type ContainerKind,
} from '../api/client';
import { useCalendarStore } from '../state/CalendarStore';
import { useDialogState } from '../state/DialogState';

/**
 * Sidebar: filter for calendars and task lists, plus quick-create
 * actions for local containers.
 *
 * Each filter row is a button with `aria-pressed` reflecting whether the
 * container is currently visible. Toggling never deletes — it just hides
 * the source from the main view.
 *
 * Rename: every row exposes an edit button. Activating it swaps the
 * name into an inline text field; Enter commits the override, Escape
 * cancels, an empty value clears the override and reverts to the
 * source name. The rename never leaves the local DB in this commit;
 * a follow-up wires the trait method that pushes it back to CalDAV
 * (PROPPATCH `DAV:displayname`) / local SQLite for adapters that
 * support it.
 *
 * The delete button on calendars is intentionally tiny and labelled
 * specifically per row, so screen readers always announce *which*
 * calendar is being removed.
 */
export function Sidebar() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const {
    calendars,
    selectedCalendarIds,
    toggleCalendar,
    refreshCalendars,
    taskLists,
    selectedTaskListIds,
    toggleTaskList,
    refreshTaskLists,
  } = useCalendarStore();
  const { openColorLabels, openAccounts } = useDialogState();

  // Identifies the row currently in edit mode. `null` means "none —
  // all rows show their static name + buttons". Only one row may be
  // in edit mode at a time, matching the underlying Modal-or-inline
  // discipline elsewhere in the app.
  const [editing, setEditing] = useState<{
    kind: ContainerKind;
    id: string;
  } | null>(null);
  const [draft, setDraft] = useState('');

  const startEdit = useCallback(
    (kind: ContainerKind, id: string, currentName: string) => {
      setEditing({ kind, id });
      setDraft(currentName);
    },
    [],
  );

  const cancelEdit = useCallback(() => {
    setEditing(null);
    setDraft('');
  }, []);

  const commitEdit = useCallback(async () => {
    if (!editing) return;
    const { kind, id } = editing;
    const trimmed = draft.trim();
    try {
      if (trimmed === '') {
        await clearContainerNameOverride(id, kind);
        announce(t('sidebar.renameCleared'));
      } else {
        await setContainerNameOverride(id, kind, trimmed);
        announce(t('sidebar.renamed', { name: trimmed }));
      }
      // Pull fresh container lists so the new name surfaces in the
      // sidebar (and in every downstream consumer of the store).
      if (kind === 'calendar') {
        await refreshCalendars();
      } else {
        await refreshTaskLists();
      }
    } catch (err) {
      if (isCommandError(err)) {
        announce(`${err.code}: ${err.message}`);
      } else {
        announce(String(err));
      }
    } finally {
      setEditing(null);
      setDraft('');
    }
  }, [
    editing,
    draft,
    refreshCalendars,
    refreshTaskLists,
    announce,
    t,
  ]);

  const onEditKey = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        void commitEdit();
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        cancelEdit();
        return;
      }
    },
    [commitEdit, cancelEdit],
  );

  const onCreateCalendar = useCallback(async () => {
    try {
      const cal = await createCalendar({
        name: t('sidebar.newCalendarName', { n: calendars.length + 1 }),
        color_hex: '#1e88e5',
      });
      await refreshCalendars();
      announce(t('sidebar.calendarCreated', { name: cal.name }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('create_calendar failed', err);
    }
  }, [calendars.length, refreshCalendars, announce, t]);

  const onDeleteCalendar = useCallback(
    async (id: string, name: string) => {
      try {
        await deleteCalendar(id);
        await refreshCalendars();
        announce(t('sidebar.calendarDeleted', { name }));
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('delete_calendar failed', err);
      }
    },
    [refreshCalendars, announce, t],
  );

  const onCreateTaskList = useCallback(async () => {
    try {
      const list = await createTaskList({
        name: t('sidebar.newTaskListName', { n: taskLists.length + 1 }),
        embedded_in_calendar: null,
      });
      await refreshTaskLists();
      announce(t('sidebar.taskListCreated', { name: list.name }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('create_task_list failed', err);
    }
  }, [taskLists.length, refreshTaskLists, announce, t]);

  return (
    <aside
      className="sidebar"
      aria-label={t('sidebar.label')}
      data-region="sidebar"
    >
      <section aria-labelledby="sb-cal" className="sidebar__section">
        <h2 id="sb-cal">{t('sidebar.calendars')}</h2>
        <ul role="list" className="sidebar__list">
          {calendars.map((cal) => {
            const checked = selectedCalendarIds.has(cal.id);
            const isEditing =
              editing?.kind === 'calendar' && editing.id === cal.id;
            return (
              <li key={cal.id} role="listitem" className="sidebar__row">
                {isEditing ? (
                  <RenameField
                    name={cal.name}
                    value={draft}
                    onChange={setDraft}
                    onCommit={commitEdit}
                    onCancel={cancelEdit}
                    onKeyDown={onEditKey}
                    ariaLabel={t('sidebar.renameInputLabel', {
                      name: cal.name,
                    })}
                    hint={t('sidebar.renameHint')}
                  />
                ) : (
                  <>
                    <button
                      type="button"
                      role="checkbox"
                      aria-checked={checked}
                      className="sidebar__toggle"
                      onClick={() => toggleCalendar(cal.id)}
                      style={
                        cal.color
                          ? ({
                              '--container-color': cal.color.hex,
                            } as React.CSSProperties)
                          : undefined
                      }
                    >
                      <span className="sidebar__swatch" aria-hidden="true" />
                      <span className="sidebar__name">{cal.name}</span>
                    </button>
                    <button
                      type="button"
                      className="sidebar__edit"
                      onClick={() => startEdit('calendar', cal.id, cal.name)}
                      aria-label={t('sidebar.renameButton', {
                        name: cal.name,
                      })}
                      title={t('sidebar.renameButtonShort')}
                    >
                      ✎
                    </button>
                    <button
                      type="button"
                      className="sidebar__delete"
                      onClick={() => onDeleteCalendar(cal.id, cal.name)}
                      aria-label={t('sidebar.deleteCalendar', {
                        name: cal.name,
                      })}
                    >
                      ✕
                    </button>
                  </>
                )}
              </li>
            );
          })}
        </ul>
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateCalendar}
        >
          + {t('sidebar.newCalendar')}
        </button>
      </section>

      <section aria-labelledby="sb-tl" className="sidebar__section">
        <h2 id="sb-tl">{t('sidebar.taskLists')}</h2>
        <ul role="list" className="sidebar__list">
          {taskLists.map((list) => {
            const checked = selectedTaskListIds.has(list.id);
            const isEditing =
              editing?.kind === 'task_list' && editing.id === list.id;
            return (
              <li key={list.id} role="listitem" className="sidebar__row">
                {isEditing ? (
                  <RenameField
                    name={list.name}
                    value={draft}
                    onChange={setDraft}
                    onCommit={commitEdit}
                    onCancel={cancelEdit}
                    onKeyDown={onEditKey}
                    ariaLabel={t('sidebar.renameInputLabel', {
                      name: list.name,
                    })}
                    hint={t('sidebar.renameHint')}
                  />
                ) : (
                  <>
                    <button
                      type="button"
                      role="checkbox"
                      aria-checked={checked}
                      className="sidebar__toggle"
                      onClick={() => toggleTaskList(list.id)}
                    >
                      <span className="sidebar__swatch" aria-hidden="true" />
                      <span className="sidebar__name">{list.name}</span>
                    </button>
                    <button
                      type="button"
                      className="sidebar__edit"
                      onClick={() =>
                        startEdit('task_list', list.id, list.name)
                      }
                      aria-label={t('sidebar.renameButton', {
                        name: list.name,
                      })}
                      title={t('sidebar.renameButtonShort')}
                    >
                      ✎
                    </button>
                  </>
                )}
              </li>
            );
          })}
        </ul>
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateTaskList}
        >
          + {t('sidebar.newTaskList')}
        </button>
      </section>

      <section className="sidebar__section">
        <button
          type="button"
          className="sidebar__add"
          onClick={() => openColorLabels()}
        >
          {t('sidebar.manageColorLabels')}
        </button>
        <button
          type="button"
          className="sidebar__add"
          onClick={() => openAccounts()}
        >
          {t('sidebar.manageAccounts')}
        </button>
      </section>
    </aside>
  );
}

/**
 * Inline rename input. Auto-focuses on mount so the user can start
 * typing immediately; Enter / Escape are handled by the caller. The
 * accompanying hint text explains the empty-value-clears-override
 * semantics inline rather than burying it in a tooltip.
 */
function RenameField({
  name: _name,
  value,
  onChange,
  onCommit,
  onCancel,
  onKeyDown,
  ariaLabel,
  hint,
}: {
  name: string;
  value: string;
  onChange: (v: string) => void;
  onCommit: () => void;
  onCancel: () => void;
  onKeyDown: (e: KeyboardEvent<HTMLInputElement>) => void;
  ariaLabel: string;
  hint: string;
}) {
  return (
    <div className="sidebar__rename">
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={onCommit}
        aria-label={ariaLabel}
        autoFocus
        className="sidebar__rename-input"
      />
      <span className="sidebar__rename-hint" aria-hidden="true">
        {hint}
      </span>
      {/* Both buttons are also reachable via Enter / Escape; they
          exist here for pointer users and as a visible cue that the
          row is in a special mode. */}
      <button
        type="button"
        className="sidebar__rename-action"
        onMouseDown={(e) => e.preventDefault()}
        onClick={onCommit}
        aria-label={ariaLabel}
      >
        ✓
      </button>
      <button
        type="button"
        className="sidebar__rename-action"
        onMouseDown={(e) => e.preventDefault()}
        onClick={onCancel}
        aria-label={ariaLabel}
      >
        ✕
      </button>
    </div>
  );
}
