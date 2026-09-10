import { describe, expect, it } from 'vitest';

import { conferenceDetailRows, type ConferenceDetail } from '@aperio/shared';

/**
 * Which detail rows a conference section shows.
 *
 * Both surfaces used to pick ONE list — `derived.length > 0 ? derived :
 * labelled` — so the moment Aperio recovered any meeting number at all, the
 * line the invitation itself contained was suppressed entirely. That is the
 * wrong way round: the derived value is parsed out of a tone sequence, the
 * labelled one is what a person typed, and only the parsed one was shown, so
 * there was nothing to notice a mis-parse against.
 *
 * It matters more than it looks because the conference section IS the meeting
 * details for a screen-reader user. A wrong number that hides the right one is
 * not something you can work around by looking further down the page.
 */
describe('conferenceDetailRows', () => {
  const d = (label: string, value: string): ConferenceDetail => ({ label, value });

  it('shows the derived rows first, then the invitation’s own', () => {
    expect(
      conferenceDetailRows(
        [d('Besprechungs-ID', '25503113955')],
        [d('Passwort', '626114'), d('Einwahl', '+49 30 1234567')],
      ),
    ).toEqual([
      d('Besprechungs-ID', '25503113955'),
      d('Passwort', '626114'),
      d('Einwahl', '+49 30 1234567'),
    ]);
  });

  it('does not read the same value out twice', () => {
    // The common case: Aperio parsed the number out of the tone sequence AND
    // the invitation states it in words. One row, not two.
    expect(
      conferenceDetailRows(
        [d('Besprechungs-ID', '25503113955')],
        [d('Meeting number (access code)', '25503113955')],
      ),
    ).toEqual([d('Besprechungs-ID', '25503113955')]);
  });

  it('matches on the VALUE, not the label', () => {
    // The two lists label the same thing differently on purpose — Aperio's
    // translated word against the sender's verbatim one — so comparing labels
    // would never match and every meeting would announce its number twice.
    const rows = conferenceDetailRows(
      [d('Password', 'hunter2')],
      [d('Kennwort', 'hunter2')],
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].label).toBe('Password');
  });

  it('ignores a trailing space on the invitation’s line', () => {
    // Invitations wrap and pad; the parsed form does not.
    expect(
      conferenceDetailRows([d('ID', '25503113955')], [d('ID', ' 25503113955 ')]),
    ).toEqual([d('ID', '25503113955')]);
  });

  it('keeps two values that differ anywhere but the ends', () => {
    // Grouped and ungrouped are two different claims about the meeting, and
    // which one dials is exactly what the user needs to see. Nothing here
    // normalises inner whitespace away.
    expect(
      conferenceDetailRows(
        [d('ID', '25503113955')],
        [d('Besprechungs-ID', '2550 311 3955')],
      ),
    ).toHaveLength(2);
  });

  it('shows the invitation’s rows when nothing was derived', () => {
    // The Exchange-organiser case the labelled list exists for: no `tel:`, no
    // `sip:`, everything behind the sender's own words.
    expect(
      conferenceDetailRows([], [d('Besprechungs-ID', '2740 1156 686')]),
    ).toEqual([d('Besprechungs-ID', '2740 1156 686')]);
  });

  it('shows the derived rows when the invitation labelled nothing', () => {
    expect(conferenceDetailRows([d('ID', '123')], [])).toEqual([d('ID', '123')]);
  });

  it('is empty when there is nothing at all', () => {
    expect(conferenceDetailRows([], [])).toEqual([]);
  });
});
