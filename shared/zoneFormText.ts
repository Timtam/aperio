// What the editors say about a series' clock — stage 12 of
// DESIGN-series-time-zone.md, "Beschriftung, Hinweis und was man hört" and
// "Prüfen und Speichern": the field labels that name the clock, the sentences
// after the end time, the gap message, the announcement after the repeat field
// switched the clock, and why a save is refused. The same on both editors.
//
// Pure: the editors pass the device's zone (`deviceZoneName()`), their
// language and their translations.

import {
  fieldInstant,
  sameClockAsDevice,
  type ClockFieldKey,
  type ClockFields,
  type FieldClock,
  type FieldInstant,
} from './clockFields';
import { zoneUsableHere } from './recurrence';
import { canonicalZone, seriesClockZone } from './seriesClock';
import { unusableZoneRefusal, type SeriesZoneForm, type ZoneFormView } from './seriesZoneForm';
import { listedZoneLabels } from './zoneList';

type Translate = (key: string, vars?: Record<string, unknown>) => string;

/** A sentence the editor shows as its own read-only field (`ReadOnlyText` on
 *  the desktop, a text with its own swipe stop on the phone). */
export interface ZoneSentence {
  /** What it is about, read first. */
  label: string;
  value: string;
}

/**
 * A zone as the labels and sentences name it: its city („New York“), "UTC"
 * for a series on UTC or on a name tzdata does not know (the core reads it as
 * UTC), and the canonical name for a zone outside the list („EST5EDT“).
 */
export function zoneCityName(zone: string | null | undefined): string {
  const named = seriesClockZone(zone);
  if (named === null) return 'UTC';
  const canonical = canonicalZone(named) ?? named;
  return listedZoneLabels().find((label) => label.zone === canonical)?.city ?? canonical;
}

/**
 * The name of the clock the fields stand on, for their labels: `null` where
 * it reads every instant as the device does (`sameClockAsDevice`), so the
 * labels stay as they are; else the city or "UTC". `Asia/Calcutta` on the
 * device and a series on Kolkata are one clock.
 */
export function fieldClockName(clock: FieldClock, deviceZone: string | null): string | null {
  if (clock.kind === 'device' || sameClockAsDevice(clock, deviceZone)) return null;
  return zoneCityName(clock.tzid);
}

/** A date or time field's label, naming the clock where it is not the
 *  device's: „Startzeit (New York)“. */
export function clockFieldLabel(
  key: ClockFieldKey,
  clock: FieldClock,
  deviceZone: string | null,
  t: Translate,
): string {
  const name = fieldClockName(clock, deviceZone);
  return name === null
    ? t(`dialogs.event.fields.${key}`)
    : t(`dialogs.event.fields.${key}WithClock`, { clock: name });
}

/** The `timeZone` an Intl formatter can take for `zone`: the zone's own name
 *  where this runtime can use it, "UTC" for a series on UTC, and the
 *  runtime's own zone (`undefined`) for a name it refuses. */
export function clockIntlZone(zone: string | null): string | undefined {
  const named = seriesClockZone(zone);
  if (named === null) return 'UTC';
  return zoneUsableHere(named) ? named : undefined;
}

