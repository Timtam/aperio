import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import { EditEventScopeDialog } from './EditEventScopeDialog';

const noop = () => {};

function renderPrompt(seriesLoadFailed?: number) {
  return render(
    <EditEventScopeDialog
      isOpen
      onClose={noop}
      title="Standup"
      onOccurrence={noop}
      onThisAndFuture={noop}
      onSeries={noop}
      seriesLoadFailed={seriesLoadFailed}
    />,
  );
}

describe('EditEventScopeDialog', () => {
  it('says nothing about loading while nothing failed', () => {
    renderPrompt();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('says the series could not be loaded and keeps every choice', () => {
    renderPrompt(1);
    expect(screen.getByRole('alert').textContent).toMatch(/Standup/);
    expect(screen.getByRole('button', { name: /ganze serie|whole series/i })).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /nur diesen termin|this occurrence only/i }),
    ).toBeInTheDocument();
  });

  it('lets the failure be read again from "Whole series"', () => {
    renderPrompt(1);
    const series = screen.getByRole('button', { name: /ganze serie|whole series/i });
    expect(series.getAttribute('aria-describedby')).toBe(screen.getByRole('alert').id);
  });

  it('puts a new alert in place when a retry fails too', () => {
    // An unchanged alert is not announced again.
    const { rerender } = renderPrompt(1);
    const first = screen.getByRole('alert');
    rerender(
      <EditEventScopeDialog
        isOpen
        onClose={noop}
        title="Standup"
        onOccurrence={noop}
        onThisAndFuture={noop}
        onSeries={noop}
        seriesLoadFailed={2}
      />,
    );
    expect(screen.getByRole('alert')).not.toBe(first);
  });
});
