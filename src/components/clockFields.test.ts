import { afterEach, describe, expect, it, vi } from "vitest";

import {
  DEVICE_CLOCK,
  changeClockField,
  dateInput,
  fieldAt,
  fieldInstant,
  fieldsAt,
  keepingInstants,
  keepingWallTimes,
  sameClockAsDevice,
  seriesFieldClock,
  timeInput,
  type ClockFields,
  type FieldClock,
} from "@aperio/shared";

/**
 * Date and time fields on a clock (stage 8 of DESIGN-series-time-zone.md):
 * a series' fields read through the real WebAssembly module, so these hold
 * on any machine; the device's fields read this runtime's own clock, and CI
 * runs them in New York and Auckland as well as UTC.
 */

const BERLIN: FieldClock = { kind: "series", tzid: "Europe/Berlin" };
const NEW_YORK: FieldClock = { kind: "series", tzid: "America/New_York" };

/** Fields typed by hand on `clock`, holding nothing. */
function typed(
  clock: FieldClock,
  start: [string, string],
  end: [string, string],
): ClockFields {
  return {
    clock,
    start: { date: start[0], time: start[1], at: null },
    end: { date: end[0], time: end[1], at: null },
  };
}

describe("a stored instant on a series clock", () => {
  it("shows on the clock and is held verbatim", () => {
    const fields = fieldsAt(
      "2026-11-02T08:00:00+00:00",
      "2026-11-02T09:00:00Z",
      BERLIN,
    );
    expect(fields.start).toEqual({
      date: "2026-11-02",
      time: "09:00",
      at: "2026-11-02T08:00:00+00:00",
    });
    expect(fields.end).toEqual({
      date: "2026-11-02",
      time: "10:00",
      at: "2026-11-02T09:00:00Z",
    });
    // Untouched, the field names its stored text, not a respelling of it.
    expect(fieldInstant(fields.start, BERLIN)).toEqual({
      kind: "at",
      at: "2026-11-02T08:00:00+00:00",
    });
  });

  it("keeps the second copy of a doubled hour, which typing would not reach", () => {
    // 02:30 shows twice in Berlin on 25 October 2026; this is the second.
    const field = fieldAt("2026-10-25T01:30:00Z", BERLIN);
    expect(field).toMatchObject({ date: "2026-10-25", time: "02:30" });
    expect(fieldInstant(field, BERLIN)).toEqual({
      kind: "at",
      at: "2026-10-25T01:30:00Z",
    });
    // Typed, the same values take the earlier instant.
    expect(fieldInstant({ ...field, at: null }, BERLIN)).toEqual({
      kind: "at",
      at: "2026-10-25T00:30:00Z",
    });
  });

  it("keeps seconds the fields do not show", () => {
    // Paris kept +00:09:21 until 1911.
    const field = fieldAt("1900-01-01T00:00:00Z", {
      kind: "series",
      tzid: "Europe/Paris",
    });
    expect(field).toEqual({
      date: "1900-01-01",
      time: "00:09",
      at: "1900-01-01T00:00:00Z",
    });
  });

  it("stands on UTC for no zone, a UTC name and a name tzdata does not know", () => {
    for (const tzid of [null, "Etc/UTC", "Pacific Standard Time"]) {
      expect(
        fieldAt("2026-07-01T00:00:00Z", { kind: "series", tzid }),
      ).toMatchObject({
        date: "2026-07-01",
        time: "00:00",
      });
    }
  });

  it("gives empty fields for a text that names no instant", () => {
    expect(fieldAt("yesterday", BERLIN)).toEqual({
      date: "",
      time: "",
      at: null,
    });
    expect(fieldAt("yesterday", DEVICE_CLOCK)).toEqual({
      date: "",
      time: "",
      at: null,
    });
  });
});