/** A `YYYY-MM-DD` day of `at` on `timeZone`. */
function dayOn(at: Date, timeZone: string | undefined): string {
  return new Intl.DateTimeFormat('en-CA', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(at);
}

/**
 * Two instants on a clock, as a sentence says them: „Montag, 14. September,
 * 15:00 bis 16:00“, the end's day named only where it is another.
 * `timeZone` is an Intl zone, or `undefined` for the runtime's own.
 */
export function clockRangeText(
  start: string,
  end: string,
  language: string,
  timeZone: string | undefined,
  t: Translate,
): string {
  const day = new Intl.DateTimeFormat(language, { weekday: 'long', day: 'numeric', month: 'long', timeZone });
  const time = new Intl.DateTimeFormat(language, { hour: '2-digit', minute: '2-digit', timeZone });
  const from = new Date(start);
  const to = new Date(end);
  const startText = `${day.format(from)}, ${time.format(from)}`;
  const endText =
    dayOn(from, timeZone) === dayOn(to, timeZone) ? time.format(to) : `${day.format(to)}, ${time.format(to)}`;
  return t('views.timeRange', { start: startText, end: endText });
}

/** The `timeZone` an Intl formatter takes for the device: the name it
 *  reports, or the runtime's own zone where it reports none it can use. */
function deviceFormatterZone(deviceZone: string | null): string | undefined {
  return deviceZone !== null && zoneUsableHere(deviceZone) ? deviceZone : undefined;
}

/** The device's clock as the hint names it; `null` for a device that
 *  reports no zone, or one tzdata does not know. */
function deviceClockName(deviceZone: string | null): string | null {
  if (deviceZone === null || canonicalZone(deviceZone) === null) return null;
  return zoneCityName(deviceZone);
}

/** The instants the fields name, or `null` where either names none. */
function fieldInstants(fields: ClockFields): { start: string; end: string } | null {
  const start = fieldInstant(fields.start, fields.clock);
  const end = fieldInstant(fields.end, fields.clock);
  return start.kind === 'at' && end.kind === 'at' ? { start: start.at, end: end.at } : null;
}

/**
 * The hint after the end time where the fields stand on another clock than
 * the device's: label „Auf diesem Gerät (Berlin)“, value „Montag, 14.
 * September, 15:00 bis 16:00“. `null` where the clocks agree, and while the
 * fields name no instant (a time the clock skips says so itself).
 */
export function deviceHint(
  fields: ClockFields,
  deviceZone: string | null,
  language: string,
  t: Translate,
): ZoneSentence | null {
  if (fieldClockName(fields.clock, deviceZone) === null) return null;
  const at = fieldInstants(fields);
  if (at === null) return null;
  const name = deviceClockName(deviceZone);
  return {
    label:
      name === null
        ? t('dialogs.event.zone.deviceHintLabelPlain')
        : t('dialogs.event.zone.deviceHintLabel', { clock: name }),
    value: clockRangeText(at.start, at.end, language, deviceFormatterZone(deviceZone), t),
  };
}

/**
 * The sentences about the series' zone after the end time, each its own
 * read-only field labelled „Zeitzone der Serie“ (question 9 of stage 12):
 * a zone this device cannot use, a new series on UTC because the calendar
 * cannot store the device's zone (22a), and a zone the target calendar cannot
 * store (139a).
 */
export function zoneSentences(view: ZoneFormView, t: Translate): ZoneSentence[] {
  const label = t('dialogs.seriesTimeZone.title');
  const sentences: ZoneSentence[] = [];
  if (view.unusable !== null) {
    sentences.push({ label, value: t('dialogs.event.zone.unusable', { clock: zoneCityName(view.unusable) }) });
  }
  if (view.calendarRefuses !== null) {
    sentences.push({
      label,
      value: t('dialogs.event.zone.calendarRefuses', { clock: zoneCityName(view.calendarRefuses) }),
    });
  }
  if (view.notStorable !== null) {
    sentences.push({ label, value: t('dialogs.event.zone.notStorable', { clock: zoneCityName(view.notStorable) }) });
  }
  return sentences;
}

/** The hint in the choice's place for "this and all following" (21a):
 *  „Zeitzone der Serie: New York. Ändern lässt sie sich, wenn du die ganze
 *  Serie bearbeitest.“ `null` elsewhere. */
export function followingHint(view: ZoneFormView, t: Translate): ZoneSentence | null {
  if (view.choice !== 'hint') return null;
  return {
    label: t('dialogs.seriesTimeZone.title'),
    value: t('dialogs.event.zone.following', { clock: zoneCityName(view.zone) }),
  };
}

/** The message for a typed time the clock skips: „02:30 gibt es am 29. März
 *  2026 in Berlin nicht: Die Uhren werden vorgestellt. Wähle eine andere
 *  Uhrzeit.“ */
export function gapMessage(
  gap: Extract<FieldInstant, { kind: 'gap' }>,
  language: string,
  t: Translate,
): string {
  const date = new Intl.DateTimeFormat(language, {
    day: 'numeric',
    month: 'long',
    year: 'numeric',
    timeZone: 'UTC',
  }).format(new Date(`${gap.date}T00:00:00Z`));
  return t('dialogs.event.zone.gap', { time: gap.time, date, clock: zoneCityName(gap.clock) });
}

/** The gap message of each field, `null` where it names an instant or is not
 *  read yet: the editors mark that time field invalid and show the message. */
export function clockFieldGaps(
  fields: ClockFields,
  language: string,
  t: Translate,
): { start: string | null; end: string | null } {
  const say = (instant: FieldInstant) => (instant.kind === 'gap' ? gapMessage(instant, language, t) : null);
  return {
    start: say(fieldInstant(fields.start, fields.clock)),
    end: say(fieldInstant(fields.end, fields.clock)),
  };
}

/**
 * What is said once the repeat field switched the fields to another clock
 * (134a: on the desktop when the field is left, on the phone after its choice
 * closes). `before` is the clock the fields stood on when the field was
 * entered, `fields` the fields now. `null` where the clock the labels name did
 * not change, for example where the series' clock is the device's.
 *
 * Repeat off: „Keine Wiederholung. Die Zeiten stehen jetzt auf der Uhr dieses
 * Geräts: Montag, 14. September, 15:00 bis 16:00.“ Repeat on, said as a
 * choice is: „Zeitzone der Serie: New York. Die Zeiten stehen jetzt auf dieser
 * Uhr: …“. Fields that name no instant are said without the times.
 */
export function repeatClockAnnouncement(
  before: FieldClock,
  fields: ClockFields,
  deviceZone: string | null,
  language: string,
  t: Translate,
): string | null {
  const was = fieldClockName(before, deviceZone);
  const now = fieldClockName(fields.clock, deviceZone);
  if (was === now) return null;
  const at = fieldInstants(fields);
  if (now === null) {
    return at === null
      ? t('dialogs.event.zone.repeatOffTyped')
      : t('dialogs.event.zone.repeatOff', {
          times: clockRangeText(at.start, at.end, language, deviceFormatterZone(deviceZone), t),
        });
  }
  const zone = fields.clock.kind === 'series' ? fields.clock.tzid : null;
  return at === null
    ? t('dialogs.event.zone.repeatOnTyped', { clock: now })
    : t('dialogs.event.zone.repeatOn', {
        clock: now,
        times: clockRangeText(at.start, at.end, language, clockIntlZone(zone), t),
      });
}

/**
 * Why a save is refused for the series' clock, worded, in this order: a typed
 * start or end the clock skips („Lücke“), changed times or a changed rule of a
 * series whose zone this device cannot use, a zone the target calendar cannot
 * store (139a). `null` where the save may go ahead.
 */
export function zoneSaveRefusal(args: {
  fields: ClockFields;
  view: ZoneFormView;
  form: SeriesZoneForm;
  changed: { times: boolean; rule: boolean };
  language: string;
  t: Translate;
}): string | null {
  const { fields, view, form, changed, language, t } = args;
  const gaps = clockFieldGaps(fields, language, t);
  if (gaps.start !== null) return gaps.start;
  if (gaps.end !== null) return gaps.end;
  const unusable = unusableZoneRefusal(view, form, changed);
  if (unusable !== null) return t('dialogs.event.zone.unusableSave', { clock: zoneCityName(unusable) });
  if (view.notStorable !== null) {
    return t('dialogs.event.zone.notStorable', { clock: zoneCityName(view.notStorable) });
  }
  return null;
}
