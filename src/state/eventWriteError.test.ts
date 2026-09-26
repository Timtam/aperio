import { describe, expect, it } from 'vitest';

import {
  eventWriteErrorMessage,
  eventWriteFailureReason,
  eventWriteRefusal,
  writeNeverLanded,
} from '@aperio/shared';
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
  i18n.getFixedT('de')(key, values as never) as unknown as string;

/** An error as the desktop host hands it over. */
const command = (code: string, message: string) => ({ code, message });

describe("a refusal on the phone, behind Expo's wrapper", () => {
  it('is found after the cause marker, on Android and on iOS', () => {
    const android = {
      code: 'forbidden',
      message:
        "Call to function 'CalFfi.updateEventJson' has been rejected.\n\u2192 Caused by: reply-only-invitation: title",
    };
    const ios = new Error(
      "Calling the 'updateEventJson' function has failed\n\u2192 Caused by: reply-only-invitation: title",
    );
    for (const err of [android, ios]) {
      expect(eventWriteRefusal(err)).toEqual({
        refusal: 'reply-only-invitation',
        key: 'dialogs.event.writeError.replyOnly',
        detail: 'title',
      });
    }
  });
});

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

describe('writeNeverLanded', () => {
  it('holds for a refusing code, on either surface', () => {
    for (const code of ['conflict', 'forbidden', 'invalid_input', 'not_found', 'auth', 'unsupported']) {
      expect(writeNeverLanded(command(code, 'no')), code).toBe(true);
    }
    // The phone throws an Error with a code on it.
    expect(writeNeverLanded(Object.assign(new Error('no'), { code: 'conflict' }))).toBe(true);
  });

  it('holds for a refusal token, whatever code carried it', () => {
    // An unknown identity is a network error, but no request went out.
    expect(writeNeverLanded(command('network', 'identity-unknown: me@example.org'))).toBe(true);
    expect(writeNeverLanded(new Error('server-refused: quota'))).toBe(true);
  });

  it('never holds where the write may have reached the provider', () => {
    expect(writeNeverLanded(command('network', 'connection reset'))).toBe(false);
    expect(writeNeverLanded(command('protocol', 'unreadable answer'))).toBe(false);
    expect(writeNeverLanded(command('internal', 'cache write failed'))).toBe(false);
    // No code at all: on the phone, every code but three arrives this way.
    expect(writeNeverLanded(new Error('Call to function has been rejected.'))).toBe(false);
    expect(writeNeverLanded('offline')).toBe(false);
    expect(writeNeverLanded(null)).toBe(false);
  });
});

describe('eventWriteFailureReason', () => {
  it('gives the reason alone, never "nothing was changed"', () => {
    // For a sentence that goes on to say what DID change: a split's new
    // series that could not be taken back.
    for (const err of [
      command('conflict', 'etag mismatch'),
      command('forbidden', 'read-only calendar'),
      command('forbidden', 'reply-only-invitation: title'),
      command('network', 'server-refused: quota'),
      command('network', 'identity-unknown: me@example.org'),
      command('invalid_input', 'occurrence-not-writable: 2026-08-24'),
    ]) {
      const reason = eventWriteFailureReason(err, t);
      expect(reason, err.message).not.toMatch(/nichts geändert|erneut/);
      expect(reason, err.message).not.toBe('');
    }
    expect(eventWriteFailureReason(command('conflict', 'etag mismatch'), t)).toMatch(
      /auf dem Server geändert/,
    );
    expect(eventWriteFailureReason(command('forbidden', 'read-only calendar'), t)).toMatch(
      /read-only calendar/,
    );
  });

  it('keeps the message of anything else', () => {
    const err = command('network', 'connection reset');
    expect(eventWriteFailureReason(err, t)).toBe(eventWriteErrorMessage(err, t));
  });
});

describe('a write Aperio will not risk', () => {
  it('reads as its own refusal, on either surface, and as nothing written', () => {
    // A CalDAV resource whose blocks name different organizers is not
    // written: the adapter says so with a token of its own.
    const err = command('forbidden', 'unsafe-to-write: mixed-organizers');
    expect(eventWriteRefusal(err)?.refusal).toBe('unsafe-to-write');
    expect(eventWriteErrorMessage(err, t)).toMatch(/nicht sicher ändern/);
    expect(eventWriteErrorMessage(err, t)).not.toMatch(/mixed-organizers/);
    expect(eventWriteFailureReason(err, t)).not.toMatch(/nichts geändert/);
    expect(writeNeverLanded(err)).toBe(true);
  });

  it('counts a server that turned the write down as nothing written', () => {
    // 400, 429 and their kin, from every adapter's update (decision 147).
    expect(writeNeverLanded(command('forbidden', 'server-refused: HTTP 429'))).toBe(true);
    expect(eventWriteErrorMessage(command('forbidden', 'server-refused: HTTP 429'), t)).toMatch(
      /abgelehnt \(HTTP 429\)/,
    );
  });
});
