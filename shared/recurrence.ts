import { RRule, rrulestr } from 'rrule';

// Event recurrence expansion, shared by desktop + mobile. Generic over a
// minimal `RecurringEventLike` so it needs neither side's full `CalendarEvent`
// type (those still live per-app for now); any event with id/start/end + the
// `{rrule, exceptions}` recurrence shape works. Desktop re-exports this from
// `src/intl/recurrence.ts`; mobile imports it directly.

/** The minimal event shape the expander needs. Both the desktop and mobile
 *  `CalendarEvent` satisfy it. */
export interface RecurringEventLike {
  id: string;
  /** RFC-3339 (UTC) start/end of the master event. */
  start: string;
  end: string;
  /** Cancelled/tombstone flag. A cancelled RECURRENCE-ID override (its id carries
   *  `::rid::`) is a pure deletion marker — `expandAll` uses it to suppress the
   *  master's occurrence and then drops it, so a deleted occurrence vanishes. */
  cancelled?: boolean;
  recurrence: {
    rrule: string;
    exceptions: string[];
    /** IANA zone of the master DTSTART (e.g. `America/New_York`), when the
     *  source carried one. Present → expand in this zone so occurrences keep
     *  their local wall-clock across DST; absent/null → expand in UTC. */
    tzid?: string | null;
  } | null;
}

/** An expanded per-occurrence copy of `E`: same fields, but `start`/`end`
 *  shifted to the occurrence, a unique `id`, and the master id kept as
 *  `series_id` so the edit/delete layer can find the underlying row. */
export type ExpandedOccurrence<E extends RecurringEventLike> = E & {
  series_id: string;
  occurrence_start: string;
};

/**
 * Expand a recurring event into all of its occurrences inside `range`.
 *
 * Returns the original event unchanged when there is no recurrence rule.
 * Otherwise produces one copy per occurrence whose `start`/`end` match the
 * occurrence time and whose `id` is suffixed with the occurrence start (ISO) so
 * list keys stay unique; the master `id` is preserved as `series_id`. `EXDATE`
 * entries are honoured (rrule.js filters them out).
 *
 * Time-zone caveat: `start` is RFC-3339 (UTC). rrule.js works in `Date`
 * instants taken from `dtstart`; the result instants are re-serialised via
 * `toISOString()`.
 */
export function expandEvent<E extends RecurringEventLike>(
  event: E,
  range: { start: Date; end: Date },
): (E | ExpandedOccurrence<E>)[] {
  if (!event.recurrence?.rrule) {
    return [event];
  }

  const dtstart = new Date(event.start);
  const dtend = new Date(event.end);
  const duration = dtend.getTime() - dtstart.getTime();
  const tzid = zoneOrNull(event.recurrence.tzid);

  let occurrences: Date[];
  try {
    occurrences = tzid
      ? zonedOccurrences(event.recurrence.rrule, dtstart, tzid, range)
      : utcOccurrences(event.recurrence.rrule, dtstart, range);
  } catch (err) {
    // Bad rule string — fall back to showing the master at its stored start so
    // the user can still see and edit it.
    // eslint-disable-next-line no-console
    console.warn('failed to expand RRULE', event.recurrence.rrule, err);
    return [event];
  }
  if (occurrences.length === 0) {
    return [];
  }

  const exceptions = new Set(
    event.recurrence.exceptions.map((iso) => new Date(iso).getTime()),
  );

  return occurrences
    .filter((d) => !exceptions.has(d.getTime()))
    .map<ExpandedOccurrence<E>>((occStart) => {
      const occEnd = new Date(occStart.getTime() + duration);
      return {
        ...event,
        id: `${event.id}@${occStart.toISOString()}`,
        series_id: event.id,
        occurrence_start: occStart.toISOString(),
        start: occStart.toISOString(),
        end: occEnd.toISOString(),
      };
    });
}

/** The recurrence zone, or `null` to expand in UTC (floating / `Z` / all-day). */
function zoneOrNull(tzid: string | null | undefined): string | null {
  return !tzid || tzid.toUpperCase() === 'UTC' ? null : tzid;
}

function buildRule(rruleBody: string, dtstart: Date): RRule {
  // rrulestr accepts a full RFC-5545 "RRULE:..." block; if the stored string is
  // just the body (FREQ=...;BYDAY=...) prepend the marker.
  const body = rruleBody.trim();
  const text = body.toUpperCase().startsWith('RRULE:') ? body : `RRULE:${body}`;
  return rrulestr(text, { dtstart }) as RRule;
}

/** Occurrences of a zone-less rule: rrule.js iterates the UTC instant directly
 *  (floating times read as UTC) — the historical behaviour, unchanged. */
