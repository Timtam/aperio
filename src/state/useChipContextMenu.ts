import { useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  deleteEventById,
  isCommandError,
  setEventColor,
  showContextMenu,
  updateEvent as apiUpdateEvent,
  type ContextMenuItemRequest,
} from '../api/client';
import type {
  CalendarEvent,
  Task,
  TaskPriority,
  TaskStatus,
} from '../api/types';
import { duplicateTask } from '../components/duplicateActions';
import { seriesIdOf } from '../intl/recurrence';
import { useCalendarStore } from './calendarStoreContext';
import { useDialogState } from './dialogStateContext';
import {
  peekCalendarUserEmail,
  warmCalendarUserEmail,
} from './currentUserEmail';
import { surfaceTaskNow } from './moveActions';
import { useTaskPriorityAction } from './useTaskPriority';
import { useTaskStatusActions } from './useTaskStatusToggle';

/** Lower-case, `mailto:`-stripped form for comparing addresses. */
function normalizeEmail(value: string | null | undefined): string {
  if (!value) return '';
  return value.trim().replace(/^mailto:/i, '').toLowerCase();
}

/**
 * Native context menu for event chips and task rows. Mirrors the
 * sidebar's per-row menu (Phase 6f follow-up) but spans every
 * calendar surface — WeekView, DayView, MonthView, AgendaView, plus
 * the dedicated TaskView.
 *
 * Entries:
 *
 *   Events:
 *     - Bearbeiten        (open EventDialog)
 *     - Verschieben nach… (open MoveCopyDialog, defaultMode='move')
 *     - Kopieren nach…    (open MoveCopyDialog, defaultMode='copy')
 *     - ──
 *     - Löschen           (confirm + delete; recurring → series only,
 *                          callers needing per-occurrence still wire
 *                          their own DeleteEventScopeDialog branch)
 *
 *   Tasks:
 *     - Bearbeiten
 *     - Status >
 *         · Offen
 *         · In Arbeit
 *         · Erledigt
 *         · Abgebrochen
 *       (each entry is a check item; the one matching the task's
 *        current status carries a check-mark glyph, drawn by the OS)
 *     - Priorität >
 *         · Niedrig
 *         · Mittel
 *         · Hoch
 *       (same check-row shape as Status — a one-click priority change
 *        without opening the editor)
 *     - Verschieben nach…
 *     - Kopieren nach…
 *     - ──
 *     - Löschen
 *
 * `position` is optional: omit for right-click triggers so the OS
 * anchors at the cursor; pass `{x, y}` for Shift+F10 / Menu-key
 * triggers so the menu appears near the focused row, not at a
 * stale cursor coordinate the user has long since moved.
 *
 * Why share a hook: the same menu shape, the same dispatch table,
 * and the same i18n strings repeat across five views. Centralising
 * keeps the labels in sync and the dispatch consistent — particularly
 * the recurring-event delete branch, which we want to surface the
 * same way everywhere.
 */

export interface ChipContextMenuActions {
  /** Open the menu for an event. */
  openForEvent: (
    event: CalendarEvent,
    position?: { x: number; y: number },
    onAfter?: () => void,
  ) => Promise<void>;
  /** Open the menu for a task. */
  openForTask: (
    task: Task,
    position?: { x: number; y: number },
    onAfter?: () => void,
  ) => Promise<void>;
}

