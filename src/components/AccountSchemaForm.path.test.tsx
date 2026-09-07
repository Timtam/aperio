import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

import type { AccountFormSpec } from '@aperio/shared';

/**
 * The browse button beside a `directory` / `file` field.
 *
 * The button is the convenience; the text input is the mechanism. Typing a
 * path has always worked and must keep working — it is the keyboard-only way
 * in, and it is what the SFTP key field has always been. So the tests here are
 * mostly about what the button must NOT do: it must not replace the input, and
 * cancelling it must not touch a path the user typed.
 */

const openDialog = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: openDialog }));

const SPEC: AccountFormSpec = {
  fields: [
    {
      key: 'path',
      label: 'Ordner',
      kind: 'directory',
      required: true,
    },
    {
      key: 'key_path',
      label: 'Schlüsseldatei',
      kind: 'file',
      required: false,
    },
    { key: 'host', label: 'Server', kind: 'text', required: true },
  ],
} as unknown as AccountFormSpec;

afterEach(() => {
  document.body.innerHTML = '';
  openDialog.mockReset();
});

async function form(values: Record<string, string | boolean> = {}) {
  const onChange = vi.fn();
  const { AccountSchemaForm } = await import('./AccountSchemaForm');
  render(<AccountSchemaForm spec={SPEC} values={values} onChange={onChange} />);
  return onChange;
}

describe('AccountSchemaForm → a path field', () => {
  it('keeps its text input, so a path can still be typed', async () => {
    const onChange = await form();
    const input = screen.getByRole('textbox', { name: /ordner/i });
    fireEvent.change(input, { target: { value: 'C:\\Sync' } });
    expect(onChange).toHaveBeenCalledWith('path', 'C:\\Sync');
  });

  it('names its browse button after the field', async () => {
    // Two path fields in one form: "Durchsuchen" twice in a row would tell a
    // screen-reader user nothing about which one they are on.
    await form();
    expect(
      screen.getByRole('button', { name: /Ordner wählen für Ordner/i }),
    ).toBeTruthy();
    expect(
      screen.getByRole('button', { name: /Datei wählen für Schlüsseldatei/i }),
    ).toBeTruthy();
  });

  it('asks for a FOLDER on a directory field and a FILE on a file field', async () => {
    await form();
    openDialog.mockResolvedValue(null);

    screen
      .getByRole('button', { name: /Ordner wählen für Ordner/i })
      .click();
    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(openDialog.mock.calls[0][0]).toMatchObject({ directory: true });

    screen
      .getByRole('button', { name: /Datei wählen für Schlüsseldatei/i })
      .click();
    await waitFor(() => expect(openDialog).toHaveBeenCalledTimes(2));
    expect(openDialog.mock.calls[1][0]).toMatchObject({ directory: false });
  });

  it('writes the picked path and leaves focus on the field it changed', async () => {
    const onChange = await form();
    openDialog.mockResolvedValue('C:\\Users\\Toni\\Sync');
    const button = screen.getByRole('button', { name: /Ordner wählen für Ordner/i });
    button.click();

    await waitFor(() =>
      expect(onChange).toHaveBeenCalledWith('path', 'C:\\Users\\Toni\\Sync'),
    );
    // The input reads its own label and new value on focus — that IS the
    // confirmation, and it is why nothing is announced separately.
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole('textbox', { name: /ordner/i }),
      ),
    );
  });

  it('changes nothing when the user cancels, and goes back to the button', async () => {
    // The whole reason cancel needed a decision: clearing the field here would
    // destroy a path somebody typed, and "I changed my mind about browsing" is
    // not "I want this empty".
    const onChange = await form({ path: 'C:\\Getippt' });
    openDialog.mockResolvedValue(null);
    const button = screen.getByRole('button', { name: /Ordner wählen für Ordner/i });
    button.click();

    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('textbox', { name: /ordner/i })).toHaveValue(
      'C:\\Getippt',
    );
    await waitFor(() => expect(document.activeElement).toBe(button));
  });

  it('says so when the picker refuses to open', async () => {
    // A button that does nothing is the worst outcome; the message also names
    // the way out, which is to type the path.
    const onChange = await form();
    openDialog.mockRejectedValue(new Error('no portal'));
    screen.getByRole('button', { name: /Ordner wählen für Ordner/i }).click();

    await waitFor(() =>
      expect(screen.getByText(/no portal/i)).toBeTruthy(),
    );
    expect(onChange).not.toHaveBeenCalled();
  });

  it('gives an ordinary text field no browse button', async () => {
    await form();
    expect(screen.getAllByRole('button')).toHaveLength(2);
  });
});