function utcOccurrences(
  rruleBody: string,
  dtstart: Date,
  range: { start: Date; end: Date },
): Date[] {
  // `inc = true` makes the boundaries inclusive so an event starting exactly on
  // a boundary appears.
  return buildRule(rruleBody, dtstart).between(range.start, range.end, true);
}

/**
 * Occurrences of a zoned rule — DST-correct AND independent of the process's own
 * time zone. rrule.js's built-in `tzid` mode is neither (its output is offset by
 * the host's zone), so we iterate the rule purely in the event's WALL-CLOCK space
 * — rrule.js with no tzid treats the dtstart's UTC fields as the recurrence
 * anchor — then convert each wall-clock occurrence back to a real UTC instant in
 * `tzid` ourselves via `Intl`. Every emitted instant is real UTC, so day
 * bucketing and EXDATE / RECURRENCE-ID matching keep comparing real instants.
 */
function zonedOccurrences(
  rruleBody: string,
  dtstart: Date,
  tzid: string,
  range: { start: Date; end: Date },
): Date[] {
  let dtstartWall: Date;
  try {
    dtstartWall = realToWall(dtstart, tzid); // probes the zone (throws if bad)
  } catch {
    // Unresolvable IANA zone (a typo, a Windows zone name, or a custom VTIMEZONE
    // id `Intl` can't load) — degrade to UTC expansion rather than dropping the
    // series. Worst case is the pre-fix behaviour, never worse.
    return utcOccurrences(rruleBody, dtstart, range);
  }
  // Iterate UNTIL in wall-clock space too, else a bounded series' final cutoff is
  // off by the zone offset.
  const rule = buildRule(shiftUntilToWall(rruleBody, tzid), dtstartWall);
  // Pad the wall-clock window a day each side (any zone offset is < 24h) so no
  // occurrence near a real-range edge is missed; the precise real filter trims.
  const lo = new Date(realToWall(range.start, tzid).getTime() - DAY_MS);
  const hi = new Date(realToWall(range.end, tzid).getTime() + DAY_MS);
  return rule
    .between(lo, hi, true)
    .map((wall) => wallToReal(wall, tzid))
    .filter((real) => real >= range.start && real <= range.end);
}

// `Intl.DateTimeFormat` construction is comparatively costly and we call it once
// per occurrence; memoise one formatter per zone. Constructing it throws
// `RangeError` for a zone Intl can't resolve, which is how `zonedOccurrences`
// detects a bad zone.
const zoneFormatters = new Map<string, Intl.DateTimeFormat>();
function zoneFormatter(tzid: string): Intl.DateTimeFormat {
  let f = zoneFormatters.get(tzid);
  if (!f) {
    f = new Intl.DateTimeFormat('en-US', {
      timeZone: tzid,
      hourCycle: 'h23',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });
    zoneFormatters.set(tzid, f);
  }
  return f;
}

/**
 * Offset in ms such that `wall-clock = instant + offset` for `tzid` at `instant`
 * (e.g. −4h for America/New_York in summer). Computed via `Intl` with an
 * explicit `timeZone`, so it never depends on the process's own zone.
 */
function zoneOffsetMs(instant: Date, tzid: string): number {
  const parts = zoneFormatter(tzid).formatToParts(instant);
  const get = (type: string): number =>
    Number(parts.find((p) => p.type === type)?.value);
  const asIfUtc = Date.UTC(
    get('year'),
    get('month') - 1,
    get('day'),
    get('hour') % 24, // some engines report midnight as 24 under h23
    get('minute'),
    get('second'),
  );
  return asIfUtc - instant.getTime();
}

/** Real UTC instant → a Date whose UTC fields hold its wall-clock in `tzid`. */
function realToWall(instant: Date, tzid: string): Date {
  return new Date(instant.getTime() + zoneOffsetMs(instant, tzid));
}

const DAY_MS = 86_400_000;

/** Inverse of {@link realToWall}: a wall-clock-as-UTC Date → the real instant in
 *  `tzid`, resolving DST edges deterministically. A spring-forward GAP time (no
 *  such reading on the local clock) rounds FORWARD to the first valid instant; a
 *  fall-back AMBIGUOUS time (two readings) takes the FIRST (earlier) instant. */
