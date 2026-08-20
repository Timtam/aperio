/**
 * Find the online meeting in an event — whoever created it.
 *
 * The JS twin of `crates/cal-core/src/conferencing.rs`. Both exist because the
 * detection is presentational — it turns text the frontend already holds into
 * an affordance — while the Rust side is where the host will need the same
 * answer once Aperio manages its own meetings. They are kept honest by sharing
 * the load-bearing fixture: the real German Webex invitation below is the same
 * one the Rust tests use, so a divergence in the case that matters fails a test
 * on one side or the other.
 *
 * ## Why URLs and not text
 *
 * The obvious approach is to read an invitation the way a human does —
 * "Meeting number (access code)", "Besprechungs-ID". It does not survive
 * contact with reality: Webex localises its template across more than 26
 * languages, site administrators can edit it, and Cisco publishes no
 * machine-readable version. That would make the Join button depend on Cisco's
 * prose in a language nobody chose. `j.php?MTID=` reads the same everywhere.
 *
 * The details are handled by treating the LABEL as data rather than as
 * vocabulary — see {@link labelledLinesNear}.
 *
 * ## What the standards offer, honestly
 *
 * RFC 7986 §5.11 defines `CONFERENCE;VALUE=URI` for exactly this and
 * essentially nobody emits it. A real Webex invitation delivered through
 * Exchange carries no `CONFERENCE`, no `X-WEBEX-*` and no
 * `X-MICROSOFT-SKYPETEAMS*` — only `X-MICROSOFT-CDO-*`, which say nothing about
 * conferencing — and its `LOCATION` is present and empty. Microsoft has the same
 * gap and fills it the same way: its Outlook add-in model matches a body
 * template the vendor registers, i.e. Microsoft also falls back to text.
 */

/** Which service a join link belongs to. */
export type ConferenceProvider =
  | 'webex'
  | 'teams'
  | 'zoom'
  | 'googleMeet'
  | 'jitsi'
  | 'goToMeeting'
  | 'bigBlueButton'
  | 'dfnconf'
  | 'whereby'
  | 'other';

/** Where the link was found, ranked from most to least authoritative. */
export type ConferenceSource =
  | 'providerField'
  | 'icalendarConference'
  | 'vendorProperty'
  | 'location'
  | 'description';

export interface ConferenceLink {
  joinUrl: string;
  provider: ConferenceProvider;
  source: ConferenceSource;
  /** Recovered without reading prose, from a SIP address or a DTMF sequence. */
  meetingNumber?: string;
  password?: string;
  sipAddress?: string;
  /** A dial-in number as a `tel:` URI. */
  phone?: string;
  /**
   * The `label: value` lines the invitation puts beside the link, with the
   * labels exactly as written. See {@link labelledLinesNear}.
   */
  labelledDetails: Array<{ label: string; value: string }>;
}

export interface ConferenceSources {
  /** A join URL a provider stated outright — Google conferenceData, Graph
   *  onlineMeeting. Nothing can be more authoritative than that. */
  providerField?: string | null;
  /** RFC 7986 `CONFERENCE` values. */
  icalendarConference?: string[];
  /** Vendor X-property values. */
  vendorProperties?: string[];
  location?: string | null;
  description?: string | null;
}

/** At most this many `label: value` pairs; a conferencing block has a handful. */
const MAX_LABELLED = 6;
const MAX_LABEL_CHARS = 40;
const MAX_VALUE_CHARS = 60;

/**
 * Find the meeting, or decide there is none.
 *
 * Sources are tried in order of authority. Within one source the LONGEST
 * matching URL wins: Webex's newer join links nest a shorter-looking URL inside
 * their query string, and taking the first match hands back a fragment that
 * joins nothing.
 */
export function detectConference(
  sources: ConferenceSources,
): ConferenceLink | null {
  const ordered: Array<[ConferenceSource, Array<string | null | undefined>]> = [
    ['providerField', [sources.providerField]],
    ['icalendarConference', sources.icalendarConference ?? []],
    ['vendorProperty', sources.vendorProperties ?? []],
    ['location', [sources.location]],
    ['description', [sources.description]],
  ];

  for (const [source, texts] of ordered) {
    let best: { url: string; provider: ConferenceProvider } | null = null;
    for (const text of texts) {
      if (!text) continue;
      for (const url of extractUrls(text)) {
        const provider = classify(url);
        if (!provider) continue;
        if (!best || url.length > best.url.length) best = { url, provider };
      }
    }
    if (best) {
      // Details come from the whole source set, not only the field the URL was
      // in: the link is often in the location while the dial-in is in the body.
      const all = [
        sources.providerField,
        sources.location,
        sources.description,
      ].filter((t): t is string => !!t);
      return {
        joinUrl: best.url,
        provider: best.provider,
        source,
        ...extractDetails(all),
        labelledDetails: sources.description
          ? labelledLinesNear(sources.description, best.url)
          : [],
      };
    }
  }
  return null;
}

