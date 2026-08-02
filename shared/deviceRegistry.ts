/**
 * How a device-registry row is read out — the part that must not differ
 * between the two frontends.
 *
 * The registry (`meta.json.devices`) is the dataset's answer to "who is
 * participating". After a few reinstalls and a run of test devices it stops
 * being true, and the panels on both platforms exist to let a user prune it.
 * What they need from a row is one judgement — is this still a device — and
 * this module makes it, once.
 *
 * Only the CLASSIFICATION lives here. The sentences are locale keys, owned by
 * each frontend, because the two render them into different widgets.
 *
 * ## Why `last_seen` and not `last_seen_log`
 *
 * The registry has carried a `last_seen_log` for as long as it has existed and
 * it cannot answer this question. It is a CONTENT horizon — the newest log the
 * device holds — so on a dataset where nothing is happening it does not move,
 * however often the device syncs. A laptop syncing every quarter of an hour and
 * one last opened in March publish the same unchanging value. `last_seen` is
 * the wall clock, added for exactly this, and absent on datasets written before
 * it existed.
 */

/** The registry row, as both hosts serialise it. */
export interface DeviceRegistryRow {
  id: string;
  name: string | null;
  /** Wall clock of the device's last completed round, RFC 3339. `null` on a
   *  dataset whose registry predates the field. */
  last_seen: string | null;
  /** The device's content horizon, RFC 3339. Not a heartbeat — see above. */
  last_seen_log: string;
  app_version: string;
  stale: boolean;
  is_this_device: boolean;
}

/** How recently a device reported in, in the terms the panels speak. */
export type DeviceActivity =
  /** This device. It is here by definition, and saying "3 minutes ago" about
   *  the machine the user is typing on reads as noise. */
  | { kind: 'self' }
  /** A wall-clock stamp the caller renders as a relative time. */
  | { kind: 'seen'; at: Date }
  /** The registry has no stamp for this device: either it predates the field,
   *  or the device has not completed a round since it was added.
   *
   *  Rendered as "unknown", NEVER as a date. A missing stamp used to be a
   *  tempting place to substitute the content horizon, and that substitution
   *  is exactly the confusion the field was added to end. */
  | { kind: 'unknown' };

export function deviceActivity(row: DeviceRegistryRow): DeviceActivity {
  if (row.is_this_device) return { kind: 'self' };
  if (!row.last_seen) return { kind: 'unknown' };
  const at = new Date(row.last_seen);
  // A stamp that will not parse is no stamp. Rendering `Invalid Date` into a
  // relative formatter produces "NaN years ago" on one platform and throws on
  // the other, and neither is a thing to put in front of somebody deciding
  // whether to delete a row.
  return Number.isNaN(at.getTime()) ? { kind: 'unknown' } : { kind: 'seen', at };
}

/** What to call a device that has no name.
 *
 *  The id is a 32-character hex string. Read out in full it is unusable —
 *  thirty-two letters spoken one at a time, and the user still cannot tell two
 *  of them apart because they differ somewhere in the middle. The first eight
 *  characters are enough to distinguish the handful of devices any dataset has,
 *  and short enough to hear.
 *
 *  This is a display fallback, not an identity: everything that acts on a
 *  device uses the full `id`.
 */
export function shortDeviceId(id: string): string {
  return id.slice(0, 8);
}

/**
 * Whether a row can be removed from the registry.
 *
 * Only one cannot: this device's own. Its next heartbeat would write the record
 * straight back, so the gesture would read as a broken button rather than as a
 * decision the app declined to make — and a user reaching for it means
 * something else, which is Disconnect.
 */
export function canForgetDevice(row: DeviceRegistryRow): boolean {
  return !row.is_this_device;
}
