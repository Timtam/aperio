import { describe, expect, it } from 'vitest';

import { eventWriteErrorMessage, eventWriteRefusal } from '@aperio/shared';
import i18n from '../i18n';

/**
 * What the user hears when a save, a delete or an answer did not happen.
 *
 * The adapter starts its message with a token, because the refusal crosses
 * several layers that carry only text. Before this, a blind user heard
 * "forbidden: reply-only-invitation: title" read out in English whatever
 * language the app ran in.
 */

const t = (key: string, values?: Record<string, unknown>): string =>
  i18n.getFixedT('de')(key, values as never) as string;

/** An error as the desktop host hands it over. */
const command = (code: string, message: string) => ({ code, message });

describe('eventWriteErrorMessage', () => {
  it('says in words that only the organizer may change a meeting', () => {
    const message = eventWriteErrorMessage(
      command('forbidden', 'reply-only-invitation: title'),
      t,
    );
    expect(message).toBe('Nur der Organisator kann diese Besprechung ändern. Es wurde nichts geändert.');
    expect(message).not.toMatch(/reply-only|forbidden|title/);
  });

  it('names what the server refused, with its own condition', () => {
    expect(eventWriteErrorMessage(command('forbidden', 'server-refused: need-privileges'), t)).toBe(
      'Der Kalenderserver hat diese Änderung abgelehnt (need-privileges). Es wurde nichts geändert.',
    );
  });

  it('reads the token whatever error carried it', () => {
    // The unknown identity travels as a network error: the server could not
    // be asked, and the write is worth retrying.
    const message = eventWriteErrorMessage(
      command('network', 'identity-unknown: the account’s own addresses could not be read'),
      t,
    );
    expect(message).toMatch(/^Aperio konnte die eigenen Adressen/);
    expect(eventWriteRefusal(command('network', 'identity-unknown'))?.refusal).toBe(
      'identity-unknown',
    );
  });

  it('has a sentence for a provider that refused without a token', () => {
    expect(eventWriteErrorMessage(command('forbidden', 'Zugriff verweigert'), t)).toBe(
      'Der Anbieter erlaubt diese Änderung nicht (Zugriff verweigert). Es wurde nichts geändert.',
    );
  });

  it('tells a conflict apart from a refusal', () => {
    expect(eventWriteErrorMessage(command('conflict', 'etag mismatch'), t)).toMatch(
      /^Dieser Termin wurde auf dem Server geändert/,
    );
  });

  it('gives the same sentence for the shape the phone throws', () => {
    // Mobile throws an Error with a code on it; the desktop an object.
    const mobile = Object.assign(new Error('reply-only-invitation: start'), {
      code: 'forbidden',
    });
    expect(eventWriteErrorMessage(mobile, t)).toBe(
      eventWriteErrorMessage(command('forbidden', 'reply-only-invitation: start'), t),
    );
  });

  it('keeps anything else it is given, without the Error prefix', () => {
    expect(eventWriteErrorMessage(command('io', 'disk full'), t)).toBe('io: disk full');
    expect(eventWriteErrorMessage(new Error('boom'), t)).toBe('boom');
    expect(eventWriteErrorMessage('plain', t)).toBe('plain');
    // A message that only starts like a token is not one.
    expect(eventWriteRefusal(command('forbidden', 'server-refused-by-proxy: x'))).toBeNull();
  });
});
