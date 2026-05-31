import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  deleteEventById,
  isCommandError,
  showContextMenu,
  type ContextMenuItemRequest,
} from '../api/client';
import type { CalendarEvent, Task, TaskStatus } from '../api/types';
import { seriesIdOf } from '../intl/recurrence';
import { useDialogState } from './dialogStateContext';
import { useTaskStatusActions } from './useTaskStatusToggle';

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

  const openForEvent = useCallback(
    async (
      event: CalendarEvent,
      position?: { x: number; y: number },
      onAfter?: () => void,
    ) => {
      const items: ContextMenuItemRequest[] = [
        { id: 'edit', label: t('chipMenu.edit') },
        { id: 'move', label: t('chipMenu.moveTo') },
        { id: 'copy', label: t('chipMenu.copyTo') },
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
        openEventDialog(event);
      } else if (selected === 'move') {
        openMoveCopy({ kind: 'event', event, defaultMode: 'move' });
      } else if (selected === 'copy') {
        openMoveCopy({ kind: 'event', event, defaultMode: 'copy' });
      } else if (selected === 'delete') {
        // Recurring events: deleting via the chip context menu maps
        // to "delete the whole series". The per-occurrence variant
        // (DeleteEventScopeDialog) lives behind the per-view
        // keyboard handlers; keeping the menu simple is intentional.
        const id = seriesIdOf(event);
        try {
          await deleteEventById(id, event.calendar_id);
          announce(t('dialogs.event.deleted', { title: event.title }));
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
    [t, announce, openEventDialog, openMoveCopy, invalidateData],
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
      // Subtasks can't be moved or copied independently — they're
      // glued to their parent. Hide the Move/Copy entries entirely
      // so the user doesn't see a path that leads nowhere; the
      // parent's row carries the moveable handle for the whole
      // family.
      const isSubtask = task.parent_id !== null;
      const items: ContextMenuItemRequest[] = [
        { id: 'edit', label: t('chipMenu.edit') },
        statusSubmenu,
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
      } else if (selected === 'move') {
        openMoveCopy({ kind: 'task', task, defaultMode: 'move' });
      } else if (selected === 'copy') {
        openMoveCopy({ kind: 'task', task, defaultMode: 'copy' });
      } else if (selected?.startsWith('status:')) {
        const next = selected.slice('status:'.length) as TaskStatus;
        await setTaskStatus(task, next);
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
    ],
  );

  return { openForEvent, openForTask };
}
