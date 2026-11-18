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
  // What the core's check refuses at once (`feedback_check`).
  refuse: null as FeedbackFailure | null,
  checked: [] as unknown[],
  logsFail: false,
  sent: [] as unknown[],
  // When set, a send waits until the test calls `release`.
  hold: false,
  release: null as null | (() => void),
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
        if (backend.logsFail) return Promise.reject(new Error('unreadable'));
        const text = backend.logs[Math.min(backend.logsCalls, backend.logs.length - 1)];
        backend.logsCalls += 1;
        return Promise.resolve(text);
      }
      case 'feedback_check':
        backend.checked.push(args?.draft);
        return Promise.resolve(backend.refuse);
      case 'send_feedback':
        backend.sent.push(args?.draft);
        if (!backend.hold) return Promise.resolve(backend.outcome);
        return new Promise((resolve) => {
          backend.release = () => resolve(backend.outcome);
        });
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
import { Modal } from './Modal';

beforeEach(() => {
  backend.available = true;
  backend.logsCalls = 0;
  backend.outcome = { outcome: 'sent' };
  backend.refuse = null;
  backend.checked = [];
  backend.logsFail = false;
  backend.sent = [];
  backend.hold = false;
  backend.release = null;
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
    // Settled before the next test: the thanks comes after the dialog.
    await screen.findByText('Danke! Deine Nachricht ist angekommen.');
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
    await screen.findByText('Danke! Deine Nachricht ist angekommen.');
    expect(backend.sent).toEqual([
      { message: 'Mit Protokollen', email: null, logs: backend.logs[0] },
    ]);
    // The check gets the draft without the logs, which it does not read.
    expect(backend.checked).toEqual([{ message: 'Mit Protokollen', email: null, logs: null }]);
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
    expect(await screen.findByText(thanks)).toBeTruthy();
    expect(announced).toEqual([thanks]);
    expect((message as HTMLTextAreaElement).value).toBe('');
  });

  it('says why a send failed and puts focus on the field it is about', async () => {
    backend.refuse = 'invalid_email';
    await renderPanel();
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'x' },
    });
    // Exactly the label: the hint is the description, not part of the name.
    const email = screen.getByRole('textbox', { name: 'E-Mail-Adresse (freiwillig)' });
    expect(email.getAttribute('aria-describedby')).toBeTruthy();
    expect(
      document.getElementById(email.getAttribute('aria-describedby') ?? '')?.textContent,
    ).toMatch(/Antwortadresse/);
    fireEvent.change(email, { target: { value: 'kaputt' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    expect((await screen.findByRole('alert')).textContent).toMatch(/sieht nicht vollständig aus/);
    expect(document.activeElement).toBe(email);
    // Refused by the core's check, so nothing went and no dialog said so.
    expect(backend.checked).toEqual([{ message: 'x', email: 'kaputt', logs: null }]);
    expect(backend.sent).toEqual([]);
  });
});

