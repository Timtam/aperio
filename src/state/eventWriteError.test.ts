import { describe, expect, it } from 'vitest';

import {
  eventWriteErrorMessage,
  eventWriteFailureReason,
  eventWriteRefusal,
  exceptionsLossOf,
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

describe('a device store the OS has not opened', () => {
  it('reads as its own refusal, behind the wrapper too, and as nothing written', () => {
    // The device adapter refuses every call without full access (decision
    // 171): a delete reported as done while nothing changed was the bug.
    const desktop = command('forbidden', 'access-not-granted: calendars: Denied');
    const phone = new Error(
      "Calling the 'deleteEventById' function has failed\n\u2192 Caused by: access-not-granted: calendars: Denied",
    );
    for (const err of [desktop, phone]) {
      expect(eventWriteRefusal(err)?.refusal).toBe('access-not-granted');
      expect(eventWriteErrorMessage(err, t)).toMatch(/keinen Zugriff auf die Kalender/);
      expect(eventWriteErrorMessage(err, t)).not.toMatch(/Denied/);
    }
    expect(eventWriteFailureReason(desktop, t)).toMatch(/keinen Zugriff/);
    expect(writeNeverLanded(desktop)).toBe(true);
  });
});

describe('an all-day day in a time zone Aperio cannot read', () => {
  it('reads as its own refusal, behind the wrapper too, and as nothing written', () => {
    // Decision 237: Exchange stores the appointment in a zone with no clock
    // Aperio can compute, so its day is not moved; the title still saves.
    const desktop = command('forbidden', 'day-zone-unreadable: no-zone');
    const phone = new Error(
      "Calling the 'updateEventJson' function has failed\n→ Caused by: day-zone-unreadable: no-zone",
    );
    for (const err of [desktop, phone]) {
      expect(eventWriteRefusal(err)?.refusal).toBe('day-zone-unreadable');
      expect(eventWriteErrorMessage(err, t)).toBe(
        'Exchange speichert diesen Termin in einer Zeitzone, die Aperio nicht lesen kann. ' +
          'Seinen Tag kannst du deshalb hier nicht ändern; den Titel und anderes schon. ' +
          'Es wurde nichts geändert.',
      );
      expect(eventWriteErrorMessage(err, t)).not.toMatch(/no-zone/);
    }
    // The reason stands only in the split's sentences, about "die alte Serie".
    expect(eventWriteFailureReason(desktop, t)).toBe(
      'Exchange speichert sie in einer Zeitzone, die Aperio nicht lesen kann, ' +
        'deshalb lässt sich ihr Tag hier nicht ändern',
    );
    expect(writeNeverLanded(desktop)).toBe(true);
  });
});

describe('a save that would drop occurrences of a series', () => {
  it('reads as its own refusal, behind the wrapper too, says what and how many, and as nothing written', () => {
    // Decisions 243-253: Exchange drops a series' changed and deleted
    // occurrences when its start and end, its zone's clock or its pattern are
    // written again. Without consent nothing is sent; a path that does not ask
    // says what the save would rewrite and lose.
    const desktop = command('forbidden', 'exceptions-would-be-lost: zone:1:0');
    const phone = new Error(
      "Calling the 'updateEventJson' function has failed\n\u2192 Caused by: exceptions-would-be-lost: zone:1:0",
    );
    for (const err of [desktop, phone]) {
      expect(eventWriteRefusal(err)?.refusal).toBe('exceptions-would-be-lost');
      expect(exceptionsLossOf(err)).toEqual({ rewrite: 'zone', changed: 1, deleted: 0 });
      expect(eventWriteErrorMessage(err, t)).toBe(
        'Diese Änderung gibt der Serie eine andere Zeitzone und schreibt dafür Beginn und Ende neu. ' +
          'Exchange verwirft dabei ein Vorkommen, das einzeln geändert wurde: Es nimmt wieder ' +
          'die Angaben der Serie an. Es wurde nichts geändert.',
      );
      expect(eventWriteErrorMessage(err, t)).not.toMatch(/exceptions-would-be-lost/);
    }
    expect(eventWriteFailureReason(desktop, t)).toBe(
      'Exchange würde dabei ihre geänderten und gelöschten Vorkommen verwerfen',
    );
    expect(writeNeverLanded(desktop)).toBe(true);
    // A detail this build cannot read keeps the plain sentence.
    expect(eventWriteErrorMessage(command('forbidden', 'exceptions-would-be-lost: zone'), t)).toBe(
      'Diese Änderung würde die Serie neu schreiben, und dabei verwirft Exchange ihre einzeln ' +
        'geänderten und gelöschten Vorkommen. Es wurde nichts geändert.',
    );
  });
});

describe('a write that needs the current copy, when it cannot be read', () => {
  it('says to try again, and counts as nothing written', () => {
    // An all-day day is never written blind: without the copy nothing is sent,
    // so a split can take its new part back (decision 144).
    const desktop = command('forbidden', 'copy-unreadable: EWS HTTP 503: Service Unavailable');
    const phone = new Error(
      "Calling the 'updateEventJson' function has failed\n→ Caused by: copy-unreadable: network down",
    );
    for (const err of [desktop, phone]) {
      expect(eventWriteRefusal(err)?.refusal).toBe('copy-unreadable');
      expect(eventWriteErrorMessage(err, t)).toMatch(/aktuellen Stand dieses Termins nicht/);
      expect(eventWriteErrorMessage(err, t)).toMatch(/Versuche es gleich noch einmal/);
      expect(eventWriteErrorMessage(err, t)).not.toMatch(/503|network/);
    }
    expect(eventWriteFailureReason(desktop, t)).toBe(
      'Aperio konnte ihren aktuellen Stand nicht vom Server lesen',
    );
    expect(writeNeverLanded(desktop)).toBe(true);
  });
});

describe('any write on the phone, not only the event editor', () => {
  it('says a refusal as its sentence and anything else as before', async () => {
    const { writeErrorMessage } = await import('@aperio/shared');
    // A reminder ticked off without access: the task screens said this raw.
    const refused = new Error(
      "Calling the 'updateTaskJson' function has failed\n\u2192 Caused by: access-not-granted: reminders: Denied",
    );
    expect(writeErrorMessage(refused, t)).toMatch(/keinen Zugriff auf die Kalender und Erinnerungen/);
    const other = new Error('network down');
    expect(writeErrorMessage(other, t)).toBe('network down');
    expect(writeErrorMessage('plain', t)).toBe('plain');
  });
});