describe("a typed time on a series clock", () => {
  it("reads once", () => {
    expect(
      fieldInstant({ date: "2026-11-02", time: "09:00", at: null }, BERLIN),
    ).toEqual({
      kind: "at",
      at: "2026-11-02T08:00:00Z",
    });
  });

  it("is a gap where the clock skips it, with the parts of the message", () => {
    expect(
      fieldInstant({ date: "2027-03-28", time: "02:30", at: null }, BERLIN),
    ).toEqual({
      kind: "gap",
      date: "2027-03-28",
      time: "02:30",
      clock: "Europe/Berlin",
      offsetBefore: 3600,
      offsetAfter: 7200,
    });
  });

  it("names the zone by its canonical name", () => {
    const named = fieldInstant(
      { date: "2026-03-08", time: "02:30", at: null },
      { kind: "series", tzid: "US/Eastern" },
    );
    expect(named).toMatchObject({ kind: "gap", clock: "America/New_York" });
  });

  it("is unreadable while it is no date and time", () => {
    for (const [date, time] of [
      ["", "09:00"],
      ["2026-11-02", ""],
      ["2026-02-30", "09:00"],
      ["2026-11-02", "9:00"],
    ]) {
      expect(fieldInstant({ date, time, at: null }, BERLIN)).toEqual({
        kind: "unreadable",
      });
    }
  });
});

describe("changing a field on a series clock", () => {
  it("lets the changed field hold nothing, and leaves an unchanged value alone", () => {
    const fields = fieldsAt(
      "2026-10-25T01:30:00Z",
      "2026-10-25T02:30:00Z",
      BERLIN,
    );
    // A picker confirmed without a change keeps the second copy of 02:30.
    expect(changeClockField(fields, "startTime", "02:30")).toBe(fields);
    const next = changeClockField(fields, "startTime", "02:45");
    expect(next.start).toEqual({ date: "2026-10-25", time: "02:45", at: null });
  });

  it("moves the end by the elapsed time when the start moves across a change (102a)", () => {
    // 20:00 CET to 08:00 CEST is 11 hours. A day earlier, both sides are CET,
    // and the 11 hours end at 07:00.
    const fields = typed(
      BERLIN,
      ["2027-03-27", "20:00"],
      ["2027-03-28", "08:00"],
    );
    const next = changeClockField(fields, "startDate", "2027-03-26");
    expect(next.end).toEqual({
      date: "2027-03-27",
      time: "07:00",
      at: "2027-03-27T06:00:00.000Z",
    });
  });

  it("keeps the end on the same wall time when the start crosses into the same offset", () => {
    const fields = typed(
      BERLIN,
      ["2027-03-27", "09:00"],
      ["2027-03-27", "10:00"],
    );
    const next = changeClockField(fields, "startDate", "2027-03-28");
    expect(next.end).toMatchObject({ date: "2027-03-28", time: "10:00" });
  });

  it("moves the end from a skipped start as the old offset reads it, and keeps the duration", () => {
    // 01:30 CET to 03:30 CEST is one hour.
    const fields = typed(
      BERLIN,
      ["2027-03-28", "01:30"],
      ["2027-03-28", "03:30"],
    );
    // 02:15 does not exist. Read with the offset from before the change it is
    // 01:15Z, 45 minutes later than 01:30 CET, so the end moves to 04:15.
    const skipped = changeClockField(fields, "startTime", "02:15");
    expect(fieldInstant(skipped.start, BERLIN)).toMatchObject({ kind: "gap" });
    expect(skipped.end).toMatchObject({ date: "2027-03-28", time: "04:15" });
    // 03:15 CEST is that same 01:15Z, so the end stays: still one hour.
    const fixed = changeClockField(skipped, "startTime", "03:15");
    expect(fixed.end).toMatchObject({ date: "2027-03-28", time: "04:15" });
  });

  it("leaves the end alone while the start is no date and time", () => {
    const fields = typed(
      BERLIN,
      ["2026-11-02", "09:00"],
      ["2026-11-02", "10:00"],
    );
    const next = changeClockField(fields, "startTime", "");
    expect(next.end).toBe(fields.end);
  });

  it("never lets the end come before the start", () => {
    const fields = fieldsAt(
      "2026-11-02T08:00:00Z",
      "2026-11-02T09:00:00Z",
      BERLIN,
    );
    const next = changeClockField(fields, "endDate", "2026-11-01");
    expect(next.end).toEqual(fields.start);
  });

  it("resizes the event when the end moves later", () => {
    const fields = fieldsAt(
      "2026-11-02T08:00:00Z",
      "2026-11-02T09:00:00Z",
      BERLIN,
    );
    const next = changeClockField(fields, "endTime", "11:00");
    expect(next.start).toBe(fields.start);
    expect(fieldInstant(next.end, BERLIN)).toEqual({
      kind: "at",
      at: "2026-11-02T10:00:00Z",
    });
  });
});

