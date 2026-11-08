import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ReminderTapListener } from './ReminderTapListener';

/**
 * When a click on a reminder's notification opens what it was for (TODO A5):
 * once Aperio's window has focus, the calendars are listed and nothing holds
 * the user. An open editor makes it wait, say so once and open after; the
 * reminders overview stays under it; the day-start review gives way.
 */

const EVENT_TAP = {
  itemId: 'ev-1',
  itemKind: 'event',
  containerId: 'work',
  start: '2026-11-16T09:00:00+00:00',
  title: 'Standup',
};
const ROW = { id: 'ev-1', calendar_id: 'work', title: 'Standup' };

const h = vi.hoisted(() => {
  const dialog = {
    mode: { kind: 'none' } as { kind: string },
    openTappedEvent: vi.fn(),
    openTaskDialog: vi.fn(),
    openDayStartReview: vi.fn(),
  };
  const store = { calendars: [] as unknown[], loading: false };
  const backend = { waiting: [] as unknown[] };
  const invokeMock = vi.fn((cmd: string) => {
    if (cmd === 'take_reminder_tap') return Promise.resolve(backend.waiting.shift() ?? null);
    return Promise.resolve(null);
  });
  const events: { tap: (() => void) | null; focus: ((e: { payload: boolean }) => void) | null } = {
    tap: null,
    focus: null,
  };
  const win = { focused: true };
  const announce = vi.fn();
  const resolve = vi.fn();
  // One stable `t`, as the real hook gives: the listener re-runs on a new one.
  const translation = { t: (key: string) => key };
  return { dialog, store, backend, invokeMock, events, win, announce, resolve, translation };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: h.invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({
  listen: (name: string, handler: () => void) => {
    if (name === 'reminder-tap') h.events.tap = handler;
    return Promise.resolve(() => {});
  },
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isFocused: () => Promise.resolve(h.win.focused),
    onFocusChanged: (handler: (e: { payload: boolean }) => void) => {
      h.events.focus = handler;
      return Promise.resolve(() => {});
    },
  }),
}));
vi.mock('../state/dialogStateContext', () => ({ useDialogState: () => h.dialog }));
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => h.store }));
vi.mock('../a11y/announcerContext', () => ({ useAnnouncer: () => h.announce }));
vi.mock('../state/reminderTapTarget', () => ({ resolveReminderTap: h.resolve }));
vi.mock('react-i18next', () => ({ useTranslation: () => h.translation }));

const settle = () => act(() => new Promise((resolve) => setTimeout(resolve, 20)));

async function mount() {
  const view = render(<ReminderTapListener />);
  await settle();
  return view;
}

/** A click: the backend keeps the tap and says so. */
async function click(data: unknown = EVENT_TAP) {
  h.backend.waiting.push(data);
  await act(async () => h.events.tap?.());
  await settle();
}

// Braces: a function returned from beforeEach runs as its teardown.
beforeEach(() => {
  h.dialog.mode = { kind: 'none' };
  h.dialog.openTappedEvent.mockClear();
  h.dialog.openTaskDialog.mockClear();
  h.dialog.openDayStartReview.mockClear();
  h.store.calendars = [];
  h.store.loading = false;
  h.backend.waiting = [];
  h.invokeMock.mockClear();
  h.events.tap = null;
  h.events.focus = null;
  h.win.focused = true;
  h.announce.mockClear();
  h.resolve.mockReset();
  h.resolve.mockImplementation((tap: { kind: string }) =>
    Promise.resolve(
      tap.kind === 'dayStart'
        ? { kind: 'dayStart' }
        : { kind: 'event', event: ROW, shownOccurrence: undefined, initialScope: undefined },
    ),
  );
});

const opened = () => h.dialog.openTappedEvent.mock.calls.length;
const announced = () => h.announce.mock.calls.map((c) => c[0] as string);

