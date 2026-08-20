import { describe, expect, it } from 'vitest';

import { classify, detectConference, extractUrls } from '@aperio/shared';

/**
 * A real German Webex invitation, as Exchange delivers it.
 *
 * The SAME fixture the Rust twin uses (`crates/cal-core/src/conferencing.rs`),
 * deliberately: the two implementations are kept honest by both having to pass
 * the case that matters. Structure, wording and escaping are verbatim from a
 * captured `text/calendar` part; the names, site, MTID, meeting id and password
 * are invented.
 */
const GERMAN_EXCHANGE_INVITATION = [
  '                Hallo Leonie,',
  '',
  'wie vereinbart, hier der Regeltermin für die Abstimmung deiner Bachelorarbeit.',
  '',
  'Bis dahin!',
  '',
  'Mit lieben Grüßen.',
  '',
  'Toni',
  '________________________________',
  'Nehmen Sie an dieser Videokonferenz teil via https://example.webex.com/example/j.php?MTID=m0123456789abcdef0123456789abcdef',
  '',
  'Besprechungs-ID: 27401156686',
  'Passwort: PteT3RSYi92',
].join('\n');

describe('detectConference', () => {
  it('finds the join link in a real German invitation whose location is empty', () => {
    // Exchange sends `LOCATION;LANGUAGE=de-DE:` — present and empty. A
    // detector that only looked there would find no meeting at all.
    const found = detectConference({
      location: '',
      description: GERMAN_EXCHANGE_INVITATION,
    });
    expect(found?.provider).toBe('webex');
    expect(found?.source).toBe('description');
    expect(found?.joinUrl).toBe(
      'https://example.webex.com/example/j.php?MTID=m0123456789abcdef0123456789abcdef',
    );
  });

  it('carries the details from the invitation own labels, knowing no German', () => {
    const found = detectConference({ description: GERMAN_EXCHANGE_INVITATION });
    // No tel:, no sip: in this invitation — so the machine-readable carriers
    // find nothing, and its password is alphanumeric so no digit heuristic
    // would have either.
    expect(found?.meetingNumber).toBeUndefined();
    expect(found?.password).toBeUndefined();
    expect(found?.labelledDetails).toEqual([
      { label: 'Besprechungs-ID', value: '27401156686' },
      { label: 'Passwort', value: 'PteT3RSYi92' },
    ]);
  });

  it('does not harvest the prose above the link as details', () => {
    const found = detectConference({ description: GERMAN_EXCHANGE_INVITATION });
    expect(found?.labelledDetails).toHaveLength(2);
    expect(found?.labelledDetails.some((d) => d.label === 'Hallo Leonie')).toBe(
      false,
    );
  });

  it('yields English labels from an English invitation, with no branch', () => {
    const found = detectConference({
      description: [
        'Join the meeting: https://example.webex.com/e/j.php?MTID=m1',
        '',
        'Meeting number (access code): 2550 311 3955',
        'Meeting password: ocn114',
      ].join('\n'),
    });
    expect(found?.labelledDetails).toEqual([
      { label: 'Meeting number (access code)', value: '2550 311 3955' },
      { label: 'Meeting password', value: 'ocn114' },
    ]);
  });

  it('is indifferent to the language around the link', () => {
    const url = 'https://example.webex.com/example/j.php?MTID=mabc';
    for (const prose of [
      `Join meeting: ${url}`,
      `Meeting beitreten: ${url}`,
      `Rejoindre la réunion : ${url}`,
      `会議に参加する: ${url}`,
    ]) {
      expect(detectConference({ description: prose })?.joinUrl).toBe(url);
    }
  });

  it('reads the location, because some invitations put the link only there', () => {
    const found = detectConference({
      location: 'https://example.webex.com/e/j.php?MTID=mxyz',
    });
    expect(found?.source).toBe('location');
  });

  it("prefers a provider's own field over anything scraped", () => {
    const found = detectConference({
      providerField: 'https://meet.google.com/abc-defg-hij',
      location: 'https://example.webex.com/e/j.php?MTID=m1',
      description: 'https://example.zoom.us/j/123',
    });
    expect(found?.provider).toBe('googleMeet');
    expect(found?.source).toBe('providerField');
  });

  it('recovers the number and password from a DTMF dial-in when there is one', () => {
    const found = detectConference({
      description: [
        'Beitreten: https://example.webex.com/e/j.php?MTID=m1',
        'Einwahl: tel:+49-555-0100,,*01*25503113955%23626114%23*01*',
      ].join('\n'),
    });
    expect(found?.meetingNumber).toBe('25503113955');
    expect(found?.password).toBe('626114');
  });

  it('gathers details across fields, not only where the link was', () => {
    const found = detectConference({
      location: 'https://example.webex.com/e/j.php?MTID=m1',
      description: 'tel:+49-555-0100,,*01*25503113955%23626114%23*01*',
    });
    expect(found?.source).toBe('location');
    expect(found?.password).toBe('626114');
  });

  it('offers nothing for links that are not meetings', () => {
    for (const description of [
      // Same host, same MTID parameter, entirely different page.
      'https://example.webex.com/e/globalcallin.php?MTID=m99',
      'https://example.webex.com/recordingservice/sites/e/recording/abc',
      // The bare site root — the whole location of one real invitation.
      'https://example.webex.com',
      'https://teams.microsoft.com/l/channel/19%3aabc',
      'Agenda: https://example.com/agenda.pdf',
      '',
    ]) {
      expect(detectConference({ description })).toBeNull();
    }
  });

  it('recognises the other providers too', () => {
    const cases: Array<[string, string]> = [
      ['https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc', 'teams'],
      ['https://example.zoom.us/j/123456789', 'zoom'],
      ['https://meet.google.com/abc-defg-hij', 'googleMeet'],
      ['https://meet.jit.si/AperioTest', 'jitsi'],
      ['https://whereby.com/aperio', 'whereby'],
    ];
    for (const [url, provider] of cases) {
      expect(classify(url)).toBe(provider);
    }
  });
});

