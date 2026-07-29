import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

// Mock the Tauri invoke boundary so openExternalUrl resolves without
// a backend.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.resolve()),
}));

import { invoke } from '@tauri-apps/api/core';

import { AnnouncerProvider } from '../a11y/Announcer';
import { DescriptionLinks } from './DescriptionLinks';
// Side-effect import initialises the shared i18next instance so
// useTranslation resolves the descriptionLinks.* keys.
import '../i18n';

const invokeMock = invoke as unknown as ReturnType<typeof vi.fn>;

function renderWith(text: string | null) {
  return render(
    <AnnouncerProvider>
      <DescriptionLinks text={text} />
    </AnnouncerProvider>,
  );
}

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe('DescriptionLinks', () => {
  it('renders nothing when the text has no links', () => {
    const { container } = renderWith('just some prose, no urls');
    expect(container.querySelector('.description-links')).toBeNull();
  });

  it('renders one button per detected link', () => {
    renderWith('see https://one.com and https://two.com');
    expect(screen.getByText('https://one.com')).toBeTruthy();
    expect(screen.getByText('https://two.com')).toBeTruthy();
  });

  it('opens the URL through the backend command on click', async () => {
    renderWith('go to https://example.com please');
    fireEvent.click(screen.getByText('https://example.com'));
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith('open_external_url', {
        url: 'https://example.com',
      }),
    );
  });

  it('normalises a bare email to a mailto: link', () => {
    renderWith('write to me at user@example.com');
    expect(screen.getByText('mailto:user@example.com')).toBeTruthy();
  });
});

describe('DescriptionLinks keyboard shape', () => {
  it('is one tab stop however many links there are', () => {
    renderWith(
      [
        'Join the meeting: https://a.test/one',
        'Global call-in numbers: https://a.test/two',
        'Notes: https://a.test/three',
      ].join('\n'),
    );
    const items = screen.getAllByRole('button');
    expect(items).toHaveLength(3);
    // Exactly one child is reachable by Tab; the rest are reached with the
    // arrow keys, which is what makes three links cost one stop and not three.
    expect(items.filter((el) => el.tabIndex === 0)).toHaveLength(1);
    expect(items[0].tabIndex).toBe(0);
  });

  it('groups the links as a toolbar, because they are actions not a selection', () => {
    renderWith('Join the meeting: https://a.test/one');
    expect(screen.getByRole('toolbar')).toBeTruthy();
  });

  it('names a link by what the description calls it', () => {
    renderWith('Join the meeting: https://a.test/one');
    // The label leads; the URL still follows, so it can be read out or dictated.
    expect(
      screen.getByRole('button', { name: /Join the meeting/ }),
    ).toBeTruthy();
  });
});