describe('a click on a reminder', () => {
  it('opens its event, and the tap is gone after', async () => {
    const view = await mount();
    await click();
    expect(h.dialog.openTappedEvent).toHaveBeenCalledWith(ROW, {
      initialScope: undefined,
      shownOccurrence: undefined,
    });
    view.rerender(<ReminderTapListener />);
    await settle();
    expect(opened()).toBe(1);
  });

  it('opens a tap made before it listened', async () => {
    h.backend.waiting.push(EVENT_TAP);
    await mount();
    expect(opened()).toBe(1);
  });

  it('a remount does not open it again', async () => {
    const first = await mount();
    await click();
    first.unmount();
    await mount();
    expect(opened()).toBe(1);
  });

  it('ignores data that is no reminder', async () => {
    await mount();
    await click({ something: 'else' });
    expect(h.resolve).not.toHaveBeenCalled();
    expect(opened()).toBe(0);
  });

  it('waits for Aperio to have focus', async () => {
    h.win.focused = false;
    await mount();
    await click();
    expect(opened()).toBe(0);
    await act(async () => h.events.focus?.({ payload: true }));
    await settle();
    expect(opened()).toBe(1);
  });

  it('waits for the calendars to be listed', async () => {
    h.store.loading = true;
    const view = await mount();
    await click();
    expect(h.resolve).not.toHaveBeenCalled();
    h.store.loading = false;
    view.rerender(<ReminderTapListener />);
    await settle();
    expect(opened()).toBe(1);
  });

  it('waits for an open editor, says so once, and opens once it closed', async () => {
    h.dialog.mode = { kind: 'event' };
    const view = await mount();
    await click();
    expect(opened()).toBe(0);
    expect(announced()).toEqual(['dialogs.reminders.tapWaits']);
    view.rerender(<ReminderTapListener />);
    await settle();
    expect(announced()).toHaveLength(1);
    h.dialog.mode = { kind: 'none' };
    view.rerender(<ReminderTapListener />);
    await settle();
    expect(opened()).toBe(1);
  });

  it('waits while a text field is in use, and says it is the field', async () => {
    const field = document.createElement('input');
    const button = document.createElement('button');
    document.body.append(field, button);
    field.focus();
    await mount();
    await click();
    expect(opened()).toBe(0);
    expect(announced()).toEqual(['dialogs.reminders.tapWaitsField']);
    await act(async () => button.focus());
    await settle();
    expect(opened()).toBe(1);
    field.remove();
    button.remove();
  });

  it('opens on top of the reminders overview, without waiting', async () => {
    h.dialog.mode = { kind: 'reminders' };
    await mount();
    await click();
    expect(opened()).toBe(1);
    expect(announced()).toEqual([]);
  });

  it('opens on top of the day-start review, never in its place', async () => {
    h.dialog.mode = { kind: 'dayStartReview' };
    await mount();
    await click();
    expect(opened()).toBe(1);
    expect(h.dialog.openTappedEvent.mock.calls[0][1]).not.toHaveProperty('replace');
  });

  it('a review that opened while the tap waited stays under it', async () => {
    h.dialog.mode = { kind: 'event' };
    const view = await mount();
    await click();
    expect(opened()).toBe(0);
    h.dialog.mode = { kind: 'dayStartReview' };
    view.rerender(<ReminderTapListener />);
    await settle();
    expect(opened()).toBe(1);
    expect(h.dialog.openTappedEvent.mock.calls[0][1]).not.toHaveProperty('replace');
  });

  it('a day-start click opens the review, but not a second one', async () => {
    const view = await mount();
    await click({ dayStart: true });
    expect(h.dialog.openDayStartReview).toHaveBeenCalledTimes(1);
    h.dialog.mode = { kind: 'dayStartReview' };
    view.rerender(<ReminderTapListener />);
    await click({ dayStart: true });
    expect(h.dialog.openDayStartReview).toHaveBeenCalledTimes(1);
  });

  it('a task opens its editor', async () => {
    const task = { id: 't-1', list_id: 'l' };
    h.resolve.mockResolvedValue({ kind: 'task', task });
    await mount();
    await click({ itemId: 't-1', itemKind: 'task', containerId: 'l', start: EVENT_TAP.start, title: 'x' });
    expect(h.dialog.openTaskDialog).toHaveBeenCalledWith(task);
  });

  it('says so when the item is gone, and opens nothing', async () => {
    h.resolve.mockResolvedValue({ kind: 'notFound' });
    await mount();
    await click();
    expect(opened()).toBe(0);
    expect(announced()).toEqual(['dialogs.reminders.tapNotFound']);
  });
});
