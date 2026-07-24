import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import {
  isCommandError,
  taskAddMember,
  taskListShares,
  taskRemoveMember,
  taskSearchUsers,
  taskSetMemberRight,
} from '../api/client';
import type {
  MemberRight,
  TaskCapabilities,
  TaskListShare,
  TaskUser,
} from '../api/types';
import { Modal } from './Modal';

const RIGHTS: MemberRight[] = ['read', 'write', 'admin'];

/**
 * Manage the membership/sharing of one task list (DESIGN §9.7): list
 * the current shares with their right + pending state, add/invite
 * people, remove them, and change roles. Capability-gated by the caller
 * — only opened for lists whose adapter declares `manageable`.
 *
 * The add control follows the adapter's `member_add_by`:
 *   - `search` (Vikunja): debounced user-directory search → pick a hit.
 *   - `email` (Todoist): type an email and send an invite (pending until
 *     accepted; Todoist has no directory + no roles).
 */
export function TaskMembersDialog({
  isOpen,
  onClose,
  listId,
  listName,
  capabilities,
}: {
  isOpen: boolean;
  onClose: () => void;
  listId: string;
  listName: string;
  capabilities?: TaskCapabilities;
}) {
  const addByEmail = capabilities?.member_add_by === 'email';
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const [shares, setShares] = useState<TaskListShare[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<TaskUser[]>([]);
  // Search lifecycle so the results area is never silent: idle (query too
  // short) → loading → done | error. Without this, an empty result set or
  // a failed request rendered nothing at all.
  const [searchState, setSearchState] = useState<
    'idle' | 'loading' | 'done' | 'error'
  >('idle');
  const [searchError, setSearchError] = useState('');
  const [busy, setBusy] = useState(false);
  // The add/search input is always mounted and never disabled — the stable
  // landing spot after an action removes/unmounts the control that had focus.
  const inputRef = useRef<HTMLInputElement>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      setShares(await taskListShares(listId));
      setError(null);
    } catch (err) {
      setError(formatErr(err));
    } finally {
      setLoading(false);
    }
  }, [listId]);

  useEffect(() => {
    if (isOpen) {
      setQuery('');
      setResults([]);
      setSearchState('idle');
      void reload();
    }
  }, [isOpen, reload]);

  // Debounced directory search (≥ 2 chars). Skipped entirely when the
  // adapter invites by email — there's no directory to query.
  useEffect(() => {
    if (!isOpen || addByEmail) return;
    const q = query.trim();
    if (q.length < 2) {
      setResults([]);
      setSearchState('idle');
      return;
    }
    let cancelled = false;
    setSearchState('loading');
    const handle = setTimeout(() => {
      void taskSearchUsers(listId, q)
        .then((r) => {
          if (cancelled) return;
          setResults(r);
          setSearchState('done');
        })
        .catch((err) => {
          if (cancelled) return;
          setResults([]);
          setSearchError(formatErr(err));
          setSearchState('error');
        });
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [isOpen, listId, query, addByEmail]);

  const run = useCallback(
    async (fn: () => Promise<void>) => {
      setBusy(true);
      try {
        await fn();
        await reload();
      } catch (err) {
        setError(formatErr(err));
      } finally {
        setBusy(false);
        // Remove/Add/Invite disable then unmount the clicked control (the row or
        // result is gone after reload), dropping focus to <body> — out of the
        // modal's role="application", so NVDA leaves application mode. If focus
        // was stranded, return it to the always-mounted add/search input.
        if (document.activeElement === document.body) {
          inputRef.current?.focus({ preventScroll: true });
        }
      }
    },
    [reload],
  );

  const add = (user: TaskUser) =>
    void run(async () => {
      // Default new members to write; the role dropdown can adjust it.
      await taskAddMember(listId, user.id, 'write');
      announce(t('dialogs.taskMembers.added', { name: user.name }));
      setQuery('');
      setResults([]);
    });

  // Email-invite path (Todoist): the typed text IS the member ref; there
  // are no roles, so `right` is null.
  const invite = () =>
    void run(async () => {
      const email = query.trim();
      if (!email) return;
      await taskAddMember(listId, email, null);
      announce(t('dialogs.taskMembers.added', { name: email }));
      setQuery('');
    });

  const remove = (share: TaskListShare) =>
    void run(async () => {
      await taskRemoveMember(listId, share.user.id);
      announce(t('dialogs.taskMembers.removed', { name: share.user.name }));
    });

  const changeRight = (share: TaskListShare, right: MemberRight) => {
    const prev = share.right;
    // Optimistic in place: update just this row's right so the <select>
    // keeps both its value and the focus. The old path went through
    // `run`, which flipped `busy` (disabling the focused select — NVDA
    // lost focus) and reloaded the whole list (rebuilding the rows, so
    // the value appeared to snap back to "read" until the refetch). We
    // mutate the single row and revert only on failure.
    setShares((rows) =>
      rows.map((r) => (r.user.id === share.user.id ? { ...r, right } : r)),
    );
    void taskSetMemberRight(listId, share.user.id, right)
      .then(() =>
        announce(
          t('dialogs.taskMembers.rightChanged', { name: share.user.name }),
        ),
      )
      .catch((err) => {
        setShares((rows) =>
          rows.map((r) =>
            r.user.id === share.user.id ? { ...r, right: prev } : r,
          ),
        );
        setError(formatErr(err));
      });
  };

  const existingIds = new Set(shares.map((s) => s.user.id));
  // Directory matches not already in the list — the actually-addable set.
  const addable = results.filter((u) => !existingIds.has(u.id));

  // Single status line for the search area (search-mode adapters only).
  // Never silent: it explains loading, failure, no-match (with the
  // Vikunja exact-username caveat) and the all-already-members case.
  let searchStatus = '';
  let searchStatusError = false;
  if (!addByEmail) {
    if (searchState === 'loading') {
      searchStatus = t('dialogs.taskMembers.searching');
    } else if (searchState === 'error') {
      searchStatus = t('dialogs.taskMembers.searchError', {
        error: searchError,
      });
      searchStatusError = true;
    } else if (searchState === 'done') {
      if (results.length === 0) {
        searchStatus = t('dialogs.taskMembers.noResults', {
          query: query.trim(),
        });
      } else if (addable.length === 0) {
        searchStatus = t('dialogs.taskMembers.allAlreadyMembers');
      } else {
        searchStatus = t('dialogs.taskMembers.searchResults', {
          count: addable.length,
        });
      }
    }
  }

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.taskMembers.title', { list: listName })}
      className="modal--form"
    >
      <div className="task-members">
        {error && (
          <p role="alert" className="form__error">
            {error}
          </p>
        )}
        {loading ? (
          <p className="form__hint" aria-live="polite">
            {t('dialogs.taskMembers.loading')}
          </p>
        ) : shares.length === 0 ? (
          <p className="form__hint">{t('dialogs.taskMembers.empty')}</p>
        ) : (
          <ul className="task-members__list">
            {shares.map((s) => (
              <li key={s.user.id} className="task-members__row">
                <span className="task-members__name">
                  {s.user.name}
                  {s.pending && (
                    <span className="task-members__pending">
                      {' · '}
                      {t('dialogs.taskMembers.pending')}
                    </span>
                  )}
                </span>
                {s.right !== null && (
                  <select
                    value={s.right}
                    disabled={busy}
                    onChange={(e) =>
                      changeRight(s, e.target.value as MemberRight)
                    }
                    aria-label={t('dialogs.taskMembers.rightFor', {
                      name: s.user.name,
                    })}
                  >
                    {RIGHTS.map((r) => (
                      <option key={r} value={r}>
                        {t(`dialogs.taskMembers.right.${r}`)}
                      </option>
                    ))}
                  </select>
                )}
                <button
                  type="button"
                  className="form__action"
                  disabled={busy}
                  onClick={() => remove(s)}
                >
                  {t('dialogs.taskMembers.remove')}
                </button>
              </li>
            ))}
          </ul>
        )}

        <div className="task-members__add">
          <label className="form__field">
            <span className="form__label">
              {t('dialogs.taskMembers.addLabel')}
            </span>
            <input
              ref={inputRef}
              type={addByEmail ? 'email' : 'text'}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t(
                addByEmail
                  ? 'dialogs.taskMembers.emailPlaceholder'
                  : 'dialogs.taskMembers.searchPlaceholder',
              )}
              autoComplete="off"
              onKeyDown={
                addByEmail
                  ? (e) => {
                      if (e.key === 'Enter') {
                        e.preventDefault();
                        invite();
                      }
                    }
                  : undefined
              }
            />
          </label>
          {addByEmail ? (
            <button
              type="button"
              className="form__action"
              disabled={busy || query.trim().length === 0}
              onClick={invite}
            >
              {t('dialogs.taskMembers.invite')}
            </button>
          ) : (
            <>
              {/* Always-present polite live region — collapses to nothing
                  when empty, so the search area is never silent yet adds
                  no gap while idle. */}
              <p
                role="status"
                aria-live="polite"
                className={
                  searchStatusError
                    ? 'task-members__search-status form__error'
                    : 'task-members__search-status form__hint'
                }
              >
                {searchStatus}
              </p>
              {addable.length > 0 && (
                <ul className="task-members__results">
                  {addable.map((u) => (
                    <li key={u.id}>
                      <button
                        type="button"
                        className="task-members__result"
                        disabled={busy}
                        onClick={() => add(u)}
                      >
                        {u.email ? `${u.name} · ${u.email}` : u.name}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}

function formatErr(err: unknown): string {
  if (isCommandError(err)) return `${err.code}: ${err.message}`;
  return String(err);
}