describe("moving the fields to another clock", () => {
  it("keeps the instants, and moving back returns the starting values", () => {
    const stored = fieldsAt(
      "2026-10-25T01:30:00Z",
      "2026-10-25T03:00:00Z",
      BERLIN,
    );
    const withEdit = changeClockField(stored, "endTime", "04:30");
    const inNewYork = keepingInstants(withEdit, NEW_YORK);
    expect(inNewYork.start).toEqual({
      date: "2026-10-24",
      time: "21:30",
      at: "2026-10-25T01:30:00Z",
    });
    expect(inNewYork.end).toEqual({
      date: "2026-10-24",
      time: "23:30",
      at: "2026-10-25T03:30:00Z",
    });
    const back = keepingInstants(inNewYork, BERLIN);
    expect(back.start).toEqual(stored.start);
    expect(back.end).toMatchObject({ date: "2026-10-25", time: "04:30" });
    expect(fieldInstant(back.end, BERLIN)).toEqual(
      fieldInstant(withEdit.end, BERLIN),
    );
  });

  it("keeps a value that names no instant as typed", () => {
    const fields = typed(
      BERLIN,
      ["2027-03-28", "02:30"],
      ["2027-03-28", "04:00"],
    );
    const moved = keepingInstants(fields, NEW_YORK);
    expect(moved.start).toEqual({
      date: "2027-03-28",
      time: "02:30",
      at: null,
    });
    expect(moved.end).toMatchObject({ date: "2027-03-27", time: "22:00" });
  });

  it("moves to the device clock and back", () => {
    const stored = fieldsAt(
      "2026-10-25T01:30:00Z",
      "2026-10-25T03:00:00Z",
      BERLIN,
    );
    const onDevice = keepingInstants(stored, DEVICE_CLOCK);
    const local = new Date("2026-10-25T01:30:00Z");
    expect(onDevice.start).toEqual({
      date: dateInput(local),
      time: timeInput(local),
      at: "2026-10-25T01:30:00Z",
    });
    expect(keepingInstants(onDevice, BERLIN)).toEqual(stored);
  });

  it("keeps the typed values of a new event, which then read on the new clock", () => {
    let fields = typed(
      BERLIN,
      ["2026-03-08", "01:30"],
      ["2026-03-08", "02:30"],
    );
    // The start moved the end, which holds its instant until the move.
    fields = changeClockField(fields, "startTime", "01:45");
    expect(fields.end.at).not.toBeNull();
    const moved = keepingWallTimes(fields, NEW_YORK);
    expect(moved.start).toEqual({
      date: "2026-03-08",
      time: "01:45",
      at: null,
    });
    expect(moved.end).toEqual({ date: "2026-03-08", time: "02:45", at: null });
    // 02:45 does not exist in New York on 8 March 2026: a gap at once.
    expect(fieldInstant(moved.end, NEW_YORK)).toMatchObject({
      kind: "gap",
      clock: "America/New_York",
    });
  });
});

