import { describe, expect, it } from 'vitest';

import contract from '../../src-tauri/tests/fixtures/notificationShortcut.json';
import type { NotificationShortcutStatus } from '../api/client';
import i18n from '../i18n';
import { NOTIFICATION_SHORTCUT_STATUS_KEY } from '../state/useNotificationShortcut';

/**
 * The status of the Start menu shortcut, as the backend sends it: the same
 * fixture `notification_shortcut.rs` serialises against. Every state the
 * section shows has its sentence in both languages; read from each
 * language's own resources, because `t` would fall back to English, or to
 * the key itself, and still return text.
 */
describe('the Start menu shortcut status the backend sends', () => {
  const statuses = contract as NotificationShortcutStatus[];

  it('covers every state, with the set-up rule the backend decides', () => {
    expect(statuses.map((s) => s.kind)).toEqual([
      'unsupported',
      'dev_build',
      'not_set_up',
      'foreign',
      'set_up',
      'points_elsewhere',
    ]);
    expect(statuses.filter((s) => s.needs_set_up).map((s) => s.kind)).toEqual([
      'not_set_up',
      'foreign',
    ]);
  });

  it.each(['de', 'en'])('has a status sentence for every shown state in %s', (lng) => {
    for (const status of statuses) {
      if (status.kind === 'unsupported') continue;
      const key = NOTIFICATION_SHORTCUT_STATUS_KEY[status.kind];
      const text: unknown = i18n.getResource(lng, 'translation', key);
      expect(typeof text === 'string' && text.length > 0, `${lng}: ${key}`).toBe(true);
    }
  });
});
