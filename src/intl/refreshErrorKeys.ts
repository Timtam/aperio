import type { RefreshCause } from '@aperio/shared';

// Which words a desktop surface says for a failing account, by the cause the
// core leads with (access, then auth, then anything else; decision 181). One
// table, so the sidebar row, its warning glyph's tooltip and the accounts list
// cannot disagree — and kept out of `useRefreshErrors`, whose module a test
// mocks whole.

/** The suffix an account row's label carries while the account fails. */
export function refreshErrorRowKey(cause: RefreshCause): string {
  switch (cause) {
    case 'access':
      return 'sidebar.tree.refreshErrorAccess';
    case 'auth':
      return 'sidebar.tree.refreshErrorAuth';
    case 'other':
      return 'sidebar.tree.refreshError';
  }
}

/** The polite announcement when accounts start failing (or fail worse). */
export function refreshErrorAnnounceKey(cause: RefreshCause): string {
  switch (cause) {
    case 'access':
      return 'dialogs.accounts.refreshErrors.announceAccess';
    case 'auth':
      return 'dialogs.accounts.refreshErrors.announceAuth';
    case 'other':
      return 'dialogs.accounts.refreshErrors.announce';
  }
}
