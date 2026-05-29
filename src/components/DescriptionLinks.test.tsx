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
