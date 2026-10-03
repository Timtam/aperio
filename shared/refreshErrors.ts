// What the external-refresh surfaces say, on the desktop and the phone alike.
//
// The core decides why each account fails and how severe that is
// (`AccountRefreshErrors.cause` / `.rank`, decision 181): a withheld grant
// before a login problem before anything else. These helpers only read that
// off: which cause leads when one surface speaks for several accounts, which
// accounts a growth announcement still has to name, and the sentence that
// ends a warm pass (decision 180).

import type { ContainerRefreshError } from './generated/ContainerRefreshError';
import type { FailingAccount } from './generated/FailingAccount';
import type { PassOutcome } from './generated/PassOutcome';
import type { RefreshCause } from './generated/RefreshCause';
import { joinNames } from './listWords';
import { compareNames } from './ordering';

export type { AccountRefreshErrors } from './generated/AccountRefreshErrors';
export type { CacheRefreshStatus } from './generated/CacheRefreshStatus';
export type { ContainerRefreshError } from './generated/ContainerRefreshError';
export type { FailingAccount } from './generated/FailingAccount';
export type { PassOutcome } from './generated/PassOutcome';
export type { RefreshCause } from './generated/RefreshCause';

type Translate = (key: string, values?: Record<string, unknown>) => string;

/** An account's cause with the core's rank of it. */
export interface RankedCause {
  cause: RefreshCause;
  rank: number;
}

/**
 * The cause a surface that speaks for several accounts leads with: the one
 * the core ranks highest. `null` when nothing fails.
 */
export function leadingCause(rows: readonly RankedCause[]): RefreshCause | null {
  let lead: RankedCause | null = null;
  for (const row of rows) {
    if (lead == null || row.rank > lead.rank) lead = row;
  }
  return lead?.cause ?? null;
}

/** Per account: the rank of the failure it was last announced with. */
export type AnnouncedFailures = ReadonlyMap<string, number>;

/**
 * The failing accounts a growth announcement still has to name: one that was
 * not failing before, and one whose cause grew more severe than what was
 * said about it (decision 185). A cause that eases says nothing.
 */
export function toAnnounce<T extends RankedCause & { account_id: string }>(
  rows: readonly T[],
  announced: AnnouncedFailures,
): T[] {
  return rows.filter((row) => (announced.get(row.account_id) ?? -1) < row.rank);
}

/**
 * What is known as announced after a publish of `rows`: every failing
 * account at the most severe rank said about it so far. An account that
 * stopped failing drops out, so failing again is news again.
 */
export function afterAnnouncing(
  rows: readonly (RankedCause & { account_id: string })[],
  announced: AnnouncedFailures,
): Map<string, number> {
  return new Map(
    rows.map((row) => [row.account_id, Math.max(row.rank, announced.get(row.account_id) ?? -1)]),
  );
}

/**
 * `announced` with the accounts a sentence has just named (the pass-end
 * "except: …"), so the growth announcement after it does not name them a
 * second time (decision 180).
 *
 * Only accounts that were not failing before: the sentence names an account,
 * not its cause, so one that was already announced and now fails worse keeps
 * the rank it was announced with, and its worse cause is still said
 * (decision 185).
 */
export function withSpoken(
  announced: AnnouncedFailures,
  spoken: readonly FailingAccount[],
): Map<string, number> {
  const next = new Map(announced);
  for (const account of spoken) {
    if (!next.has(account.account_id)) next.set(account.account_id, account.rank);
  }
  return next;
}

/**
 * The accounts [`passEndSentence`] names for `outcome`: none when it says
 * that nothing could be updated, or when there is no outcome.
 */
export function namedByPassEnd(outcome: PassOutcome | null | undefined): FailingAccount[] {
  if (outcome == null || outcome.all_failed) return [];
  return [...outcome.failing];
}

/**
 * The sentence that ends a warm pass: "External data updated." when nothing
 * is left undone, "… updated, except: A and B." naming the accounts that are
 * not current, in name order, and "… could not be updated." when the pass
 * read nothing at all and the failure is confirmed. Without an outcome (one
 * that could not be read, or a status from an older core), and for a pass
 * that read nothing but has no confirmed failure yet, a sentence that only
 * says the refresh ended, claiming neither.
 */
export function passEndSentence(
  outcome: PassOutcome | null | undefined,
  t: Translate,
): string {
  if (outcome == null) return t('cacheRefresh.ended');
  // Nothing read: "could not be updated" once a failure is confirmed (the
  // warning names its cause). A first network blip of an unforced pass is
  // not confirmed yet and shows nowhere, so it only ends the refresh.
  if (outcome.all_failed) {
    return outcome.failing.length > 0 ? t('cacheRefresh.failed') : t('cacheRefresh.ended');
  }
  if (outcome.failing.length === 0) return t('cacheRefresh.done');
  const names = outcome.failing.map((account) => account.name).sort(compareNames);
  return t('cacheRefresh.doneExcept', { names: joinNames(names, t) });
}

/** The data families a withheld grant can cover, in the order a sentence
 *  names them. */
const WITHHELD_FAMILIES = ['calendars', 'task_lists', 'contact_lists'] as const;

/**
 * What an account may not read, for the one line that stands for its withheld
 * families ("the calendars and the task lists"): each family appears once, as
 * its listing row, with cause `access`. The account's data in general when no
 * family can be told.
 */
export function withheldPhrase(
  errors: readonly ContainerRefreshError[],
  t: Translate,
): string {
  const families = WITHHELD_FAMILIES.filter((family) =>
    errors.some((row) => row.cause === 'access' && row.scope === family),
  );
  if (families.length === 0) return t('refreshErrors.accessWhat.data');
  return joinNames(
    families.map((family) => t(`refreshErrors.accessWhat.${family}`)),
    t,
  );
}

/** How stale a withheld account's data is: the earliest last success among
 *  its withheld rows, `null` when one of them never succeeded — a family
 *  never read is the oldest data there is. */
export function withheldSince(errors: readonly ContainerRefreshError[]): string | null {
  let earliest: string | null = null;
  for (const row of errors) {
    if (row.cause !== 'access') continue;
    if (row.last_success_at == null) return null;
    if (earliest == null || Date.parse(row.last_success_at) < Date.parse(earliest)) {
      earliest = row.last_success_at;
    }
  }
  return earliest;
}
