// The people a task can be assigned to, as an editor holds them.
//
// The pool is read from the provider whenever an editor opens a list. It used
// to become an empty list when that read failed, and an empty list hid the
// "Assigned to" field: a Vikunja token made before 2.4, which lacks the
// permission the read needs, made every picker vanish without a word
// (decision 130). Now the field is there whenever the list can hold assignees
// at all, and it says what it knows: still loading, could not be read and why,
// nobody to assign, or the picker. Both editors ask the same two functions.

import { codedError, errorMessageText } from './eventWriteError';
import type { ReadRefusal } from './generated/ReadRefusal';
import type { TaskAssignment } from './generated/TaskAssignment';
import type { TaskUser } from './generated/TaskUser';

/** Where the read of a list's people stands. */
export type AssigneePool =
  | { status: 'loading' }
  | { status: 'failed'; error: unknown }
  | { status: 'ready'; members: TaskUser[] };

/** What the "Assigned to" field shows. */
export type AssigneeField = 'hidden' | 'loading' | 'failed' | 'empty' | 'picker';

/**
 * What the field shows for a list that holds `mode` assignees, with `pool` as
 * it stands. Hidden only where the source has no notion of assigning; never
 * because a read failed or came back empty.
 */
export function assigneeField(mode: TaskAssignment, pool: AssigneePool): AssigneeField {
  if (mode === 'none') return 'hidden';
  if (pool.status === 'loading') return 'loading';
  if (pool.status === 'failed') return 'failed';
  return pool.members.length === 0 ? 'empty' : 'picker';
}

/** The sentence each read refusal reads as. Typed by the generated union, so a
 *  new refusal in the core is a compile error here. */
const READ_REFUSAL_KEYS: Record<ReadRefusal, string> = {
  'token-refused': 'dialogs.task.assignees.tokenRefused',
};

const READ_TOKENS = Object.keys(READ_REFUSAL_KEYS) as ReadRefusal[];

/**
 * The read refusal a message names (`cal_core::ReadRefusal`), whatever error
 * carried it: the token at the start, the provider's detail after the colon.
 */
export function readRefusal(
  err: unknown,
): { refusal: ReadRefusal; key: string; detail: string } | null {
  const message = errorMessageText(err).trim();
  for (const refusal of READ_TOKENS) {
    if (!message.startsWith(refusal)) continue;
    const rest = message.slice(refusal.length);
    if (rest === '') return { refusal, key: READ_REFUSAL_KEYS[refusal], detail: '' };
    if (rest.startsWith(':')) {
      return { refusal, key: READ_REFUSAL_KEYS[refusal], detail: rest.slice(1).trim() };
    }
  }
  return null;
}

type Translate = (key: string, values?: Record<string, unknown>) => string;

/**
 * Why a list's people could not be read, in the reader's language: a refused
 * token names the permission the read needs; anything else says it failed,
 * with what the host said, so nothing is swallowed.
 */
export function assigneePoolErrorMessage(err: unknown, t: Translate): string {
  const refusal = readRefusal(err);
  if (refusal) return t(refusal.key, { permission: refusal.detail });
  const known = codedError(err);
  return t('dialogs.task.assignees.loadFailed', {
    detail: known ? `${known.code}: ${known.message}` : errorMessageText(err),
  });
}