/**
 * Pull every http(s) URL out of free text, trimming what punctuation around a
 * link would otherwise glue on.
 *
 * The trailing characters are the whole difficulty: a link at the end of a
 * sentence keeping its full stop, a `<url>` keeping its `%3E`, a bracketed link
 * keeping its `)`. Each has been a shipped bug somewhere. Brackets count as
 * trailing only when unbalanced, so a URL legitimately ending in one survives.
 */
export function extractUrls(text: string): string[] {
  const out: string[] = [];
  let i = 0;
  while (i < text.length) {
    const at = text.indexOf('http', i);
    if (at < 0) break;
    const tail = text.slice(at);
    if (!tail.startsWith('http://') && !tail.startsWith('https://')) {
      i = at + 4;
      continue;
    }
    let end = at;
    for (const ch of tail) {
      if (/\s/.test(ch) || ch === '<' || ch === '>' || ch === '"' || ch === "'")
        break;
      end += ch.length;
    }
    let url = text.slice(at, end);
    for (;;) {
      const last = url.at(-1);
      if (last && '.,;:!?'.includes(last)) url = url.slice(0, -1);
      else if (
        last === ')' &&
        (url.match(/\(/g) ?? []).length < (url.match(/\)/g) ?? []).length
      )
        url = url.slice(0, -1);
      else if (
        last === ']' &&
        (url.match(/\[/g) ?? []).length < (url.match(/\]/g) ?? []).length
      )
        url = url.slice(0, -1);
      else break;
    }
    // `<https://…>` percent-encodes its closing bracket in some mailers.
    if (/%3e$/i.test(url)) url = url.slice(0, -3);
    if (url.length > 'https://'.length) out.push(url);
    i = Math.max(end, at + 1);
  }
  return out;
}

/**
 * Decide which service a URL belongs to, or `null` when it is not a join link.
 *
 * The exclusions matter as much as the matches. Webex's dial-in listing lives
 * on the same host and carries the same `MTID` as a real join link, so matching
 * on `MTID` alone offers a Join button that opens a page of phone numbers.
 */
export function classify(url: string): ConferenceProvider | null {
  const lower = url.toLowerCase();
  const host = hostOf(lower);
  if (!host) return null;

  if (host.endsWith('webex.com') || host.endsWith('webex.com.cn')) {
    if (
      lower.includes('globalcallin.php') ||
      lower.includes('/recordingservice/') ||
      lower.includes('/playback/')
    )
      return null;
    const joinable =
      lower.includes('j.php?') ||
      lower.includes('/meet/') ||
      lower.includes('/join/') ||
      lower.includes('/wbxmjs/joinservice/');
    return joinable ? 'webex' : null;
  }
  if (host.endsWith('teams.microsoft.com') || host.endsWith('teams.live.com')) {
    return lower.includes('/l/meetup-join/') ? 'teams' : null;
  }
  if (
    host.endsWith('zoom.us') ||
    host.endsWith('zoom.com') ||
    host.endsWith('zoomgov.com')
  ) {
    return lower.includes('/j/') || lower.includes('/my/') || lower.includes('/w/')
      ? 'zoom'
      : null;
  }
  // DFNconf (the German research network's Pexip service). The join links live
  // on the bare host under /webapp/; the DOCUMENTATION lives on www.conf.dfn.de,
  // so an exact host match is what keeps a link to the manual from offering a
  // Join button. A conference id is required too — `/webapp/` on its own is the
  // app's landing page and joins nothing.
  //
  // Only the Pexip half. DFNconf also still runs Adobe Connect on
  // webconf.vc.dfn.de, but its meeting URLs have no stable path marker, so
  // matching that host would offer Join for its login page as readily as for a
  // meeting — the same trap the Webex exclusions above exist for.
  if (host === 'conf.dfn.de') {
    return lower.includes('conference=') || lower.includes('/conference/')
      ? 'dfnconf'
      : null;
  }
  if (host === 'meet.google.com') return 'googleMeet';
  if (host === 'meet.jit.si' || host.endsWith('.jit.si')) return 'jitsi';
  if (host.endsWith('gotomeeting.com') || host.endsWith('goto.com')) {
    return lower.includes('/join/') ? 'goToMeeting' : null;
  }
  if (host.endsWith('whereby.com')) return 'whereby';
  // BigBlueButton is self-hosted, so the host says nothing — the join path is
  // the only stable marker.
  if (
    lower.includes('/bigbluebutton/api/join') ||
    (lower.includes('/b/') && lower.includes('bbb'))
  )
    return 'bigBlueButton';
  return null;
}

