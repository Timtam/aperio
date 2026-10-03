import { describe, expect, it } from 'vitest';

import {
  afterAnnouncing,
  leadingCause,
  passEndSentence,
  toAnnounce,
  withheldPhrase,
  withheldSince,
  withSpoken,
  type ContainerRefreshError,
  type FailingAccount,
  type RefreshCause,
} from '@aperio/shared';

import i18n from '../i18n';

const de = i18n.getFixedT('de');
const en = i18n.getFixedT('en');

const RANK: Record<RefreshCause, number> = { other: 0, auth: 1, access: 2 };
const row = (account_id: string, cause: RefreshCause) => ({ account_id, cause, rank: RANK[cause] });
const failing = (name: string, cause: RefreshCause): FailingAccount => ({
  account_id: name.toLowerCase(),
  name,
  cause,
  rank: RANK[cause],
});

describe('which cause a surface for several accounts leads with', () => {
  it('reads the core rank off, access before auth before anything else', () => {
    expect(leadingCause([])).toBeNull();
    expect(leadingCause([row('a', 'other'), row('b', 'auth')])).toBe('auth');
    expect(leadingCause([row('a', 'auth'), row('b', 'access'), row('c', 'other')])).toBe('access');
  });
});

describe('what a growth announcement still has to name', () => {
  it('names a new failure and one that grew worse, never one that eased', () => {
    const said = new Map([
      ['a', RANK.other],
      ['b', RANK.access],
    ]);
    const now = [row('a', 'auth'), row('b', 'other'), row('c', 'other')];
    expect(toAnnounce(now, said).map((r) => r.account_id)).toEqual(['a', 'c']);
  });

  it('keeps the most severe cause said while an account fails, and forgets one that cleared', () => {
    const said = new Map([
      ['a', RANK.access],
      ['gone', RANK.auth],
    ]);
    const next = afterAnnouncing([row('a', 'other'), row('b', 'auth')], said);
    expect([...next]).toEqual([
      ['a', RANK.access],
      ['b', RANK.auth],
    ]);
    // Easing back and worsening again to what was said is not news.
    expect(toAnnounce([row('a', 'access')], next)).toEqual([]);
  });

  it('does not name again what the closing sentence already named', () => {
    const said = withSpoken(new Map(), [failing('Work', 'access')]);
    expect(toAnnounce([row('work', 'access')], said)).toEqual([]);
  });
});

describe('the sentence that ends a warm pass', () => {
  it('names the accounts left not current, in name order', () => {
    const outcome = {
      failing: [failing('Zuhause', 'other'), failing('Arbeit', 'access')],
      all_failed: false,
    };
    expect(passEndSentence(outcome, de)).toBe(
      'Externe Daten aktualisiert, außer: Arbeit und Zuhause.',
    );
    expect(passEndSentence(outcome, en)).toBe(
      'External data updated, except: Arbeit and Zuhause.',
    );
  });

  it('says plainly when nothing is left undone, or nothing was read', () => {
    expect(passEndSentence({ failing: [], all_failed: false }, de)).toBe(
      'Externe Daten aktualisiert.',
    );
    // An older core, or an outcome that could not be read.
    expect(passEndSentence(null, de)).toBe('Externe Daten aktualisiert.');
  });

  it('does not claim an update when every account failed', () => {
    expect(passEndSentence({ failing: [failing('Arbeit', 'other')], all_failed: true }, de)).toBe(
      'Externe Daten konnten nicht aktualisiert werden.',
    );
  });
});

describe('the one line for a withheld account', () => {
  const error = (
    scope: string,
    cause: RefreshCause,
    last_success_at: string | null,
  ): ContainerRefreshError => ({
    scope,
    container_id: '',
    container_name: null,
    error: 'access-not-granted',
    last_success_at,
    cause,
  });

  it('names each withheld family once, and nothing else', () => {
    const errors = [
      error('task_lists', 'access', null),
      error('calendars', 'access', null),
      error('contacts', 'other', null),
    ];
    expect(withheldPhrase(errors, de)).toBe('die Kalender und die Aufgabenlisten');
    expect(withheldPhrase([error('events', 'other', null)], de)).toBe('die Daten dieses Kontos');
  });

  it('dates the data by the oldest withheld success', () => {
    const errors = [
      error('calendars', 'access', '2026-09-20T10:00:00Z'),
      error('task_lists', 'access', '2026-09-18T08:00:00Z'),
      error('contacts', 'other', '2026-01-01T00:00:00Z'),
    ];
    expect(withheldSince(errors)).toBe('2026-09-18T08:00:00Z');
    expect(withheldSince([error('calendars', 'access', null)])).toBeNull();
  });
});
