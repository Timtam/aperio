import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { Modal } from './Modal';

afterEach(() => {
  // Clean up any portaled content the test left around.
  document.body.innerHTML = '';
});

describe('Modal', () => {
  it('renders nothing when closed', () => {
    render(
      <Modal isOpen={false} onClose={() => {}} title="Test">
        <p>content</p>
      </Modal>,
    );
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('renders a labelled dialog when open', () => {
    render(
      <Modal isOpen onClose={() => {}} title="Edit event">
        <input aria-label="title" />
      </Modal>,
    );
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveAccessibleName('Edit event');
  });

  it('Escape calls onClose', () => {
    const onClose = vi.fn();
    render(
      <Modal isOpen onClose={onClose} title="X">
        <input aria-label="title" />
      </Modal>,
    );
    fireEvent.keyDown(screen.getByRole('dialog').parentElement!, {
      key: 'Escape',
    });
    expect(onClose).toHaveBeenCalled();
  });

  it('traps Tab inside the dialog (close button is part of the cycle)', () => {
    render(
      <Modal isOpen onClose={() => {}} title="X">
        <button type="button">first</button>
        <button type="button">last</button>
      </Modal>,
    );
    const close = screen.getByRole('button', { name: 'Close' });
    const last = screen.getByRole('button', { name: 'last' });

    // Forward wrap: from the last button, Tab goes back to the first
    // focusable inside the dialog — which is the close button.
    last.focus();
    fireEvent.keyDown(screen.getByRole('dialog').parentElement!, {
      key: 'Tab',
    });
    expect(document.activeElement).toBe(close);

    // Backward wrap: Shift+Tab from the close button goes to the last.
    close.focus();
    fireEvent.keyDown(screen.getByRole('dialog').parentElement!, {
      key: 'Tab',
      shiftKey: true,
    });
    expect(document.activeElement).toBe(last);
  });

  it('focuses the first focusable element on open', () => {
    render(
      <Modal isOpen onClose={() => {}} title="X">
        <input aria-label="first" />
        <input aria-label="second" />
      </Modal>,
    );
    expect(document.activeElement).toBe(
      screen.getByRole('textbox', { name: 'first' }),
    );
  });

  it('restores previous focus on close', () => {
    const trigger = document.createElement('button');
    trigger.textContent = 'open';
    document.body.appendChild(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);

    const { rerender } = render(
      <Modal isOpen onClose={() => {}} title="X">
        <input aria-label="x" />
      </Modal>,
    );
    expect(document.activeElement).not.toBe(trigger);

    rerender(
      <Modal isOpen={false} onClose={() => {}} title="X">
        <input aria-label="x" />
      </Modal>,
    );
    expect(document.activeElement).toBe(trigger);
  });

  it('backdrop click closes when dismissOnBackdrop is true', () => {
    const onClose = vi.fn();
    render(
      <Modal isOpen onClose={onClose} title="X">
        <input aria-label="x" />
      </Modal>,
    );
    const backdrop = screen.getByRole('dialog').parentElement!;
    fireEvent.click(backdrop);
    expect(onClose).toHaveBeenCalled();
  });

  it('backdrop click is ignored when dismissOnBackdrop is false', () => {
    const onClose = vi.fn();
    render(
      <Modal isOpen onClose={onClose} title="X" dismissOnBackdrop={false}>
        <input aria-label="x" />
      </Modal>,
    );
    const backdrop = screen.getByRole('dialog').parentElement!;
    fireEvent.click(backdrop);
    expect(onClose).not.toHaveBeenCalled();
  });
});
