import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  createCalendar,
  createTaskList,
  deleteCalendar,
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
            return (
              <li key={cal.id} role="listitem" className="sidebar__row">
                <button
                  type="button"
                  role="checkbox"
                  aria-checked={checked}
                  className="sidebar__toggle"
                  onClick={() => toggleCalendar(cal.id)}
                  style={
                    cal.color
                      ? ({ '--container-color': cal.color.hex } as React.CSSProperties)
                      : undefined
                  }
                >
                  <span className="sidebar__swatch" aria-hidden="true" />
                  <span className="sidebar__name">{cal.name}</span>
                </button>
                <button
                  type="button"
                  className="sidebar__delete"
                  onClick={() => onDeleteCalendar(cal.id, cal.name)}
                  aria-label={t('sidebar.deleteCalendar', { name: cal.name })}
                >
                  ✕
                </button>
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
            return (
              <li key={list.id} role="listitem" className="sidebar__row">
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