function wallToReal(wall: Date, tzid: string): Date {
  const t = wall.getTime();
  // Bracket any transition near the wall time: DST changes once per ~6 months
  // and at most once within a day, so the offsets a day before/after pin it.
  const offBefore = zoneOffsetMs(new Date(t - DAY_MS), tzid);
  const offAfter = zoneOffsetMs(new Date(t + DAY_MS), tzid);
  if (offBefore === offAfter) {
    return new Date(t - offBefore); // no transition in range → unambiguous
  }
  const candBefore = t - offBefore;
  const candAfter = t - offAfter;
  // A candidate is real iff its actual offset matches the side it came from.
  const beforeValid = zoneOffsetMs(new Date(candBefore), tzid) === offBefore;
  const afterValid = zoneOffsetMs(new Date(candAfter), tzid) === offAfter;
  if (beforeValid && afterValid) {
    return new Date(Math.min(candBefore, candAfter)); // overlap → first reading
  }
  if (beforeValid) return new Date(candBefore);
  if (afterValid) return new Date(candAfter);
  return new Date(Math.max(candBefore, candAfter)); // gap → round forward
}

/** Rewrite a real-UTC `UNTIL=…Z` bound into wall-clock space so it lines up with
 *  the wall-clock iteration above; other UNTIL forms are left untouched. */
function shiftUntilToWall(rruleBody: string, tzid: string): string {
  return rruleBody.replace(
    /UNTIL=(\d{8})T(\d{6})Z/i,
    (whole, d: string, t: string) => {
      const real = new Date(
        Date.UTC(
          Number(d.slice(0, 4)),
          Number(d.slice(4, 6)) - 1,
          Number(d.slice(6, 8)),
          Number(t.slice(0, 2)),
          Number(t.slice(2, 4)),
          Number(t.slice(4, 6)),
        ),
      );
      if (Number.isNaN(real.getTime())) return whole;
      const w = realToWall(real, tzid);
      const p2 = (n: number): string => String(n).padStart(2, '0');
      return (
        `UNTIL=${w.getUTCFullYear()}${p2(w.getUTCMonth() + 1)}${p2(w.getUTCDate())}` +
        `T${p2(w.getUTCHours())}${p2(w.getUTCMinutes())}${p2(w.getUTCSeconds())}Z`
      );
    },
  );
}

/**
 * The host's current IANA time zone (e.g. `America/New_York`), or `null` when
 * the runtime can't report a usable one (or only reports plain UTC).
 */
export function localTimeZone(): string | null {
  try {
    const tz = new Intl.DateTimeFormat().resolvedOptions().timeZone;
    return tz && tz.toUpperCase() !== 'UTC' ? tz : null;
  } catch {
    return null;
  }
}

/**
 * Stamp the host's local zone onto a freshly-created TIMED recurring rule so it
 * expands DST-correctly — a series created here at 19:00 keeps 19:00 across DST,
 * the same guarantee a zoned CalDAV series gets. Leaves all-day rules (they use
 * the date-based path), already-zoned rules, and non-recurring events untouched.
 * Use at CREATE time only; editing keeps whatever zone the series already has.
 */
export function withCreatedRecurrenceZone<
  R extends { rrule: string; exceptions: string[]; tzid?: string | null },
>(recurrence: R | null, allDay: boolean): R | null {
  if (!recurrence || allDay || recurrence.tzid) {
    return recurrence;
  }
  const tz = localTimeZone();
  return tz ? { ...recurrence, tzid: tz } : recurrence;
}

/**
 * Marker in an override instance's id, separating the recurring series'
 * `{href}|{uid}` from the RECURRENCE-ID instant it replaces (e.g.
 * `…|uid::rid::2026-06-14T13:00:00Z`). Mirrors `RECURRENCE_ID_MARKER` in the
 * CalDAV adapter (`crates/adapter-caldav/src/mapping.rs`), which mints these
 * ids — keep the two in sync.
 */
const RECURRENCE_ID_MARKER = '::rid::';

/**
 * The original occurrence instant (ISO) a RECURRENCE-ID override replaces, or
 * `null` for a master / plain event. Read off the id the CalDAV adapter minted.
 */
export function overrideRecurrenceIso<E extends RecurringEventLike>(
  event: E,
): string | null {
  const i = event.id.indexOf(RECURRENCE_ID_MARKER);
  return i < 0 ? null : event.id.slice(i + RECURRENCE_ID_MARKER.length);
}

/**
 * The series (master) id a RECURRENCE-ID override belongs to, or `null` for a
 * master / plain event.
 */
export function overrideSeriesId<E extends RecurringEventLike>(
  event: E,
): string | null {
  const i = event.id.indexOf(RECURRENCE_ID_MARKER);
  return i < 0 ? null : event.id.slice(0, i);
}

