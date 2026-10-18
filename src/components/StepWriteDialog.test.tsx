import { useEffect } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

import { shownInNotice } from '@aperio/shared';

import type { DialogStateValue } from '../state/DialogState';

const { store } = vi.hoisted(() => ({ store: { calendars: [] } }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('../state/calendarStoreContext', () => ({ useCalendarStore: () => store }));

import { DialogStateProvider } from '../state/DialogState';
import { useDialogState } from '../state/dialogStateContext';
import { StepWriteDialog } from './StepWriteWait';

/**
 * The "please wait" dialog of a surface without a dialog of its own — a drop
 * in the sidebar, Ctrl+D (decisions 227, 265) —, from the dialog stack: open
 * while the write runs, gone when it is done, and the Close-only notice of the
 * failure when it is not (210).
 */

let api: DialogStateValue | null = null;

function Probe() {
  const state = useDialogState();
  useEffect(() => {
    api = state;
  });
  return state.mode.kind === 'stepWrite' ? (
    <StepWriteDialog notice={state.mode.notice} onClose={state.close} />
  ) : null;
}

function mount() {
  render(
    <div id="app-root">
      <DialogStateProvider>
        <Probe />
      </DialogStateProvider>
    </div>,
  );
}

const waiting = () => screen.queryByRole('dialog', { name: /Vorgang läuft|please wait/i });

afterEach(() => {
  document.body.innerHTML = '';
  api = null;
});

describe('the dialog stack\'s wait for a write in steps', () => {
  it('shows nothing for a write that does not run in steps', async () => {
    mount();
    const wait = api!.stepWriteWait(() => 'never');
    await act(async () => {
      await expect(wait(false, async () => 7)).resolves.toBe(7);
    });
    expect(waiting()).toBeNull();
  });

  it('waits without a button and is gone when the write is done', async () => {
    mount();
    let finish = () => {};
    const write = new Promise<string>((resolve) => {
      finish = () => resolve('written');
    });
    let result: Promise<string> = Promise.resolve('');
    act(() => {
      result = api!.stepWriteWait(() => 'never')(true, () => write);
    });
    const dialog = await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    expect(dialog.querySelectorAll('button')).toHaveLength(0);
    await act(async () => {
      finish();
      await expect(result).resolves.toBe('written');
    });
    await waitFor(() => expect(waiting()).toBeNull());
  });

  it('turns into the failure, focused and only closed, and marks it said', async () => {
    mount();
    let fail = () => {};
    const write = new Promise<string>((_resolve, reject) => {
      fail = () => reject(new Error('created-may-stand: x'));
    });
    let result: Promise<string> = Promise.resolve('');
    act(() => {
      result = api!.stepWriteWait(() => 'Steht möglicherweise doppelt.')(true, () => write);
    });
    await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    let caught: unknown = null;
    await act(async () => {
      fail();
      await result.catch((err: unknown) => {
        caught = err;
      });
    });
    expect(shownInNotice(caught)).toBe(true);
    const notice = await screen.findByRole('dialog', { name: /Nicht abgeschlossen|Not completed/ });
    const sentence = screen.getByText('Steht möglicherweise doppelt.');
    await waitFor(() => expect(document.activeElement).toBe(sentence));
    expect(waiting()).toBeNull();
    // Its own Close (the header's × is named in English).
    fireEvent.click(screen.getByRole('button', { name: /^schließen$/i }));
    await waitFor(() => expect(notice.isConnected).toBe(false));
  });

  it('takes only its own frame away when a dialog opened above it', async () => {
    mount();
    let finish = () => {};
    const write = new Promise<string>((resolve) => {
      finish = () => resolve('written');
    });
    let result: Promise<string> = Promise.resolve('');
    act(() => {
      result = api!.stepWriteWait(() => 'never')(true, () => write);
    });
    await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    act(() => api!.openSearch());
    await waitFor(() => expect(api!.mode.kind).toBe('search'));
    await act(async () => {
      finish();
      await expect(result).resolves.toBe('written');
    });
    // The dialog on top stays open; closing it leaves nothing behind.
    await waitFor(() => expect(api!.mode.kind).toBe('search'));
    act(() => api!.close());
    await waitFor(() => expect(api!.mode.kind).toBe('none'));
    expect(waiting()).toBeNull();
  });

  it('turns into its notice under a dialog opened above it', async () => {
    mount();
    let fail = () => {};
    const write = new Promise<string>((_resolve, reject) => {
      fail = () => reject(new Error('created-may-stand: x'));
    });
    let result: Promise<string> = Promise.resolve('');
    act(() => {
      result = api!.stepWriteWait(() => 'Steht möglicherweise doppelt.')(true, () => write);
    });
    await screen.findByRole('dialog', { name: /Vorgang läuft|please wait/i });
    act(() => api!.openSearch());
    await waitFor(() => expect(api!.mode.kind).toBe('search'));
    await act(async () => {
      fail();
      await result.catch(() => {});
    });
    expect(api!.mode.kind).toBe('search');
    // Closing the dialog on top shows the notice, not the wait.
    act(() => api!.close());
    await screen.findByRole('dialog', { name: /Nicht abgeschlossen|Not completed/ });
    expect(screen.getByText('Steht möglicherweise doppelt.')).toBeTruthy();
    expect(waiting()).toBeNull();
  });
});