describe("the device clock", () => {
  it("reads a stored instant on this runtime and holds it", () => {
    const at = "2026-11-02T08:00:00Z";
    const local = new Date(at);
    expect(fieldAt(at, DEVICE_CLOCK)).toEqual({
      date: dateInput(local),
      time: timeInput(local),
      at,
    });
  });

  it("reads a typed time as this runtime does", () => {
    expect(
      fieldInstant(
        { date: "2026-11-02", time: "09:00", at: null },
        DEVICE_CLOCK,
      ),
    ).toEqual({
      kind: "at",
      at: new Date(2026, 10, 2, 9, 0).toISOString(),
    });
    expect(
      fieldInstant({ date: "2026-11-02", time: "", at: null }, DEVICE_CLOCK),
    ).toEqual({
      kind: "unreadable",
    });
  });

  it("moves the end with the start", () => {
    // No zone CI runs in changes its clock between 2 and 3 November 2026.
    const fields = typed(
      DEVICE_CLOCK,
      ["2026-11-02", "09:00"],
      ["2026-11-02", "10:30"],
    );
    const next = changeClockField(fields, "startDate", "2026-11-03");
    expect(next.end).toMatchObject({ date: "2026-11-03", time: "10:30" });
  });
});

describe("the same clock as the device", () => {
  it("compares canonical names", () => {
    expect(sameClockAsDevice(DEVICE_CLOCK, null)).toBe(true);
    expect(
      sameClockAsDevice(
        { kind: "series", tzid: "Asia/Calcutta" },
        "Asia/Kolkata",
      ),
    ).toBe(true);
    expect(
      sameClockAsDevice(
        { kind: "series", tzid: "Asia/Kolkata" },
        "Asia/Calcutta",
      ),
    ).toBe(true);
    expect(sameClockAsDevice(BERLIN, "Europe/Berlin")).toBe(true);
    expect(sameClockAsDevice(BERLIN, "America/New_York")).toBe(false);
    expect(sameClockAsDevice(BERLIN, null)).toBe(false);
  });

  it("matches a series on UTC with a device on any of tzdata names for UTC only", () => {
    for (const tzid of [null, "Etc/UTC", "Pacific Standard Time"]) {
      const clock: FieldClock = { kind: "series", tzid };
      expect(sameClockAsDevice(clock, "UTC")).toBe(true);
      expect(sameClockAsDevice(clock, "Etc/UTC")).toBe(true);
      expect(sameClockAsDevice(clock, "Europe/Berlin")).toBe(false);
      // A name tzdata does not know says nothing about the device's clock.
      expect(sameClockAsDevice(clock, "Etc/Unknown")).toBe(false);
    }
  });
});

describe("the clock a series stands on here", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("is its own zone, its stored spelling kept", () => {
    expect(seriesFieldClock("Asia/Calcutta")).toEqual({
      clock: { kind: "series", tzid: "Asia/Calcutta" },
      unusable: null,
    });
    expect(seriesFieldClock(undefined)).toEqual({
      clock: { kind: "series", tzid: null },
      unusable: null,
    });
    expect(seriesFieldClock("Etc/UTC")).toEqual({
      clock: { kind: "series", tzid: "Etc/UTC" },
      unusable: null,
    });
  });

  it("is UTC, saying so, where this device cannot use the zone", () => {
    // A zone the core reads but old zone data on a device refuses. Nothing in
    // this file asks Intl about it before, so no formatter for it is cached.
    const Real = Intl.DateTimeFormat;
    vi.spyOn(Intl, "DateTimeFormat").mockImplementation(function (
      locales?: string | string[],
      options?: Intl.DateTimeFormatOptions,
    ) {
      if (options?.timeZone === "America/Ciudad_Juarez") {
        throw new RangeError(
          "Invalid time zone specified: America/Ciudad_Juarez",
        );
      }
      return new Real(locales, options);
    } as unknown as typeof Intl.DateTimeFormat);
    expect(seriesFieldClock("America/Ciudad_Juarez")).toEqual({
      clock: { kind: "series", tzid: null },
      unusable: "America/Ciudad_Juarez",
    });
  });
});