/**
 * Walk events through {@link expandEvent}, flatten, and sort chronologically.
 * The result is `E[]` (occurrences are assignment-compatible with `E`); callers
 * that need the underlying series read `series_id` via {@link seriesIdOf}.
 *
 * RECURRENCE-ID overrides (a recurring series' modified single instances) arrive
 * as separate non-recurring events whose id carries both the master id and the
 * occurrence they replace. We drop the master's own copy of each overridden
 * occurrence so the override stands in for it (at its possibly-moved time) —
 * otherwise the day shows the instance twice, or (before the adapter fix that
 * stopped them colliding in the cache) not at all.
 */
export function expandAll<E extends RecurringEventLike>(
  events: E[],
  range: { start: Date; end: Date },
): E[] {
  // Per series, the original occurrence instants an override supersedes.
  let overridden: Map<string, Set<number>> | null = null;
  for (const ev of events) {
    const iso = overrideRecurrenceIso(ev);
    const seriesId = overrideSeriesId(ev);
    if (iso == null || seriesId == null) continue;
    const t = new Date(iso).getTime();
    if (Number.isNaN(t)) continue;
    (overridden ??= new Map());
    let set = overridden.get(seriesId);
    if (!set) {
      set = new Set();
      overridden.set(seriesId, set);
    }
    set.add(t);
  }

  const out = events.flatMap((ev) => {
    // A CANCELLED RECURRENCE-ID override is a deletion tombstone: it exists only to
    // suppress the master's occurrence (already recorded in `overridden` above) and
    // carries no content of its own, so never emit it as a visible event — the
    // deleted occurrence must VANISH, not linger as an empty cancelled row. This is
    // independent of the show-cancelled-events toggle, which governs whole cancelled
    // events (those keep a normal id, no `::rid::`). A MODIFIED (non-cancelled)
    // override still renders at its moved time.
    if (ev.cancelled && overrideRecurrenceIso(ev) != null) return [];
    const occs = expandEvent(ev, range);
    const replaced = ev.recurrence?.rrule ? overridden?.get(ev.id) : undefined;
    if (!replaced || replaced.size === 0) return occs;
    // Drop the master occurrences an override stands in for (matched on the
    // original occurrence instant). Keep anything we can't place — never hide.
    return occs.filter((o) => {
      const iso = occurrenceIsoOf(o);
      return iso == null || !replaced.has(new Date(iso).getTime());
    });
  });
  out.sort((a, b) => a.start.localeCompare(b.start));
  return out;
}

/** Type guard: a synthetic occurrence vs a regular/master event. */
export function isExpandedOccurrence<E extends RecurringEventLike>(
  event: E,
): event is ExpandedOccurrence<E> {
  return (
    'series_id' in event &&
    typeof (event as ExpandedOccurrence<E>).series_id === 'string'
  );
}

/**
 * Underlying series id for an event row: the master's `series_id` for an
 * expanded occurrence, else `event.id`. Keying off `series_id` (not
 * `id.split('@')[0]`) is the canonical fix — Aperio CalDAV UIDs themselves
 * contain `@aperio`, so the split shortcut dropped half the master UID.
 */
export function seriesIdOf<E extends RecurringEventLike>(event: E): string {
  return isExpandedOccurrence(event) ? event.series_id : event.id;
}

/**
 * Occurrence-start ISO for an expanded occurrence, else `null` for a master.
 * Drives "delete only this occurrence" (append onto the master's EXDATE).
 */
export function occurrenceIsoOf<E extends RecurringEventLike>(
  event: E,
): string | null {
  return isExpandedOccurrence(event) ? event.occurrence_start : null;
}

/** UTC "basic" RFC-5545 timestamp (`YYYYMMDDTHHMMSSZ`) — the form an RRULE
 *  `UNTIL` takes when the series has a zoned/timed DTSTART. */
function formatRRuleUntilUtc(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return (
    `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}` +
    `T${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}Z`
  );
}

/** Date-only RFC-5545 value (`YYYYMMDD`) — the form an RRULE `UNTIL` MUST take
 *  when the series has a DATE-valued (all-day) DTSTART. Per RFC-5545 §3.3.10 the
 *  UNTIL value type must match DTSTART's, so a datetime UNTIL on an all-day
 *  series is malformed and strict providers (iCloud CalDAV) may reject the write
 *  or silently drop the RRULE. */
