import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

import type { ExceptionsLoss } from '@aperio/shared';

import { useExceptionsLossQuestion } from './ExceptionsLossQuestion';

/**
 * The question a refused drag of a series asks (decision 250): a confirm
 * dialog of its own with the editors' words; yes runs the same move again,
 * Cancel runs nothing.
 */

function Harness({ loss, yes }: { loss: ExceptionsLoss; yes: () => void }) {
  const { ask, dialog } = useExceptionsLossQuestion();
  return (
    <>
      <button type="button" onClick={() => ask(loss, 'Teamrunde', yes)}>
        Ziehen
      </button>
      {dialog}
    </>
  );
}

afterEach(() => {
  document.body.innerHTML = '';
});

describe('the question a refused drag asks', () => {
  it('says what the move rewrites and loses, and yes moves it again', async () => {
    const yes = vi.fn();
    render(<Harness loss={{ rewrite: 'slot', changed: 1, deleted: 0 }} yes={yes} />);
    fireEvent.click(screen.getByRole('button', { name: 'Ziehen' }));
    const dialog = await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
    expect(dialog.textContent).toMatch(/Teamrunde/);
    expect(dialog.textContent).toMatch(/Beginn und Ende der Serie neu/);
    expect(dialog.textContent).toMatch(/ein Vorkommen, das einzeln geändert wurde/);
    fireEvent.click(within(dialog).getByRole('button', { name: /trotzdem speichern/i }));
    expect(yes).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /vorkommen gehen verloren/i })).toBeNull(),
    );
  });

  it('moves nothing on Cancel', async () => {
    const yes = vi.fn();
    render(<Harness loss={{ rewrite: 'pattern', changed: 0, deleted: 2 }} yes={yes} />);
    fireEvent.click(screen.getByRole('button', { name: 'Ziehen' }));
    const dialog = await screen.findByRole('dialog', { name: /vorkommen gehen verloren/i });
    expect(dialog.textContent).toMatch(/2 gelöschte Vorkommen/);
    fireEvent.click(within(dialog).getByRole('button', { name: /^abbrechen$|^cancel$/i }));
    await waitFor(() =>
      expect(screen.queryByRole('dialog', { name: /vorkommen gehen verloren/i })).toBeNull(),
    );
    expect(yes).not.toHaveBeenCalled();
  });
});
