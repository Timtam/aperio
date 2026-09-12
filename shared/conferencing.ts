/**
 * Finding the online meeting in an event — installed by the surface, not
 * implemented here.
 *
 * The rule lives in `cal_core::conferencing`. What differs per surface is only
 * how Rust is reached: the desktop through WebAssembly compiled into its
 * webview, mobile through the UniFFI bridge. Each installs its own door at
 * startup and everything in this package then detects the same way.
 *
 * # Why this file is a door and not a rule
 *
 * It was 384 lines of TypeScript — a second implementation of a rule that has
 * been in `cal-core` all along and runs in production there, called by
 * `videoconference.rs` and `cal-ffi/host.rs` to resolve a meeting binding.
 * Nothing pinned the two together: each carried its own hand-copied test list.
 *
 * Reading them side by side turned up SIX places where they disagreed. None was
 * observable, because both Rust callers use only `join_url` and five of the six
 * lived in the other fields — so the disagreements would all have surfaced at
 * once, silently, the day this file was deleted. They are written down instead,
 * in `crates/cal-core/tests/fixtures/conferencing.json`: every input, both
 * answers, and why the surviving one was chosen.
 *
 * Two of them were a defect rather than a difference of opinion. This file
 * computed an index in a lowercased copy of a string and applied it to the
 * original, so a Turkish dotted capital I — the one code point in Unicode whose
 * lowercase form is longer — dropped the leading digit of a meeting number. And
 * its label caps counted UTF-16 code units where the core counts characters, so
 * it was always the more aggressive dropper: the move can only ADD detail rows.
 *
 * # The boundary is JSON
 *
 * A detected meeting is a record with two optional string fields and a list of
 * pairs, which is more than the bare values that crossed before. It crosses as
 * JSON — still a string, so the WebAssembly crate's own rule holds — because
 * `serde_json` is already in both graphs and one shape for both doors is what
 * keeps them from drifting in what they accept.
 */
export type { ConferenceProvider } from './generated/ConferenceProvider';
export type { ConferenceSource } from './generated/ConferenceSource';
export type { ConferenceLink } from './generated/ConferenceLink';
export type { ConferenceDetail } from './generated/ConferenceDetail';
import type { ConferenceSourcesInput } from './generated/ConferenceSourcesInput';

import type { ConferenceLink } from './generated/ConferenceLink';

/**
 * The fields a detector looks at, in the order it prefers them.
 *
 * Every one is optional here even though the Rust twin declares them all: a
 * caller passes what it has, and both call sites in this repository pass a
 * location and a description and nothing else.
 */
export interface ConferenceSources {
  /** A join URL a provider stated outright — Google `conferenceData`, Graph
   *  `onlineMeeting`. Nothing can be more authoritative than that. */
  providerField?: string | null;
  /** RFC 7986 `CONFERENCE` values. */
  icalendarConference?: string[];
  /** Vendor X-property values. */
  vendorProperties?: string[];
  location?: string | null;
  description?: string | null;
}

/**
 * This surface's door into `cal_core::conferencing`.
 *
 * JSON in, JSON out. The caller here builds the request and parses the answer,
 * so a surface only has to hand over the one call it can make synchronously.
 */
export interface ConferenceDetector {
  /** `sourcesJson` in, a `ConferenceLink` as JSON or the string `"null"` out. */
  detectConferenceJson(sourcesJson: string): string;
}

let installedDetector: ConferenceDetector | null = null;

/** Bind this surface's door into the core. */
export function installConferenceDetector(detector: ConferenceDetector): void {
  installedDetector = detector;
}

function detector(): ConferenceDetector {
  if (installedDetector === null) {
    // Loud, not a local fallback. A fallback here would be the second
    // implementation all over again — and the failure it produces is one
    // surface offering a Join button the other does not, which nobody reports
    // because each device looks self-consistent.
    throw new Error(
      'conference detection used before installConferenceDetector() — the ' +
        'surface must install its door into cal-core during startup',
    );
  }
  return installedDetector;
}

/**
 * Find the meeting, or decide there is none.
 *
 * Sources are tried in order of authority. Within one source the longest
 * matching URL wins: Webex's newer join links nest a shorter-looking URL inside
 * their query string, and taking the first match hands back a fragment that
 * joins nothing.
 */
export function detectConference(
  sources: ConferenceSources,
): ConferenceLink | null {
  // Absent fields are sent explicitly rather than omitted. The Rust side
  // defaults them either way, but writing them out means the payload always has
  // the same shape, which is one less thing to wonder about when a detection
  // goes wrong.
  const input: ConferenceSourcesInput = {
    providerField: sources.providerField ?? null,
    icalendarConference: sources.icalendarConference ?? [],
    vendorProperties: sources.vendorProperties ?? [],
    location: sources.location ?? null,
    description: sources.description ?? null,
  };
  const answer = detector().detectConferenceJson(JSON.stringify(input));
  return JSON.parse(answer) as ConferenceLink | null;
}
