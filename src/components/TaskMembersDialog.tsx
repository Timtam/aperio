import { useCallback, useEffect, useState } from 'react';
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
import type { MemberRight, TaskListShare, TaskUser } from '../api/types';
import { Modal } from './Modal';

const RIGHTS: MemberRight[] = ['read', 'write', 'admin'];

/**
 * Manage the membership/sharing of one task list (DESIGN §9.7): list
 * the current shares with their right, add/invite people via a search,
 * remove them, and change roles. Capability-gated by the caller — only
 * opened for lists whose adapter supports membership management.
 */
export function TaskMembersDialog({
  isOpen,
  onClose,
  listId,
  listName,
}: {
  isOpen: boolean;
  onClose: () => void;
  listId: string;
  listName: string;
}) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const [shares, setShares] = useState<TaskListShare[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<TaskUser[]>([]);
  const [busy, setBusy] = useState(false);

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
      void reload();
    }
  }, [isOpen, reload]);

  // Debounced directory search (≥ 2 chars).
  useEffect(() => {
    if (!isOpen) return;
    const q = query.trim();
    if (q.length < 2) {
      setResults([]);
      return;
    }
    let cancelled = false;
    const handle = setTimeout(() => {
      void taskSearchUsers(listId, q)
        .then((r) => {
          if (!cancelled) setResults(r);
        })
        .catch(() => {
          if (!cancelled) setResults([]);
        });
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(handle);
    };
  }, [isOpen, listId, query]);

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

  const remove = (share: TaskListShare) =>
    void run(async () => {
      await taskRemoveMember(listId, share.user.id);
      announce(t('dialogs.taskMembers.removed', { name: share.user.name }));
    });

  const changeRight = (share: TaskListShare, right: MemberRight) =>
    void run(async () => {
      await taskSetMemberRight(listId, share.user.id, right);
    });

  const existingIds = new Set(shares.map((s) => s.user.id));

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
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t('dialogs.taskMembers.searchPlaceholder')}
              autoComplete="off"
            />
          </label>
          {results.length > 0 && (
            <ul className="task-members__results">
              {results
                .filter((u) => !existingIds.has(u.id))
                .map((u) => (
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
        </div>
      </div>
    </Modal>
  );
}

function formatErr(err: unknown): string {
  if (isCommandError(err)) return `${err.code}: ${err.message}`;
  return String(err);
}
