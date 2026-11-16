import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

/**
 * "Send logs" in Settings → Logs opens the feedback tab with its logs box
 * ticked, in the same dialog; reaching the tab through the tab list opens the
 * plain form.
 */

const invokeMock = vi.hoisted(() =>
  vi.fn((cmd: string) => {
    switch (cmd) {
      case 'feedback_info':
        return Promise.resolve({ available: true, system: 'Aperio 0.2.0' });
      case 'feedback_logs':
        return Promise.resolve('a log line\n');
      case 'get_log_level':
        return Promise.resolve('info');
      case 'logs_dir_path':
        return Promise.resolve('C:/logs');
      case 'get_recent_logs':
        return Promise.resolve('');
      default:
        return Promise.resolve(null);
    }
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: () => Promise.resolve(null) }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => () => {} }));
vi.mock('../state/dialogStateContext', () => ({
  useDialogState: () => ({ recordSettingsTab: () => {} }),
}));
// The other tabs' panels are not under test and need stores of their own.
for (const name of [
  'AccountsPanel',
  'CalendarsPanel',
  'ColorLabelsPanel',
  'DayMarkersPanel',
  'SignaturesPanel',
  'ContactsPanel',
  'GeneralPanel',
  'PluginsPanel',
  'SyncPanel',
  'TasksPanel',
]) {
  vi.doMock(`./${name}`, () => ({ [name]: () => null }));
}

const { SettingsDialog } = await import('./SettingsDialog');

afterEach(() => {
  document.body.innerHTML = '';
});

describe('Settings: Send logs', () => {
  it('opens the feedback tab with the logs ticked, and the tab list opens it plain', async () => {
    await act(async () => {
      render(<SettingsDialog isOpen onClose={() => {}} initialTab="logs" />);
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Protokolle senden …' }));
    });

    const feedbackTab = screen.getByRole('tab', { name: 'Feedback' });
    expect(feedbackTab.getAttribute('aria-selected')).toBe('true');
    expect(document.activeElement).toBe(feedbackTab);
    const box = (await screen.findByRole('checkbox', {
      name: 'Protokolle anhängen',
    })) as HTMLInputElement;
    expect(box.checked).toBe(true);

    // Away and back through the tab list: the plain form.
    await act(async () => {
      fireEvent.click(screen.getByRole('tab', { name: 'Protokolle' }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('tab', { name: 'Feedback' }));
    });
    const again = (await screen.findByRole('checkbox', {
      name: 'Protokolle anhängen',
    })) as HTMLInputElement;
    expect(again.checked).toBe(false);
  });
});
