import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { FirstLaunchWizardChecker } from './FirstLaunchWizardChecker';

/**
 * Whether the first-launch wizard asks about Windows notifications: decided
 * before it opens, from the backend's own rule, and a failed answer leaves
 * the step out rather than the wizard.
 */

const h = vi.hoisted(() => {
  const backend = {
    status: { kind: 'not_set_up', needs_set_up: true } as unknown,
    statusFails: false,
  };
  const open = vi.fn();
  const invokeMock = vi.fn((cmd: string) => {
    switch (cmd) {
      case 'get_user_pref':
        return Promise.resolve(null);
      case 'is_fresh_instance':
        return Promise.resolve(true);
      case 'notification_shortcut_status':
        return backend.statusFails
          ? Promise.reject(new Error('no answer'))
          : Promise.resolve(backend.status);
      default:
        return Promise.resolve(null);
    }
  });
  return { backend, open, invokeMock };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: h.invokeMock }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ mode: { kind: 'none' }, openFirstLaunchWizard: h.open }),
}));

async function check() {
  render(<FirstLaunchWizardChecker />);
  await act(() => new Promise((resolve) => setTimeout(resolve, 20)));
}

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  h.open.mockReset();
  h.backend.statusFails = false;
});

describe('the first-launch wizard check', () => {
  it.each([
    ['not set up', { kind: 'not_set_up', needs_set_up: true }, true],
    ['a foreign entry', { kind: 'foreign', needs_set_up: true }, true],
    ['set up', { kind: 'set_up', needs_set_up: false }, false],
    ['another copy', { kind: 'points_elsewhere', target: 'x', needs_set_up: false }, false],
    ['a development build', { kind: 'dev_build', needs_set_up: false }, false],
    ['not Windows', { kind: 'unsupported', needs_set_up: false }, false],
  ])('%s: asks about notifications = %s', async (_name, status, step) => {
    h.backend.status = status;
    await check();
    expect(h.open).toHaveBeenCalledWith({ notificationStep: step });
  });

  it('still opens the wizard when the status cannot be read', async () => {
    h.backend.statusFails = true;
    await check();
    expect(h.open).toHaveBeenCalledWith({ notificationStep: false });
  });
});