function hostOf(lowerUrl: string): string | null {
  const afterScheme = lowerUrl.split('://')[1];
  if (!afterScheme) return null;
  const hostPart = afterScheme.split(/[/?#]/)[0] ?? '';
  const host = hostPart.split('@').at(-1) ?? hostPart;
  return host.split(':')[0] ?? host;
}

/**
 * Read the `label: value` lines that follow the join link.
 *
 * Deliberately knows no labels. A real German Webex invitation carries no
 * `tel:` and no `sip:`, so its meeting id and password exist ONLY behind prose
 * labels — and its password is alphanumeric, so no digit heuristic finds it
 * either. Rather than learning "Besprechungs-ID", the label is treated as DATA
 * and handed on verbatim: a screen reader then reads it in the language the
 * invitation arrived in, and Aperio never needed to know what the words mean.
 *
 * The bounds are what keep it from harvesting prose: it starts at the join link
 * so the greeting above is out of reach, a label and a value must both be short
 * and on one line, and a value containing a scheme is the link rather than a
 * detail.
 */
export function labelledLinesNear(
  description: string,
  joinUrl: string,
): Array<{ label: string; value: string }> {
  const at = description.indexOf(joinUrl);
  const from = at >= 0 ? at + joinUrl.length : 0;
  const out: Array<{ label: string; value: string }> = [];
  for (const raw of description.slice(from).split('\n')) {
    if (out.length >= MAX_LABELLED) break;
    const line = raw.trim();
    if (!line) continue;
    const idx = line.indexOf(':');
    if (idx < 0) continue;
    const label = line.slice(0, idx).trim();
    const value = line.slice(idx + 1).trim();
    if (
      !label ||
      !value ||
      label.length > MAX_LABEL_CHARS ||
      value.length > MAX_VALUE_CHARS ||
      value.startsWith('//') ||
      value.includes('://')
    )
      continue;
    out.push({ label, value });
  }
  return out;
}

/**
 * Recover the join details WITHOUT reading prose, where the invitation carries
 * them machine-readably.
 *
 * A SIP address `sip:25503113955@example.webex.com` has the meeting number as
 * its user part; the DTMF sequence in `tel:+…,,*01*25503113955%23626114%23*01*`
 * has the number and then the numeric password, each terminated by an encoded
 * `#`. Neither is touched by translation. Many invitations have neither, which
 * is what {@link labelledLinesNear} is for.
 */
function extractDetails(texts: string[]): Pick<
  ConferenceLink,
  'meetingNumber' | 'password' | 'sipAddress' | 'phone'
> {
  const found: {
    meetingNumber?: string;
    password?: string;
    sipAddress?: string;
    phone?: string;
  } = {};
  for (const text of texts) {
    if (!found.sipAddress) {
      const sip = findUri(text, 'sip:');
      if (sip) {
        const user = sip.slice('sip:'.length).split('@')[0] ?? '';
        if (user && /^\d+$/.test(user)) found.meetingNumber = user;
        found.sipAddress = sip;
      }
    }
    if (!found.phone) {
      const tel = findUri(text, 'tel:');
      if (tel) {
        const [number, password] = parseDtmf(tel);
        found.meetingNumber ??= number;
        found.password ??= password;
        found.phone = tel;
      }
    }
  }
  return found;
}

function findUri(text: string, scheme: string): string | undefined {
  const at = text.toLowerCase().indexOf(scheme);
  if (at < 0) return undefined;
  const tail = text.slice(at);
  let end = tail.length;
  for (let i = 0; i < tail.length; i += 1) {
    const ch = tail[i]!;
    if (/\s/.test(ch) || ch === '<' || ch === '>' || ch === '"' || ch === "'") {
      end = i;
      break;
    }
  }
  const uri = tail.slice(0, end).replace(/[.,;:)]+$/, '');
  return uri.length > scheme.length ? uri : undefined;
}

/** Split the `*01*<number>#<password>#*01*` payload of a dial-in link. */
function parseDtmf(tel: string): [string | undefined, string | undefined] {
  const decoded = tel.replace(/%23/g, '#').replace(/%2a/gi, '*');
  const payload = decoded.split(',,')[1];
  if (!payload) return [undefined, undefined];
  const digits = payload
    .split('#')
    .map((part) =>
      // The LONGEST digit run in the part, not the first: the field is wrapped
      // in a `*01*` tone marker, and the first run would stop at "01".
      (part.match(/\d+/g) ?? []).reduce((a, b) => (b.length > a.length ? b : a), ''),
    )
    .filter((d) => d.length >= 4);
  return [digits[0], digits[1]];
}
