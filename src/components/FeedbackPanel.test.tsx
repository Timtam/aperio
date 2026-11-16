import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import {
  feedbackFailureField,
  feedbackOutcomeSentence,
  type FeedbackFailure,
  type FeedbackOutcome,
} from '@aperio/shared';

/**
 * Settings → Feedback (host_core::feedback). What the user reads is what is
 * sent: the logs shown in the field are the text the draft carries, read
 * once. Nothing is sent before the button, and a keyless build shows a note
 * instead of a form that fails on the button.
 */

const backend = vi.hoisted(() => ({
  available: true,
  logs: ['first read\nme@example.org → [redacted-email]\n', 'second read\n'],
  logsCalls: 0,
  outcome: { outcome: 'sent' } as FeedbackOutcome,
  sent: [] as unknown[],
}));

const invokeMock = vi.hoisted(() =>
  vi.fn((cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'feedback_info':
        return Promise.resolve({
          available: backend.available,
          system: 'Aperio 0.2.0 · Windows 11 Pro 25H2 (build 26220.9836) · x86_64',
        });
      case 'feedback_logs': {
        const text = backend.logs[Math.min(backend.logsCalls, backend.logs.length - 1)];
        backend.logsCalls += 1;
        return Promise.resolve(text);
      }
      case 'send_feedback':
        backend.sent.push(args?.draft);
        return Promise.resolve(backend.outcome);
      default:
        return Promise.resolve(null);
    }
  }),
);
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
const announced = vi.hoisted(() => [] as string[]);
vi.mock('../a11y/announcerContext', () => ({
  useAnnouncer: () => (text: string) => announced.push(text),
}));

import de from '../../locales/de/translation.json';
import en from '../../locales/en/translation.json';
import { FeedbackPanel } from './FeedbackPanel';

beforeEach(() => {
  backend.available = true;
  backend.logsCalls = 0;
  backend.outcome = { outcome: 'sent' };
  backend.sent = [];
  announced.length = 0;
  invokeMock.mockClear();
});

afterEach(() => {
  document.body.innerHTML = '';
});

async function renderPanel(props: { initialAttachLogs?: boolean } = {}) {
  render(<FeedbackPanel {...props} />);
  await screen.findByRole('button', { name: 'Senden' });
}

describe('FeedbackPanel', () => {
  it('shows a note and no form in a build without the key', async () => {
    backend.available = false;
    render(<FeedbackPanel />);
    expect(await screen.findByText(/kann kein Feedback senden/)).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Senden' })).toBeNull();
    expect(screen.queryByRole('textbox', { name: 'Nachricht' })).toBeNull();
  });

  it('shows the system line that goes along', async () => {
    await renderPanel();
    expect(
      screen.getByText('Geht immer mit: Aperio 0.2.0 · Windows 11 Pro 25H2 (build 26220.9836) · x86_64'),
    ).toBeTruthy();
  });

  it('reads no logs and sends none unless asked', async () => {
    await renderPanel();
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'Hallo' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    expect(invokeMock).not.toHaveBeenCalledWith('feedback_logs');
    expect(backend.sent).toEqual([{ message: 'Hallo', email: null, logs: null }]);
  });

  it('sends the very log text it showed, read once', async () => {
    await renderPanel();
    fireEvent.click(screen.getByRole('checkbox', { name: 'Protokolle anhängen' }));
    const field = await screen.findByRole('textbox', { name: 'Diese Protokolle gehen mit' });
    await waitFor(() =>
      expect((field as HTMLTextAreaElement).value).toBe(backend.logs[0]),
    );

    // Off and on again: the same text, not a second read.
    fireEvent.click(screen.getByRole('checkbox', { name: 'Protokolle anhängen' }));
    fireEvent.click(screen.getByRole('checkbox', { name: 'Protokolle anhängen' }));
    expect(backend.logsCalls).toBe(1);

    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'Mit Protokollen' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    expect(backend.sent).toEqual([
      { message: 'Mit Protokollen', email: null, logs: backend.logs[0] },
    ]);
  });

  it('opens with the logs ticked from "Send logs"', async () => {
    await renderPanel({ initialAttachLogs: true });
    expect(
      (screen.getByRole('checkbox', { name: 'Protokolle anhängen' }) as HTMLInputElement)
        .checked,
    ).toBe(true);
    const field = await screen.findByRole('textbox', { name: 'Diese Protokolle gehen mit' });
    await waitFor(() =>
      expect((field as HTMLTextAreaElement).value).toBe(backend.logs[0]),
    );
  });

  it('thanks the user on screen and aloud, and clears the message', async () => {
    await renderPanel();
    const message = screen.getByRole('textbox', { name: 'Nachricht' });
    fireEvent.change(message, { target: { value: 'Danke' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    const thanks = 'Danke! Deine Nachricht ist angekommen.';
    expect(screen.getByText(thanks)).toBeTruthy();
    expect(announced).toEqual([thanks]);
    expect((message as HTMLTextAreaElement).value).toBe('');
  });

  it('says why a send failed and puts focus on the field it is about', async () => {
    backend.outcome = { outcome: 'failed', reason: 'invalid_email' };
    await renderPanel();
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'x' },
    });
    const email = screen.getByRole('textbox', { name: /E-Mail-Adresse/ });
    fireEvent.change(email, { target: { value: 'kaputt' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    expect(screen.getByRole('alert').textContent).toMatch(/sieht nicht vollständig aus/);
    expect(document.activeElement).toBe(email);
    expect(backend.sent).toEqual([{ message: 'x', email: 'kaputt', logs: null }]);
  });
});

describe('feedback outcome sentences', () => {
  const t = (key: string) => key;
  const reasons: FeedbackFailure[] = [
    'unavailable',
    'empty_message',
    'invalid_email',
    'too_large',
    'network',
    'busy',
    'service',
    'rejected',
  ];

  it('gives every reason a sentence of its own that exists in both languages', () => {
    const lookup = (tree: Record<string, unknown>, key: string) =>
      key.split('.').reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], tree);
    const keys = reasons.map((reason) => feedbackOutcomeSentence({ outcome: 'failed', reason }, t));
    expect(new Set(keys).size).toBe(reasons.length);
    for (const key of [...keys, feedbackOutcomeSentence({ outcome: 'sent' }, t)]) {
      expect(typeof lookup(de as Record<string, unknown>, key), key).toBe('string');
      expect(typeof lookup(en as Record<string, unknown>, key), key).toBe('string');
    }
  });

  it('names the field a failure is about', () => {
    expect(feedbackFailureField({ outcome: 'failed', reason: 'empty_message' })).toBe('message');
    expect(feedbackFailureField({ outcome: 'failed', reason: 'too_large' })).toBe('message');
    expect(feedbackFailureField({ outcome: 'failed', reason: 'invalid_email' })).toBe('email');
    expect(feedbackFailureField({ outcome: 'failed', reason: 'network' })).toBeNull();
    expect(feedbackFailureField({ outcome: 'sent' })).toBeNull();
  });
});
