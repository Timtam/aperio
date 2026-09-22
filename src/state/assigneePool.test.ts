import { describe, expect, it } from 'vitest';

import {
  assigneeField,
  assigneePoolErrorMessage,
  readRefusal,
  type AssigneePool,
} from '@aperio/shared';

/** Echoes the key and its values, so a test sees which sentence was chosen. */
const t = (key: string, values?: Record<string, unknown>) =>
  `${key}${values ? ` ${JSON.stringify(values)}` : ''}`;

const ready = (n: number): AssigneePool => ({
  status: 'ready',
  members: Array.from({ length: n }, (_, i) => ({ id: String(i), name: `p${i}`, email: null })),
});

describe('assigneeField', () => {
  it('hides the field only where the list cannot hold assignees', () => {
    expect(assigneeField('none', ready(2))).toBe('hidden');
    expect(assigneeField('none', { status: 'failed', error: new Error('x') })).toBe('hidden');
  });

  it('never hides it because the read failed or came back empty (130)', () => {
    expect(assigneeField('multiple', { status: 'loading' })).toBe('loading');
    expect(assigneeField('multiple', { status: 'failed', error: new Error('x') })).toBe('failed');
    expect(assigneeField('single', ready(0))).toBe('empty');
    expect(assigneeField('single', ready(1))).toBe('picker');
  });
});

describe('assigneePoolErrorMessage', () => {
  it('names the permission a refused token lacks', () => {
    const err = { code: 'forbidden', message: 'token-refused: users search (projects)' };
    expect(readRefusal(err)?.refusal).toBe('token-refused');
    expect(assigneePoolErrorMessage(err, t)).toBe(
      'dialogs.task.assignees.tokenRefused {"permission":"users search (projects)"}',
    );
  });

  it("reads the token on the phone, behind Expo's wrapper", () => {
    // Android: Expo keeps the code and puts its sentence before the message.
    const android = {
      code: 'forbidden',
      message:
        "Call to function 'CalFfi.taskListMembersJson' has been rejected.\n\u2192 Caused by: token-refused: users search (projects)",
    };
    // iOS: Expo drops the code and describes the cause.
    const ios = new Error(
      "Calling the 'taskListMembersJson' function has failed\n\u2192 Caused by: token-refused: users search (projects)",
    );
    for (const err of [android, ios]) {
      expect(assigneePoolErrorMessage(err, t)).toBe(
        'dialogs.task.assignees.tokenRefused {"permission":"users search (projects)"}',
      );
    }
    // Any other failure is said without the wrapper too.
    expect(
      assigneePoolErrorMessage(
        { code: 'network', message: "Call to function 'x' has been rejected.\n\u2192 Caused by: offline" },
        t,
      ),
    ).toBe('dialogs.task.assignees.loadFailed {"detail":"network: offline"}');
  });

  it('reads the token from any error that carries it', () => {
    expect(assigneePoolErrorMessage(new Error('token-refused: x'), t)).toBe(
      'dialogs.task.assignees.tokenRefused {"permission":"x"}',
    );
  });

  it('says any other failure with what the host said, never swallowing it', () => {
    expect(assigneePoolErrorMessage({ code: 'network', message: 'offline' }, t)).toBe(
      'dialogs.task.assignees.loadFailed {"detail":"network: offline"}',
    );
    expect(assigneePoolErrorMessage(new Error('boom'), t)).toBe(
      'dialogs.task.assignees.loadFailed {"detail":"boom"}',
    );
    // A word that merely starts like a token is not one.
    expect(readRefusal({ code: 'forbidden', message: 'token-refusedx' })).toBeNull();
  });
});
