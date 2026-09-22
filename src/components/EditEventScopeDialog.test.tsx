import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import { EditEventScopeDialog } from './EditEventScopeDialog';

const noop = () => {};

function renderPrompt(
  seriesLoadFailed?: number,
  seriesLoadFailedScope?: 'series' | 'this_and_future',
) {
  return render(
    <EditEventScopeDialog
      isOpen
      onClose={noop}
      title="Standup"
      onOccurrence={noop}
      onThisAndFuture={noop}
      onSeries={noop}
      seriesLoadFailed={seriesLoadFailed}
      seriesLoadFailedScope={seriesLoadFailedScope}
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

  it('lets the failure be read again from "This and all following" when that one failed', () => {
    // It loads the series too, for an occurrence the provider keeps (126), and
    // focus stays on it.
    renderPrompt(1, 'this_and_future');
    const future = screen.getByRole('button', {
      name: /diesen und alle folgenden|this and all following/i,
    });
    expect(future.getAttribute('aria-describedby')).toBe(screen.getByRole('alert').id);
    const series = screen.getByRole('button', { name: /ganze serie|whole series/i });
    expect(series.getAttribute('aria-describedby')).toBeNull();
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
