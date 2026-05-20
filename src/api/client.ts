// Typed wrappers around the Tauri `invoke` boundary.
//
// Every function maps 1:1 to a `#[tauri::command]` defined in
// `src-tauri/src/commands/`. The argument shapes mirror the Rust
// `Request` structs.

import { invoke } from '@tauri-apps/api/core';
import type {
  Account,
  AdapterKind,
  Calendar,
  CalendarEvent,
  ColorLabel,
  CommandError,
  NewEvent,
  SearchResults,
  Task,
  TaskList,
} from './types';

/** Type guard — a backend error always carries `code` and `message`. */
export function isCommandError(value: unknown): value is CommandError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'code' in value &&
    'message' in value
  );
}

// ── Calendars ──────────────────────────────────────────────────────────────

export const listCalendars = () => invoke<Calendar[]>('list_calendars');

export interface CreateCalendarRequest {
  name: string;
  color_hex: string | null;
}

export const createCalendar = (request: CreateCalendarRequest) =>
  invoke<Calendar>('create_calendar', { request });

export const deleteCalendar = (id: string) =>
  invoke<void>('delete_calendar', { id });

// ── Events ─────────────────────────────────────────────────────────────────

export interface EventRangeRequest {
  calendar_id: string;
  start: string;
  end: string;
}

export const getEvents = (request: EventRangeRequest) =>
  invoke<CalendarEvent[]>('get_events', { request });

export interface CreateEventRequest extends NewEvent {
  calendar_id: string;
}

export const createEvent = (request: CreateEventRequest) =>
  invoke<CalendarEvent>('create_event', { request });

export const updateEvent = (event: CalendarEvent) =>
  invoke<CalendarEvent>('update_event', { event });

export const deleteEventById = (id: string) =>
  invoke<void>('delete_event', { id });

export const getEventById = (id: string) =>
  invoke<CalendarEvent | null>('get_event_by_id', { id });

export const getTaskById = (id: string) =>
  invoke<Task | null>('get_task_by_id', { id });

/** Append `occurrence` to a recurring event's EXDATE list. Used when
 *  the user deletes or overrides a single occurrence — the master row
 *  stays intact and the expansion engine simply skips that date. */
export const addEventExdate = (id: string, occurrence: string) =>
  invoke<void>('add_event_exdate', { id, occurrence });

// ── Task lists & tasks ─────────────────────────────────────────────────────

export const listTaskLists = () => invoke<TaskList[]>('list_task_lists');

export interface CreateTaskListRequest {
  name: string;
  embedded_in_calendar: string | null;
}

export const createTaskList = (request: CreateTaskListRequest) =>
  invoke<TaskList>('create_task_list', { request });

export const deleteTaskList = (id: string) =>
  invoke<void>('delete_task_list', { id });

export const getTasks = (list_id: string) =>
  invoke<Task[]>('get_tasks', { listId: list_id });

export interface CreateTaskRequest {
  list_id: string;
  title: string;
  description: string | null;
  status: Task['status'];
  priority: Task['priority'];
  scheduled_date: string | null;
  deadline_type: 'on' | 'by' | null;
  deadline_date: string | null;
  deadline_time: string | null;
  recurrence: unknown;
  parent_id: string | null;
  color_label: string | null;
  reminders: Task['reminders'];
  sound: Task['sound'];
}

export const createTask = (request: CreateTaskRequest) =>
  invoke<Task>('create_task', { request });

// ── Color labels ───────────────────────────────────────────────────────────

export const listColorLabels = () =>
  invoke<ColorLabel[]>('list_color_labels');

export interface CreateColorLabelRequest {
  name: string;
  hex: string;
}

export const createColorLabel = (request: CreateColorLabelRequest) =>
  invoke<ColorLabel>('create_color_label', { request });

export const updateColorLabel = (label: ColorLabel) =>
  invoke<ColorLabel>('update_color_label', { label });

export const deleteColorLabel = (id: string) =>
  invoke<void>('delete_color_label', { id });

// ── Search ─────────────────────────────────────────────────────────────────

export type SearchKind = 'both' | 'events' | 'tasks';

export type EventTypeFilter = 'any' | 'single' | 'recurring' | 'all_day';

export interface SearchFilters {
  kind?: SearchKind;
  calendar_ids?: string[];
  list_ids?: string[];
  /** ISO 8601 lower bound (date or datetime). */
  since?: string | null;
  /** ISO 8601 upper bound (date or datetime). */
  until?: string | null;
  /** Event-type filter — ignored when `kind = 'tasks'`. */
  event_type?: EventTypeFilter;
  /** Task-status whitelist — empty means no restriction. */
  task_statuses?: Task['status'][];
}

export const search = (query: string, filters?: SearchFilters) =>
  invoke<SearchResults>('search', { query, filters: filters ?? null });

// ── Reminders ──────────────────────────────────────────────────────────────

export interface UpcomingReminder {
  item_id: string;
  item_kind: 'event' | 'task';
  title: string;
  /** ISO 8601 UTC. */
  trigger_at: string;
}

export const listUpcomingReminders = () =>
  invoke<UpcomingReminder[]>('list_upcoming_reminders');

// ── Accounts ───────────────────────────────────────────────────────────────

export const listAccounts = () => invoke<Account[]>('list_accounts');

export interface CreateAccountRequest {
  adapter_kind: AdapterKind;
  display_name: string;
  /** Optional adapter-specific config; defaults to "{}" backend-side. */
  config_json?: string;
}

export const createAccount = (request: CreateAccountRequest) =>
  invoke<Account>('create_account', { request });

export const deleteAccount = (id: string) =>
  invoke<void>('delete_account', { id });
