import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useEffect, useRef } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { ToastProvider } from './ToastProvider';
import { useToast, type ToastInput } from './toastContext';

// Minimal i18n shim — the provider reaches for translated strings via
// react-i18next's `useTranslation`. Vitest's jsdom environment doesn't
// boot the real i18n stack, so we feed it the keys verbatim. Asserting
// against the raw key (`toast.regionLabel`) is enough — the production
// runtime substitutes the real translation.
vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string) => key,
  }),
}));

/** Helper that fires off a toast as a side-effect on mount. Lets us
 *  drive the provider without a separate clickable button — saves
 *  noise in tests that don't care about the trigger surface. */
function Trigger({ input }: { input: ToastInput }) {
  const { showToast } = useToast();
  const firedRef = useRef(false);
  useEffect(() => {
    if (firedRef.current) return;
    firedRef.current = true;
    showToast(input);
  }, [input, showToast]);
  return null;
}

describe('ToastProvider', () => {
  it('renders a toast with the supplied message', async () => {
    render(
      <ToastProvider>
        <Trigger input={{ message: 'Five tasks moved' }} />
      </ToastProvider>,
    );
    await waitFor(() =>
      expect(screen.getByText('Five tasks moved')).toBeInTheDocument(),
    );
  });

  it('runs the undo action and dismisses on click', async () => {
    const undo = vi.fn().mockResolvedValue(undefined);
    render(
      <ToastProvider>
        <Trigger
          input={{
            message: 'Moved',
            undo: { action: undo },
          }}
        />
      </ToastProvider>,
    );
    const button = await screen.findByRole('button', {
      name: 'toast.undoLabel',
    });
    await act(async () => {
      fireEvent.click(button);
    });
    expect(undo).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('Moved')).not.toBeInTheDocument();
  });

  it('keeps the toast visible when the undo action throws', async () => {
    const undo = vi.fn().mockRejectedValue(new Error('network'));
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
    render(
      <ToastProvider>
        <Trigger
          input={{
            message: 'Moved',
            undo: { action: undo },
          }}
        />
      </ToastProvider>,
    );
    const button = await screen.findByRole('button', {
      name: 'toast.undoLabel',
    });
    await act(async () => {
      fireEvent.click(button);
    });
    expect(undo).toHaveBeenCalledTimes(1);
    // Failure keeps the toast up so the user can retry.
    expect(screen.getByText('Moved')).toBeInTheDocument();
    // Button re-enabled — `aria-disabled` cleared after the rejection.
    expect(button).not.toHaveAttribute('aria-disabled', 'true');
    warnSpy.mockRestore();
  });

  it('auto-dismisses after the supplied duration', async () => {
    vi.useFakeTimers();
    try {
      render(
        <ToastProvider>
          <Trigger input={{ message: 'Brief', durationMs: 1000 }} />
        </ToastProvider>,
      );
      // The trigger fires inside a `useEffect`, which jsdom + fake
      // timers schedules via a microtask. Flush it before checking.
      await act(async () => {
        await Promise.resolve();
      });
      expect(screen.getByText('Brief')).toBeInTheDocument();
      act(() => {
        vi.advanceTimersByTime(1100);
      });
      expect(screen.queryByText('Brief')).not.toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('dismisses when the × button is clicked', async () => {
    render(
      <ToastProvider>
        <Trigger input={{ message: 'Dismissable' }} />
      </ToastProvider>,
    );
    const dismiss = await screen.findByRole('button', {
      name: 'toast.dismissLabel',
    });
    fireEvent.click(dismiss);
    expect(screen.queryByText('Dismissable')).not.toBeInTheDocument();
  });

  it('dismisses on Escape when focus is inside the toast', async () => {
    render(
      <ToastProvider>
        <Trigger input={{ message: 'EscMe', undo: { action: vi.fn() } }} />
      </ToastProvider>,
    );
    const button = await screen.findByRole('button', {
      name: 'toast.undoLabel',
    });
    // The Undo button focus triggers a React state update (the
    // toast's onFocus handler flips `focusInside`). Wrap in act so
    // React commits the update before we send the Escape.
    act(() => {
      button.focus();
    });
    fireEvent.keyDown(button, { key: 'Escape' });
    expect(screen.queryByText('EscMe')).not.toBeInTheDocument();
  });

  it('caps the visible stack at three toasts', async () => {
    function FireFour() {
      const { showToast } = useToast();
      const firedRef = useRef(false);
      useEffect(() => {
        if (firedRef.current) return;
        firedRef.current = true;
        showToast({ message: 'one' });
        showToast({ message: 'two' });
        showToast({ message: 'three' });
        showToast({ message: 'four' });
      }, [showToast]);
      return null;
    }
    render(
      <ToastProvider>
        <FireFour />
      </ToastProvider>,
    );
    await waitFor(() =>
      expect(screen.getByText('four')).toBeInTheDocument(),
    );
    // Oldest entry got pushed out.
    expect(screen.queryByText('one')).not.toBeInTheDocument();
    expect(screen.getByText('two')).toBeInTheDocument();
    expect(screen.getByText('three')).toBeInTheDocument();
    expect(screen.getByText('four')).toBeInTheDocument();
  });

  it('throws when useToast runs outside the provider', () => {
    function Bad() {
      useToast();
      return null;
    }
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => render(<Bad />)).toThrow(/useToast/);
    spy.mockRestore();
  });
});
