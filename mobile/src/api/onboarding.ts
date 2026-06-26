import { listAccounts } from './accounts';
import { listCalendars } from './calendar';
import { listTaskLists } from './client';
import { syncStatus } from './sync';

/**
 * Whether this is a FRESH instance that should be offered the first-launch
 * wizard (DESIGN.md §19.11): no external account, no sync target configured,
 * and an empty local store (no task lists, no calendars).
 *
 * DATA-based on purpose, not a stored flag — an established install that already
 * has real data, or that already set up sync/accounts, is never re-prompted.
 * Composed from existing cal-ffi commands (no dedicated Rust call / no binding
 * regen). The implicit `local` account (migration 0003) is always present and
 * doesn't count. The caller pairs this with a device-local "already shown"
 * marker so an empty instance that dismissed the wizard isn't offered it again.
 */
export async function isFreshInstance(): Promise<boolean> {
  // Any non-local account → not fresh.
  const accounts = await listAccounts();
  if (accounts.some((a) => a.adapter_kind !== 'local')) return false;

  // A sync target configured.
  const status = await syncStatus();
  if (status.configured) return false;

  // Any local task list or calendar → the user has already created data.
  if ((await listTaskLists()).length > 0) return false;
  if ((await listCalendars()).length > 0) return false;

  return true;
}
