import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';

import { EditEventScopeDialog } from './EditEventScopeDialog';

const noop = () => {};

describe('EditEventScopeDialog', () => {
  it('says nothing about loading while nothing failed', () => {
    render(
      <EditEventScopeDialog
        isOpen
        onClose={noop}
        title="Standup"
        onOccurrence={noop}
        onThisAndFuture={noop}
        onSeries={noop}
      />,
    );
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('says the series could not be loaded and keeps every choice', () => {
    render(
      <EditEventScopeDialog
        isOpen
        onClose={noop}
        title="Standup"
        onOccurrence={noop}
        onThisAndFuture={noop}
        onSeries={noop}
        seriesLoadFailed
      />,
    );
    expect(screen.getByRole('alert').textContent).toMatch(/Standup/);
    expect(screen.getByRole('button', { name: /ganze serie|whole series/i })).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /nur diesen termin|this occurrence only/i }),
    ).toBeInTheDocument();
  });
});