export function useChipContextMenu(): ChipContextMenuActions {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const {
    openEventDialog,
    openTaskDialog,
    openMoveCopy,
    invalidateData,
  } = useDialogState();
  const { set: setTaskStatus } = useTaskStatusActions();
  const setTaskPriority = useTaskPriorityAction();
  const { colorLabels, calendars } = useCalendarStore();
  const calById = useMemo(
    () => new Map(calendars.map((c) => [c.id, c])),
    [calendars],
  );

  const openForEvent = useCallback(
    async (
      event: CalendarEvent,
      position?: { x: number; y: number },
      onAfter?: () => void,
    ) => {
      // Color submenu — a check row per (non-ad-hoc) label plus "none".
      // For external events this routes to a host-local override (no
      // provider write, so iCloud & co. never reject it); local events
      // keep their color on the row via update_event. Custom one-off
      // colors stay in the event dialog's picker.
      const colorSubmenu: ContextMenuItemRequest = {
        kind: 'submenu',
        label: t('chipMenu.color'),
        items: [
          {
            kind: 'check',
            id: 'color:none',
            label: t('chipMenu.colorNone'),
            checked: !event.color_label,
          },
          ...colorLabels
            .filter((l) => !l.ad_hoc)
            .map((l) => ({
              kind: 'check' as const,
              id: `color:${l.id}`,
              label: l.name,
              checked: event.color_label === l.id,
            })),
        ],
      };
      // A meeting the connected account ORGANIZES (with attendees, on a
      // scheduling-capable provider) can be CANCELLED with an attendee
      // notification — offer that vs a silent remove as two menu items instead
      // of a single Delete. "Who am I" is a LIVE provider call, so we NEVER
      // await it here (that would stall the native menu / hang offline): read
      // it synchronously from the cache (warmed whenever a meeting is opened),
      // and if it's not warm yet, prime it and just show plain Delete this time.
      const cal = calById.get(event.calendar_id);
      let offersChoice = false;
      if ((cal?.supports_scheduling ?? false) && event.attendees.length > 0) {
        const cached = peekCalendarUserEmail(event.calendar_id);
        if (cached === undefined) {
          warmCalendarUserEmail(event.calendar_id);
        } else {
          const me = normalizeEmail(cached);
          offersChoice = !!me && normalizeEmail(event.organizer) === me;
        }
      }
      const items: ContextMenuItemRequest[] = [
        { id: 'edit', label: t('chipMenu.edit') },
        { id: 'move', label: t('chipMenu.moveTo') },
        { id: 'copy', label: t('chipMenu.copyTo') },
        colorSubmenu,
        { kind: 'separator' },
        ...(offersChoice
          ? [
              { id: 'cancel-notify', label: t('chipMenu.cancelNotify') },
              { id: 'cancel-silent', label: t('chipMenu.cancelSilent') },
            ]
          : [{ id: 'delete', label: t('chipMenu.delete') }]),
      ];
      let selected: string | null = null;
      try {
        selected = await showContextMenu(items, position);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('show_context_menu failed', err);
      }
      if (selected === 'edit') {
        openEventDialog(event);
      } else if (selected === 'move') {
        openMoveCopy({ kind: 'event', event, defaultMode: 'move' });
      } else if (selected === 'copy') {
        openMoveCopy({ kind: 'event', event, defaultMode: 'copy' });
      } else if (
        selected === 'delete' ||
        selected === 'cancel-notify' ||
        selected === 'cancel-silent'
      ) {
        // Recurring events: deleting via the chip context menu maps
        // to "delete the whole series". The per-occurrence variant
        // (DeleteEventScopeDialog) lives behind the per-view
        // keyboard handlers; keeping the menu simple is intentional.
        const id = seriesIdOf(event);
        // `cancel-notify`/`cancel-silent` only appear for a meeting we organize;
        // plain `delete` keeps the prior heuristic (notify iff attendees — the
        // adapters tolerate that from a non-organizer, falling back to a plain
        // delete).
        const send =
          selected === 'cancel-notify'
            ? true
            : selected === 'cancel-silent'
              ? false
              : event.attendees.length > 0;
        try {
          await deleteEventById(id, event.calendar_id, send);
          announce(
            selected === 'cancel-notify'
              ? t('dialogs.event.meetingCancelled', { title: event.title })
              : t('dialogs.event.deleted', { title: event.title }),
          );
          invalidateData();
        } catch (err) {
          if (isCommandError(err)) {
            announce(`${err.code}: ${err.message}`);
          } else {
            announce(String(err));
          }
        }
      } else if (selected?.startsWith('color:')) {
        const raw = selected.slice('color:'.length);
        const next = raw === 'none' ? null : raw;
        const seriesId = seriesIdOf(event);
        const cal = calById.get(event.calendar_id);
        // Color rides update_event when the provider can store it natively:
        // local always, plus color-capable CalDAV (RFC 7986 COLOR). Otherwise
        // it goes to a host-local override (no provider PUT to be rejected).
        const storesColorNatively =
          cal?.account_id === 'local' || cal?.supports_event_color === true;
        try {
          if (storesColorNatively) {
            await apiUpdateEvent(
              { ...event, id: seriesId, color_label: next },
              event.calendar_id,
            );
          } else {
            await setEventColor(seriesId, event.calendar_id, next);
          }
          announce(t('chipMenu.colorSet', { title: event.title }));
          invalidateData();
        } catch (err) {
          if (isCommandError(err)) {
            announce(`${err.code}: ${err.message}`);
          } else {
            announce(String(err));
          }
        }
      }
      onAfter?.();
    },
    [
      t,
      announce,
      openEventDialog,
      openMoveCopy,
      invalidateData,
      colorLabels,
      calById,
    ],
  );

  const openForTask = useCallback(
    async (
      task: Task,
      position?: { x: number; y: number },
      onAfter?: () => void,
    ) => {
      // Status submenu — every status is a check row so the OS draws
      // its native check-mark next to the row that matches the task's
      // current state. Sighted users glance, SR users hear "checked"
      // from the platform itself.
      const statusSubmenu: ContextMenuItemRequest = {
        kind: 'submenu',
        label: t('chipMenu.status'),
        items: (
          ['open', 'in_progress', 'completed', 'cancelled'] as TaskStatus[]
        ).map((s) => ({
          kind: 'check' as const,
          id: `status:${s}`,
          label: t(`chipMenu.statusValue.${s}`),
          checked: task.status === s,
        })),
      };
      // Priority submenu — Low / Medium / High as check rows, in the
      // same order as the editor's priority picker. The row matching the
      // task's current priority carries the OS check-mark, just like the
      // status submenu. Reuses the dialog's priority labels so the wording
      // lives in one place; medium is the neutral default. A one-click
      // priority change without opening the editor.
      const prioritySubmenu: ContextMenuItemRequest = {
        kind: 'submenu',
        label: t('chipMenu.priority'),
        items: (['low', 'medium', 'high'] as TaskPriority[]).map((p) => ({
          kind: 'check' as const,
          id: `priority:${p}`,
          label: t(`dialogs.task.priority.${p}`),
          checked: task.priority === p,
        })),
      };
      // Subtasks can't be moved or copied independently — they're
      // glued to their parent. Hide the Move/Copy entries entirely
      // so the user doesn't see a path that leads nowhere; the
      // parent's row carries the moveable handle for the whole
      // family.
      const isSubtask = task.parent_id !== null;
      // "Ins Backlog holen" — only for a deferred task (a future resurface
      // day waiting in the "Zukünftig" group, DESIGN §9.12). Clearing the
      // resurface date pulls it back into the active backlog now.
      const isDeferred = task.resurface_date !== null;
      const items: ContextMenuItemRequest[] = [
        { id: 'edit', label: t('chipMenu.edit') },
        { id: 'duplicate', label: t('chipMenu.duplicate') },
        statusSubmenu,
        prioritySubmenu,
        ...(isDeferred
          ? ([
              { id: 'surface', label: t('chipMenu.bringToBacklog') },
            ] as ContextMenuItemRequest[])
          : []),
        ...(isSubtask
          ? []
          : ([
              { id: 'move', label: t('chipMenu.moveTo') },
              { id: 'copy', label: t('chipMenu.copyTo') },
            ] as ContextMenuItemRequest[])),
        { kind: 'separator' },
        { id: 'delete', label: t('chipMenu.delete') },
      ];
      let selected: string | null = null;
      try {
        selected = await showContextMenu(items, position);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('show_context_menu failed', err);
      }
      if (selected === 'edit') {
        openTaskDialog(task);
      } else if (selected === 'duplicate') {
        // Same primitive as the Ctrl+D shortcut and the mobile menu — a flat,
        // in-place copy in the same list (no subtree, parent_id reset).
        try {
          await duplicateTask(task);
          announce(t('actions.duplicated', { title: task.title }));
          invalidateData();
        } catch (err) {
          if (isCommandError(err)) {
            announce(`${err.code}: ${err.message}`);
          } else {
            announce(String(err));
          }
        }
      } else if (selected === 'move') {
        openMoveCopy({ kind: 'task', task, defaultMode: 'move' });
      } else if (selected === 'copy') {
        openMoveCopy({ kind: 'task', task, defaultMode: 'copy' });
      } else if (selected?.startsWith('status:')) {
        const next = selected.slice('status:'.length) as TaskStatus;
        await setTaskStatus(task, next);
      } else if (selected?.startsWith('priority:')) {
        const next = selected.slice('priority:'.length) as TaskPriority;
        await setTaskPriority(task, next);
      } else if (selected === 'surface') {
        try {
          await surfaceTaskNow(task);
          announce(t('chipMenu.broughtToBacklog', { title: task.title }));
          invalidateData();
        } catch (err) {
          if (isCommandError(err)) {
            announce(`${err.code}: ${err.message}`);
          } else {
            announce(String(err));
          }
        }
      } else if (selected === 'delete') {
        // Tasks don't have a recurring-occurrence concept on the
        // delete side (Phase 9.x leaves task recurrence as a future
        // wave), so this is a flat delete.
        try {
          await invoke<void>('delete_task', {
            id: task.id,
            listId: task.list_id,
          });
          announce(t('dialogs.task.deleted', { title: task.title }));
          invalidateData();
        } catch (err) {
          if (isCommandError(err)) {
            announce(`${err.code}: ${err.message}`);
          } else {
            announce(String(err));
          }
        }
      }
      onAfter?.();
    },
    [
      t,
      announce,
      openTaskDialog,
      openMoveCopy,
      invalidateData,
      setTaskStatus,
      setTaskPriority,
    ],
  );

  return { openForEvent, openForTask };
}