describe('extractUrls', () => {
  it('never keeps the punctuation a sentence glued on', () => {
    const cases: Array<[string, string]> = [
      ['Join at https://x.webex.com/e/j.php?MTID=m1.', 'https://x.webex.com/e/j.php?MTID=m1'],
      ['Join at <https://x.webex.com/e/j.php?MTID=m2>', 'https://x.webex.com/e/j.php?MTID=m2'],
      ['Join at https://x.webex.com/e/j.php?MTID=m3%3E', 'https://x.webex.com/e/j.php?MTID=m3'],
      ['(see https://x.webex.com/e/j.php?MTID=m4)', 'https://x.webex.com/e/j.php?MTID=m4'],
    ];
    for (const [text, want] of cases) {
      expect(extractUrls(text)[0]).toBe(want);
    }
  });

  it('takes the longest match in a source', () => {
    // Webex's newer join link nests a shorter-looking URL in its query string.
    const long =
      'https://x.webex.com/wbxmjs/joinservice/sites/x/meeting/download/abc?siteurl=x&MTID=m1';
    const found = detectConference({
      description: `Alt: https://x.webex.com/e/j.php?MTID=m0 Neu: ${long}`,
    });
    expect(found?.joinUrl).toBe(long);
  });
});

describe('DFNconf', () => {
  it('recognises a Pexip room link', () => {
    expect(
      classify('https://conf.dfn.de/webapp/#/?conference=97912345'),
    ).toBe('dfnconf');
    expect(classify('https://conf.dfn.de/webapp/conference/97912345')).toBe(
      'dfnconf',
    );
  });

  it('does NOT offer Join for the documentation site', () => {
    // The manual lives on www.conf.dfn.de and the app on conf.dfn.de. A
    // colleague pasting "see the DFNconf instructions" into an invitation
    // must not produce a Join button that opens a help page.
    expect(
      classify('https://www.conf.dfn.de/dfnconf/anleitungen-und-dokumentation/'),
    ).toBeNull();
  });

  it('does not join the app itself', () => {
    // `/webapp/` with no conference is the landing page.
    expect(classify('https://conf.dfn.de/webapp/')).toBeNull();
    expect(classify('https://conf.dfn.de/')).toBeNull();
  });

  it('leaves the Adobe Connect half alone', () => {
    // DFNconf still runs Adobe Connect on its own host, but its meeting URLs
    // carry no stable path marker — matching the host would offer Join for the
    // login page too. Deliberately unclassified rather than wrongly claimed.
    expect(classify('https://webconf.vc.dfn.de/r/abc123/')).toBeNull();
  });

  it('finds the room link in a real-shaped invitation', () => {
    const found = detectConference({
      location: null,
      description: [
        'Sie sind zu einer Videokonferenz eingeladen.',
        '',
        'Per Browser: https://conf.dfn.de/webapp/#/?conference=97912345',
        'Per SIP: 97912345@conf.dfn.de',
        'Per Telefon: +49 30 200 97912345',
      ].join(String.fromCharCode(10)),
    });
    expect(found?.provider).toBe('dfnconf');
    expect(found?.joinUrl).toBe('https://conf.dfn.de/webapp/#/?conference=97912345');
  });
});

