import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import { MoveEventScopeDialog } from './MoveEventScopeDialog';

describe('MoveEventScopeDialog', () => {
  it('offers both scopes when nothing was refused', () => {
    render(
      <MoveEventScopeDialog isOpen onClose={() => {}} title="Standup" onOccurrence={() => {}} onSeries={() => {}} />,
    );
    expect(screen.getByRole('button', { name: /nur diesen termin|this occurrence only/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /ganze serie|whole series/i })).toBeInTheDocument();
  });

  it('says why the whole series cannot move and offers only this occurrence', () => {
    render(
      <MoveEventScopeDialog
        isOpen
        onClose={() => {}}
        title="Standup"
        onOccurrence={() => {}}
        onSeries={() => {}}
        refused="ordinal_weekday"
      />,
    );
    expect(screen.getByText(/zweiten Sonntag|second Sunday/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /nur diesen termin|this occurrence only/i })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /ganze serie|whole series/i })).toBeNull();
  });

  it('describes the dialog with its message, so a screen reader reads the refusal', () => {
    render(
      <MoveEventScopeDialog
        isOpen
        onClose={() => {}}
        title="Standup"
        onOccurrence={() => {}}
        onSeries={() => {}}
        refused="month_end"
      />,
    );
    const describedBy = screen.getByRole('dialog').getAttribute('aria-describedby');
    expect(describedBy).toBeTruthy();
    expect(document.getElementById(describedBy!)?.textContent).toMatch(/Standup/);
  });
});