function formatRRuleUntilDate(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getUTCFullYear()}${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}`;
}

/** Parse an RRULE `UNTIL` value (`YYYYMMDD` or `YYYYMMDDTHHMMSSZ`) to an epoch
 *  instant in ms for chronological comparison. A date-only value is inclusive of
 *  the whole day, so it maps to that day's last instant — this is what lets a
 *  date-only and a datetime bound compare correctly (a naive lexicographic
 *  compare mis-ranks same-date values because the date-only string is a prefix of
 *  the datetime form). Returns NaN for an unparseable value. */
function untilInstantMs(until: string): number {
  const m = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})Z?)?$/.exec(
    until.trim(),
  );
  if (!m) return Number.NaN;
  const [, y, mo, d, hh, mm, ss] = m;
  if (hh == null) {
    return Date.UTC(+y, +mo - 1, +d, 23, 59, 59, 999);
  }
  return Date.UTC(+y, +mo - 1, +d, +hh, +mm, +ss);
}

/**
 * Truncate a recurrence RULE so it keeps only occurrences STRICTLY BEFORE
 * `cutoff` — the "this and all following occurrences" split point.
 *
 * Sets `UNTIL` to one second before `cutoff` (RFC-5545 `UNTIL` is inclusive) and
 * drops any `COUNT` (a rule can't carry both). An existing earlier `UNTIL` wins
 * (we never EXTEND a series). `cutoff` is the target occurrence's UTC instant, so
 * the comparison is zone-independent: the occurrence at `cutoff` and everything
 * after it fall away, everything before stays.
 *
 * `opts.allDay` picks the emitted UNTIL value type: an all-day (DATE-valued)
 * series gets a date-only `YYYYMMDD` UNTIL, a timed series the datetime
 * `YYYYMMDDTHHMMSSZ` form — the value type MUST match DTSTART's or strict
 * providers drop the rule.
 *
 * Returns the rule body without a leading `RRULE:` (matching how the app stores
 * recurrence rules).
 */
export function truncateRRuleBefore(
  rrule: string,
  cutoff: Date,
  opts: { allDay?: boolean } = {},
): string {
  const lastKept = new Date(cutoff.getTime() - 1000);
  const body = rrule.trim().replace(/^RRULE:/i, '');
  const kept: string[] = [];
  let existingUntil: string | null = null;
  for (const part of body.split(';')) {
    if (!part) continue;
    const eq = part.indexOf('=');
    const key = part.slice(0, eq).toUpperCase();
    if (key === 'COUNT') continue;
    if (key === 'UNTIL') {
      existingUntil = part.slice(eq + 1);
      continue;
    }
    kept.push(part);
  }
  // Keep whichever ends the series sooner, comparing on normalized instants so a
  // date-only existing UNTIL and the computed bound rank chronologically. Re-emit
  // in the series' own value type (date-only for all-day) regardless of which
  // bound won, so an all-day series never carries a datetime UNTIL.
  let boundMs = lastKept.getTime();
  if (existingUntil) {
    const ex = untilInstantMs(existingUntil);
    if (Number.isFinite(ex) && ex < boundMs) boundMs = ex;
  }
  const bound = new Date(boundMs);
  const finalUntil = opts.allDay
    ? formatRRuleUntilDate(bound)
    : formatRRuleUntilUtc(bound);
  kept.push(`UNTIL=${finalUntil}`);
  return kept.join(';');
}

/**
 * Split a recurrence rule for "edit this and all following occurrences": the
 * ORIGINAL series is truncated to end just before `cutoff`, and a NEW series
 * takes over from `cutoff` (created with the edited fields by the caller).
 *
 * Returns `{ oldRule, newRule }` (rule bodies, no `RRULE:` prefix). The new
 * series reuses the same pattern; only a COUNT-bounded series needs adjusting —
 * its remaining count is `COUNT - occurrencesBeforeCutoff` (clamped to >= 1). An
 * UNTIL/absolute-ended or open-ended series carries its end unchanged: the same
 * UNTIL still bounds the new series from its later start, and an open series
 * stays open.
 */
export function splitRRuleForEdit(
  rrule: string,
  cutoff: Date,
  occurrencesBeforeCutoff: number,
  opts: { allDay?: boolean } = {},
): { oldRule: string; newRule: string } {
  const oldRule = truncateRRuleBefore(rrule, cutoff, opts);
  const body = rrule.trim().replace(/^RRULE:/i, '');
  const parts = body.split(';').filter(Boolean);
  const countIdx = parts.findIndex((p) => p.toUpperCase().startsWith('COUNT='));
  if (countIdx === -1) {
    return { oldRule, newRule: body };
  }
  const count = Number(parts[countIdx].slice('COUNT='.length));
  const remaining = Number.isFinite(count)
    ? Math.max(1, count - Math.max(0, occurrencesBeforeCutoff))
    : count;
  const newParts = parts.map((p, i) =>
    i === countIdx ? `COUNT=${remaining}` : p,
  );
  return { oldRule, newRule: newParts.join(';') };
}
