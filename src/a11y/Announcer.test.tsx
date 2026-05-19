import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { AnnouncerProvider, useAnnouncer } from './Announcer';

function Trigger({
  urgency,
  message = 'Hello world',
}: {
  urgency?: 'polite' | 'assertive';
  message?: string;
}) {
  const announce = useAnnouncer();
  return (
    <button type="button" onClick={() => announce(message, urgency)}>
      Fire
    </button>
  );
}

describe('AnnouncerProvider', () => {
  it('writes polite announcements after a tick', async () => {
    render(
      <AnnouncerProvider>
        <Trigger />
      </AnnouncerProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Fire' }));
    await waitFor(() =>
      expect(screen.getByTestId('announcer-polite')).toHaveTextContent(
        'Hello world',
      ),
    );
    expect(screen.getByTestId('announcer-assertive')).toHaveTextContent('');
  });

  it('writes assertive announcements when urgency=assertive', async () => {
    render(
      <AnnouncerProvider>
        <Trigger urgency="assertive" />
      </AnnouncerProvider>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Fire' }));
    await waitFor(() =>
      expect(screen.getByTestId('announcer-assertive')).toHaveTextContent(
        'Hello world',
      ),
    );
  });

  it('re-announces by blanking first', async () => {
    render(
      <AnnouncerProvider>
        <Trigger />
      </AnnouncerProvider>,
    );
    const button = screen.getByRole('button', { name: 'Fire' });
    fireEvent.click(button);
    await waitFor(() =>
      expect(screen.getByTestId('announcer-polite')).toHaveTextContent(
        'Hello world',
      ),
    );
    // Fire again — the implementation must clear the region first so the
    // identical text is announced a second time.
    fireEvent.click(button);
    expect(screen.getByTestId('announcer-polite')).toHaveTextContent('');
    await waitFor(() =>
      expect(screen.getByTestId('announcer-polite')).toHaveTextContent(
        'Hello world',
      ),
    );
  });

  it('throws when used outside provider', () => {
    function Bad() {
      useAnnouncer();
      return null;
    }
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(() => render(<Bad />)).toThrow(/useAnnouncer/);
    spy.mockRestore();
  });
});
