import { afterEach, describe, expect, it, vi } from 'vitest';

/**
 * The phone's date and time field carriers (`mobile/src/intl/dateTimeField.ts`).
 *
 * @expo/ui hands the Android date dialog a Date's millis unchanged, and
 * Material3's date picker reads them as a day in UTC and answers with midnight
 * UTC of the chosen day. A local midnight east of UTC is the day before in UTC,
 * so the dialog preselected the day before and confirming it unchanged saved
 * that day; west of UTC every chosen date read back a day early. The phone has
 * no test runner of its own, so its module runs here.
 *
 * Both hold in any process zone; only one with an offset shows the old fault
 * (the desktop's own zone where this runs locally, UTC on CI).
 */

/** Through a path the type checker does not follow (see
 *  `../state/moveActions.parity.test.ts`). */
const PHONE_FIELD = '../../mobile/src/intl/dateTimeField';

interface Field {
  parseLocalDate(value: string): Date;
  formatLocalDate(date: Date): string;
  parseLocalTime(value: string): Date;
  formatLocalTime(date: Date): string;
  toAndroidDialogDay(local: Date): Date;
  fromAndroidDialogDay(utcMidnight: Date): Date;
}

async function field(): Promise<Field> {
  return (await import(/* @vite-ignore */ PHONE_FIELD)) as Field;
}

/** The day Material3 shows for the millis it is given: their UTC day. */
const shownDay = (date: Date): string => date.toISOString().slice(0, 10);

/** What Material3 answers for a chosen day: its midnight UTC. */
const answered = (day: string): Date => new Date(`${day}T00:00:00Z`);

/** Days around year and month ends, both clock changes in Europe, and a leap
 *  day. */
const DAYS = ['2026-01-01', '2026-03-29', '2026-10-25', '2026-12-31', '2028-02-29'];

describe('the Android date dialog', () => {
  it('preselects the day the field holds', async () => {
    const f = await field();
    for (const day of DAYS) {
      expect(shownDay(f.toAndroidDialogDay(f.parseLocalDate(day))), day).toBe(day);
    }
  });

  it('reads back the day that was chosen', async () => {
    const f = await field();
    for (const day of DAYS) {
      expect(f.formatLocalDate(f.fromAndroidDialogDay(answered(day))), day).toBe(day);
    }
  });

  it('jumps to a local midnight of the chosen day', async () => {
    const f = await field();
    const jumped = f.fromAndroidDialogDay(answered('2026-11-22'));
    expect([jumped.getHours(), jumped.getMinutes()]).toEqual([0, 0]);
    expect(f.formatLocalDate(jumped)).toBe('2026-11-22');
  });
});

describe('a typed time', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('keeps a time the device clock skips today', async () => {
    // Berlin's spring-forward day: 02:00 to 03:00 does not exist. The time used
    // to ride on today's date and came back as 03:30.
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2027-03-28T00:30:00Z'));
    const f = await field();
    for (const time of ['00:00', '02:30', '12:05', '23:59']) {
      expect(f.formatLocalTime(f.parseLocalTime(time)), time).toBe(time);
    }
  });
});
