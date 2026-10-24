import { describe, expect, it } from 'vitest';

import {
  errorMessage,
  meetingFailureSentence,
  readErrorMessage,
  readFailureReason,
  readFailureSentence,
  writeErrorMessage,
  writeFailureSentence,
} from '@aperio/shared';

import i18n from '../i18n';

/**
 * What a person hears when a read or a write failed, whatever carried the
 * error: the desktop's `{code, message}` object, an Android `Error` with a
 * code, or the iOS `Error` behind Expo's wrapper (decision 273).
 */

const t = (key: string, values?: Record<string, unknown>): string =>
  i18n.getFixedT('de')(key, values as never) as unknown as string;

/** The three carriers of one host message. */
function carriers(code: string, message: string): unknown[] {
  const android = Object.assign(
    new Error(`Call to function 'CalFfi.getTasksJson' has been rejected.\n→ Caused by: ${message}`),
    { code },
  );
  const ios = new Error(`Calling the 'getTasksJson' function has failed\n→ Caused by: ${message}`);
  return [{ code, message }, android, ios];
}

describe('errorMessage', () => {
  it('says a desktop rejection as code and message, never "[object Object]"', () => {
    expect(errorMessage({ code: 'network', message: 'timed out' })).toBe('network: timed out');
    expect(writeErrorMessage({ code: 'network', message: 'timed out' }, t)).toBe('network: timed out');
  });

  it("says an Error as its message, without Expo's English in front", () => {
    expect(errorMessage(new Error('x'))).toBe('x');
    for (const err of carriers('network', 'timed out').slice(1)) {
      expect(errorMessage(err)).toBe('timed out');
    }
  });

  it('says anything else as itself', () => {
    expect(errorMessage('plain')).toBe('plain');
    expect(errorMessage(42)).toBe('42');
  });
});

describe('a read that failed', () => {
  it('says a busy provider without claiming a change was undone', () => {
    for (const err of carriers('forbidden', 'provider-busy: http-429')) {
      const said = readErrorMessage(err, t);
      expect(said).toBe('Der Anbieter ist gerade ausgelastet. Versuche es später noch einmal.');
      expect(said).not.toMatch(/geändert/);
      expect(readFailureReason(err, t)).toBe('der Anbieter ist gerade ausgelastet');
    }
  });

  it('says a device without access as a read', () => {
    const said = readErrorMessage({ code: 'forbidden', message: 'access-not-granted: reminders' }, t);
    expect(said).toMatch(/keinen Zugriff/);
    expect(said).not.toMatch(/geändert/);
  });

  it('says any other failure as what the host said', () => {
    expect(readErrorMessage({ code: 'network', message: 'timed out' }, t)).toBe('network: timed out');
    expect(readFailureReason(new Error('kaputt'), t)).toBe('kaputt');
  });

  it('frames a busy reason and adds that trying later may work, without a duration', () => {
    const busy = { code: 'forbidden', message: 'provider-busy: rate-limit' };
    expect(readFailureSentence('dialogs.event.availability.failed', busy, t)).toBe(
      'Die Verfügbarkeit ließ sich nicht prüfen: der Anbieter ist gerade ausgelastet. ' +
        'Versuche es später noch einmal.',
    );
    expect(
      readFailureSentence('dialogs.event.availability.failed', new Error('kaputt'), t),
    ).toBe('Die Verfügbarkeit ließ sich nicht prüfen: kaputt.');
  });
});

describe('writeFailureSentence', () => {
  it('says a busy provider in the caller\'s frame, and to try again later', () => {
    for (const err of carriers('forbidden', 'provider-busy: http-429')) {
      expect(writeFailureSentence('toast.undoFailed', err, t)).toBe(
        'Das ließ sich nicht rückgängig machen: der Anbieter ist gerade ausgelastet. Versuche es später noch einmal.',
      );
    }
  });

  it('says what stands in full when part of it landed', () => {
    for (const err of carriers('network', 'partly-saved: undo')) {
      expect(writeFailureSentence('toast.undoFailed', err, t)).toBe(
        'Die Änderung ist nur zum Teil gespeichert, weil der Anbieter gerade ausgelastet ist. Prüfe sie und versuche den Rest später noch einmal.',
      );
    }
  });
});

describe('meetingFailureSentence', () => {
  it('says a busy provider in the meeting frame, and to try again later', () => {
    for (const err of carriers('forbidden', 'provider-busy: http-429')) {
      expect(meetingFailureSentence(err, t)).toBe(
        'Das Meeting konnte nicht bearbeitet werden: der Anbieter ist gerade ausgelastet. Versuche es später noch einmal.',
      );
    }
  });

  it('says what stands in full when part of it landed', () => {
    for (const err of carriers('network', 'partly-saved: event')) {
      expect(meetingFailureSentence(err, t)).toBe(
        'Die Änderung ist nur zum Teil gespeichert, weil der Anbieter gerade ausgelastet ist. Prüfe sie und versuche den Rest später noch einmal.',
      );
    }
  });

  it('keeps what the host said for anything else', () => {
    expect(meetingFailureSentence({ code: 'network', message: 'offline' }, t)).toBe(
      'Das Meeting konnte nicht bearbeitet werden: network: offline',
    );
  });
});

describe('the new sentences', () => {
  const de = (key: string) => i18n.getFixedT('de')(key);
  const en = (key: string) => i18n.getFixedT('en')(key);
  it('exist in both languages and differ', () => {
    for (const key of [
      'common.providerBusy',
      'common.tryAgainLater',
      'common.accessNotGranted',
      'dialogs.event.availability.failed',
    ]) {
      expect(i18n.exists(key, { lng: 'de' }), key).toBe(true);
      expect(i18n.exists(key, { lng: 'en' }), key).toBe(true);
      expect(de(key), key).not.toBe(en(key));
    }
  });
});
