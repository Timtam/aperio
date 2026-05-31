import { listAccountsMissingCredentials } from '../api/client';
import type { Account } from '../api/types';

/** Caller-side helper: fetch the missing-credentials list and,
 *  if non-empty, hand it back so the caller can mount the dialog
 *  via DialogState. Returns `null` when there's nothing to do so
 *  the caller can skip opening the wizard altogether. */
export async function fetchAccountsNeedingConnect(): Promise<Account[] | null> {
  try {
    const accounts = await listAccountsMissingCredentials();
    return accounts.length === 0 ? null : accounts;
  } catch (err) {
    // eslint-disable-next-line no-console
    console.warn('list_accounts_missing_credentials failed', err);
    return null;
  }
}
