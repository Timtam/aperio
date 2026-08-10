// Full-text search api-client — events + tasks via the Host's search_json.
//
// Covers BOTH halves, exactly as the desktop `search` command does: the local
// tables' FTS and the external snapshot cache's mirrors, so an iCloud, Google
// or Exchange item is found here too. (The comment that used to stand here said
// otherwise; the cache half landed in cal-ffi and this was never updated.)
//
// A READ path, so — unlike the mutating clients — it does NOT schedule a
// background push. Types mirror the desktop search wire shape (src/api/client.ts).

import type { Task } from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';
import type { CalendarEvent } from './calendar';

export type SearchKind = 'both' | 'events' | 'tasks';
export type EventTypeFilter = 'any' | 'single' | 'recurring' | 'all_day';

export interface SearchFilters {
  kind?: SearchKind;
  calendar_ids?: string[];
  list_ids?: string[];
  since?: string | null;
  until?: string | null;
  event_type?: EventTypeFilter;
  task_statuses?: string[];
}

export interface SearchResults {
  events: CalendarEvent[];
  tasks: Task[];
}

/** Local FTS over events + tasks. `filters` omitted → both kinds, no limits.
 *  Returns full event/task rows in two arrays (the frontend flattens them). */
export const search = async (
  query: string,
  filters?: SearchFilters,
): Promise<SearchResults> =>
  JSON.parse(
    await CalFfi.searchJson(query, filters ? JSON.stringify(filters) : ''),
  ) as SearchResults;