describe('FeedbackPanel while a send is on its way', () => {
  it('keeps text typed during the send, which was not sent', async () => {
    backend.hold = true;
    await renderPanel();
    const message = screen.getByRole('textbox', { name: 'Nachricht' }) as HTMLTextAreaElement;
    fireEvent.change(message, { target: { value: 'Erster Teil' } });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    fireEvent.change(message, { target: { value: 'Erster Teil und mehr' } });
    await act(async () => {
      backend.release?.();
    });
    // Settled: the thanks comes once the wait dialog is gone.
    expect(await screen.findByText('Danke! Deine Nachricht ist angekommen.')).toBeTruthy();
    expect(backend.sent).toEqual([{ message: 'Erster Teil', email: null, logs: null }]);
    expect(message.value).toBe('Erster Teil und mehr');
  });

  it('says a failure that comes back after the panel is gone', async () => {
    backend.hold = true;
    backend.outcome = { outcome: 'failed', reason: 'network' };
    const { unmount } = render(<FeedbackPanel />);
    await screen.findByRole('button', { name: 'Senden' });
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'x' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    unmount();
    await act(async () => {
      backend.release?.();
    });
    await waitFor(() =>
      expect(announced).toEqual([
        'Der Feedback-Dienst ist nicht erreichbar. Prüf die Internetverbindung und versuch es noch einmal.',
      ]),
    );
  });

  it('shows a failure in the panel, not twice', async () => {
    backend.outcome = { outcome: 'failed', reason: 'busy' };
    await renderPanel();
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'x' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    expect((await screen.findByRole('alert')).textContent).toMatch(/zu viele Nachrichten/);
    expect(announced).toEqual([]);
  });

  // Toni, 2026-11-18: the button's word changing is not said, and a send with
  // the logs can take a minute; a dialog says it is under way.
  const waitDialog = () =>
    screen.queryByRole('dialog', { name: 'Feedback wird gesendet, bitte warten …' });

  it('shows a dialog while it is on its way, focused, gone before the thanks', async () => {
    backend.hold = true;
    await renderPanel();
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'Hallo' },
    });
    const send = screen.getByRole('button', { name: 'Senden' });
    send.focus();
    await act(async () => {
      fireEvent.click(send);
    });
    const dialog = await waitFor(() => {
      const found = waitDialog();
      expect(found).not.toBeNull();
      return found!;
    });
    await waitFor(() =>
      expect(document.activeElement?.textContent).toBe(
        'Aperio sendet deine Nachricht. Das dauert meist nur wenige Sekunden.',
      ),
    );
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(announced).toEqual([]);
    await act(async () => {
      backend.release?.();
    });
    // The moment the dialog is gone, focus is back on a button that says
    // "Send" again, not "Sending …, unavailable".
    await waitFor(() => expect(waitDialog()).toBeNull());
    const button = screen.getByRole('button', { name: 'Senden' });
    expect(button.getAttribute('aria-disabled')).toBeNull();
    const thanks = 'Danke! Deine Nachricht ist angekommen.';
    expect(await screen.findByText(thanks)).toBeTruthy();
    expect(announced).toEqual([thanks]);
    expect(document.activeElement).toBe(button);
  });

  it('shows no dialog for a draft the core refuses at once', async () => {
    backend.refuse = 'empty_message';
    await renderPanel();
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    expect((await screen.findByRole('alert')).textContent).toMatch(/Nachricht/);
    expect(waitDialog()).toBeNull();
    expect(backend.sent).toEqual([]);
    expect(document.activeElement).toBe(screen.getByRole('textbox', { name: 'Nachricht' }));
  });

  it('does not say the logs go along when they could not be read', async () => {
    backend.hold = true;
    backend.logsFail = true;
    await renderPanel({ initialAttachLogs: true });
    await waitFor(() =>
      expect(
        (screen.getByRole('textbox', { name: 'Diese Protokolle gehen mit' }) as HTMLTextAreaElement)
          .value,
      ).toMatch(/nicht gelesen/),
    );
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'Ohne' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    await waitFor(() => expect(waitDialog()?.textContent).toMatch(/wenige Sekunden/));
    expect(waitDialog()?.textContent).not.toMatch(/Protokollen/);
    await act(async () => {
      backend.release?.();
    });
    await waitFor(() => expect(waitDialog()).toBeNull());
    expect(backend.sent).toEqual([{ message: 'Ohne', email: null, logs: null }]);
  });

  it('keeps focus in the dialog when the window loses it, inside the Settings dialog', async () => {
    // The panel lives inside the Settings dialog; the wait dialog portals out
    // of it, but its focusout bubbles through Settings' focus net.
    backend.hold = true;
    render(
      <Modal isOpen onClose={() => {}} title="Einstellungen">
        <FeedbackPanel />
      </Modal>,
    );
    await screen.findByRole('button', { name: 'Senden' });
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'Hallo' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    const note = await waitFor(() => {
      const active = document.activeElement as HTMLElement | null;
      expect(active?.textContent).toMatch(/wenige Sekunden/);
      return active!;
    });
    // The window loses focus: focusout with nothing to go to, focus stays.
    fireEvent.focusOut(note, { relatedTarget: null });
    await act(async () => {
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    });
    expect(document.activeElement).toBe(note);
    await act(async () => {
      backend.release?.();
    });
    await waitFor(() => expect(waitDialog()).toBeNull());
  });

  it('says in the dialog that the logs make it longer', async () => {
    backend.hold = true;
    await renderPanel({ initialAttachLogs: true });
    const field = await screen.findByRole('textbox', { name: 'Diese Protokolle gehen mit' });
    await waitFor(() => expect((field as HTMLTextAreaElement).value).toBe(backend.logs[0]));
    fireEvent.change(screen.getByRole('textbox', { name: 'Nachricht' }), {
      target: { value: 'Mit Protokollen' },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Senden' }));
    });
    await waitFor(() => expect(waitDialog()?.textContent).toMatch(/mit den Protokollen/));
    await act(async () => {
      backend.release?.();
    });
    await waitFor(() => expect(waitDialog()).toBeNull());
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
