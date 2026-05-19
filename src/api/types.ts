// TypeScript counterparts to the cal-core types.
//
// Phase 1 maintains these by hand. The crate `cal-core` is the source of
// truth; if a field changes there, mirror it here. Phase 2 will look at
// generating these from the Rust definitions (e.g. via `specta`), at
// which point this file becomes a starting point.

export interface ContainerColor {
  hex: string;
  source: 'native' | 'custom';
}

export interface Calendar {
  id: string;
  name: string;
  color: ContainerColor | null;
  read_only: boolean;
  default_sound: SoundConfig | null;
}

export interface SoundConfig {
  source:
    | { type: 'system' }
    | { type: 'silent' }
    | { type: 'custom'; sha256: string };
  volume: number;
}

export interface Reminder {
  kind:
    | { type: 'relative'; minutes_before: number }
    | { type: 'absolute'; at: string }
    | { type: 'app_start' }
    | { type: 'email'; minutes_before: number };
  sound: SoundConfig | null;
}

export interface EventRecurrence {
  rrule: string;
  exceptions: string[];
}

export interface CalendarEvent {
  id: string;
  calendar_id: string;
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
  created_at: string;
  updated_at: string;
  etag: string | null;
}

export interface NewEvent {
  title: string;
  description: string | null;
  location: string | null;
  start: string;
  end: string;
  all_day: boolean;
  recurrence: EventRecurrence | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  attendees: string[];
}

export interface TaskList {
  id: string;
  name: string;
  color: ContainerColor | null;
  default_sound: SoundConfig | null;
  embedded_in_calendar: string | null;
  read_only: boolean;
}

export type TaskStatus = 'open' | 'in_progress' | 'completed' | 'cancelled';
export type TaskPriority = 'low' | 'medium' | 'high';
export type DeadlineType = 'on' | 'by';

export interface Task {
  id: string;
  list_id: string;
  title: string;
  description: string | null;
  status: TaskStatus;
  priority: TaskPriority;
  scheduled_date: string | null;
  deadline_type: 'on' | 'by' | null;
  deadline_date: string | null;
  deadline_time: string | null;
  recurrence: unknown;
  parent_id: string | null;
  color_label: string | null;
  reminders: Reminder[];
  sound: SoundConfig | null;
  created_at: string;
  updated_at: string;
  completed_at: string | null;
  etag: string | null;
}

export interface ColorLabel {
  id: string;
  name: string;
  hex: string;
}

export interface SearchResults {
  events: CalendarEvent[];
  tasks: Task[];
}

export interface CommandError {
  code:
    | 'auth'
    | 'forbidden'
    | 'not_found'
    | 'conflict'
    | 'network'
    | 'protocol'
    | 'invalid_input'
    | 'unsupported'
    | 'internal';
  message: string;
}
