import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

import { DialogStateProvider } from './DialogState';
import type { DialogStateValue } from './DialogState';
import { useDialogState } from './dialogStateContext';

// Captures the live context so a test can drive open/close imperatively — a
// button click would steal focus and defeat the "trigger was on <body>" case.
function ApiProbe({ apiRef }: { apiRef: { current: DialogStateValue | null } }) {
  apiRef.current = useDialogState();
  // A stand-in for the active view: the [data-active-view-root] wrapper (always
  // present in the real shell) around a focusable role="listbox" container, plus
  // an unrelated opener button OUTSIDE the view.
  return (
    <div>
      <div data-active-view-root tabIndex={-1}>
        <ul role="listbox" tabIndex={0} data-testid="grid" aria-label="view" />
      </div>
      <button type="button" data-testid="opener">
        opener
      </button>
    </div>
  );
}

describe('DialogState focus-return keeps the user in the application role', () => {
  // Run the close() double-rAF synchronously so focus has settled by assertion.
  beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      cb(0);
      return 0;
    });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function renderApi() {
    const apiRef: { current: DialogStateValue | null } = { current: null };
    render(
      <DialogStateProvider>
        <ApiProbe apiRef={apiRef} />
      </DialogStateProvider>,
    );
    return apiRef;
  }

  it('falls back to the view container when the trigger was <body> (Alt+N)', () => {
    const apiRef = renderApi();
    // Alt+N fires globally: focus may be on <body>, so the captured trigger is
    // null. Nothing is focused after render ⇒ activeElement is <body>.
    expect(document.activeElement).toBe(document.body);
    act(() => apiRef.current!.openQuickAddTask());
    act(() => apiRef.current!.close());
    // Focus must land inside the view (the listbox), NOT stay on <body>.
    expect(document.activeElement).toBe(screen.getByTestId('grid'));
  });

  it('falls back to the view when the trigger was unmounted after create', () => {
    const apiRef = renderApi();
    // A detached trigger: focus a throwaway button, capture it, then remove it
    // before close — mimicking the post-create re-render unmounting the trigger.
    const ephemeral = document.createElement('button');
    document.body.appendChild(ephemeral);
    ephemeral.focus();
    expect(document.activeElement).toBe(ephemeral);
    act(() => apiRef.current!.openQuickAddTask());
    ephemeral.remove();
    act(() => apiRef.current!.close());
    expect(document.activeElement).toBe(screen.getByTestId('grid'));
  });

  it('still returns focus to a live trigger (normal button-opened path)', () => {
    const apiRef = renderApi();
    const opener = screen.getByTestId('opener');
    opener.focus();
    act(() => apiRef.current!.openTaskDialog(null));
    act(() => apiRef.current!.close());
    // A surviving trigger wins — focus returns to it, not the view fallback.
    expect(document.activeElement).toBe(opener);
  });

  it('does not steal focus to the view when a parent dialog remains', () => {
    const apiRef = renderApi();
    // Stack two dialogs; the inner one's trigger is a throwaway we then detach.
    const opener = screen.getByTestId('opener');
    opener.focus();
    act(() => apiRef.current!.openSettings());
    const ephemeral = document.createElement('button');
    document.body.appendChild(ephemeral);
    ephemeral.focus();
    act(() => apiRef.current!.openSearch());
    ephemeral.remove();
    act(() => apiRef.current!.close());
    // A parent dialog is still open ⇒ the view fallback must NOT fire (its Modal
    // owns focus); focus is left off the view container.
    expect(document.activeElement).not.toBe(screen.getByTestId('grid'));
  });
});
