// Mobile contacts api-client — the engine-reuse boundary for the contacts
// surface (the Host: local address book + statically-embedded external
// CardDAV/Google/EWS providers). JSON passthrough over the Host's contact
// methods; the wire shapes are the cal_core/desktop serde shape.
//
// Contacts are NOT on the sync event log (local contacts are device-local;
// external ones self-sync via their provider), so — unlike tasks/events — the
// mutations here do NOT kick a background sync push. The composite types are
// defined here for now (like Calendar in ./calendar), reusing leaf types from
// @aperio/shared; they hoist to @aperio/shared in a later consolidation.

import CalFfi from '../../modules/cal-ffi';
import type { ContainerColor, WireContactValue } from '@aperio/shared';

/** An address book enriched with its owning account (the `ContactListRow`
 *  wire shape). `account_id === 'local'` marks the device-local book. */
export interface ContactList {
  id: string;
  name: string;
  color: ContainerColor | null;
  color_label: string | null;
  read_only: boolean;
  account_id: string;
}

/** One postal address on a contact (the cal_core `ContactAddress` shape). Every
 *  field is optional; an all-empty address is dropped on save. `label` is
 *  free-form — the conventional slots are `"home"`/`"work"`/`"other"`. (The
 *  Rust side skip-serializes `None`, so a field can be absent on read; treat
 *  missing as empty.) */
export interface ContactAddress {
  label: string | null;
  street: string | null;
  city: string | null;
  region: string | null;
  postal_code: string | null;
  country: string | null;
}

/** A persisted contact (the cal_core `Contact` wire shape). `members` / `photo`
 *  still cross opaquely (the group + photo editors are deferred). */
export interface Contact {
  id: string;
  list_id: string;
  display_name: string;
  given_name: string | null;
  family_name: string | null;
  /** Honorific name prefix ("Prof. Dr.") and suffix ("jun.") — vCard `N`
   *  components 4/5, Google honorificPrefix/Suffix, Graph title/generation.
   *  EWS surfaces neither (read-only CompleteName), so they stay null there. */
  name_prefix: string | null;
  name_suffix: string | null;
  organization: string | null;
  /** See `WireContactValue`: an object with a label, or a bare string for
   *  anything stored before labels existed. Normalise with `toContactValues`
   *  from `@aperio/shared` before rendering. */
  emails: WireContactValue[];
  phone_numbers: WireContactValue[];
  /** Websites, same labelled shape. */
  urls: WireContactValue[];
  /** ISO `YYYY-MM-DD`, or null. */
  birthday: string | null;
  /** Wedding / partnership anniversary, ISO `YYYY-MM-DD` or null. Microsoft
   *  Graph has no field for it, so it stays null on Outlook accounts. */
  anniversary: string | null;
  job_title: string | null;
  department: string | null;
  notes: string | null;
  /** `null` ⇒ a person; an array (even empty) ⇒ a group / distribution list. */
  members: unknown[] | null;
  has_photo: boolean;
  addresses: ContactAddress[];
  created_at: string;
  updated_at: string;
  etag: string | null;
}

/** A new (unsaved) contact — the cal_core `NewContact` wire shape. */
export interface NewContact {
  display_name: string;
  given_name: string | null;
  family_name: string | null;
  /** Honorific name prefix ("Prof. Dr.") and suffix ("jun.") — vCard `N`
   *  components 4/5, Google honorificPrefix/Suffix, Graph title/generation.
   *  EWS surfaces neither (read-only CompleteName), so they stay null there. */
  name_prefix: string | null;
  name_suffix: string | null;
  organization: string | null;
  /** See `Contact.emails`. Write with `fromContactValues`. */
  emails: WireContactValue[];
  phone_numbers: WireContactValue[];
  urls: WireContactValue[];
  birthday: string | null;
  anniversary: string | null;
  job_title: string | null;
  department: string | null;
  notes: string | null;
  addresses: ContactAddress[];
  members: unknown[] | null;
  /** Optional avatar to attach on create (`{content_type, data:<base64>}`). */
  photo: ContactPhoto | null;
}

// ── Address books ────────────────────────────────────────────────────────────

/** All address books (local + external); also primes the Host's route map, so
 *  call it before contact operations. */
export const listContactLists = async (): Promise<ContactList[]> =>
  JSON.parse(await CalFfi.contactListsJson()) as ContactList[];

export const createContactList = async (name: string): Promise<ContactList> =>
  JSON.parse(await CalFfi.createContactListJson(name)) as ContactList;

export const deleteContactList = (id: string): Promise<void> =>
  CalFfi.deleteContactList(id);

// ── Contacts ─────────────────────────────────────────────────────────────────

export const getContacts = async (listId: string): Promise<Contact[]> =>
  JSON.parse(await CalFfi.contactsJson(listId)) as Contact[];

/** Cross-account contact search (local FTS + each external provider's search,
 *  incl. directories like the GAL). Local hits first. Read-only — no push. */
export const searchContacts = async (query: string): Promise<Contact[]> =>
  JSON.parse(await CalFfi.searchContactsJson(query)) as Contact[];

export const createContact = async (
  listId: string,
  contact: NewContact,
): Promise<Contact> =>
  JSON.parse(await CalFfi.createContactJson(listId, JSON.stringify(contact))) as Contact;

/** Full-overwrite update; the contact's `list_id` selects the route. */
export const updateContact = async (contact: Contact): Promise<Contact> =>
  JSON.parse(await CalFfi.updateContactJson(JSON.stringify(contact))) as Contact;

/** Delete a contact. Pass the owning `listId` so the delete routes to the right
 *  account (external contacts need it; local ones ignore it). */
export const deleteContact = (
  id: string,
  listId: string | null = null,
): Promise<void> => CalFfi.deleteContact(id, listId);

// ── Contact photos ───────────────────────────────────────────────────────────

/** A contact avatar — its MIME type + the raw bytes as base64 (the cal_core
 *  `ContactPhoto` wire shape; `data` is base64-encoded on both directions). */
export interface ContactPhoto {
  content_type: string;
  /** Base64-encoded image bytes. */
  data: string;
}

/** The contact's avatar, or `null` when it has none. Call only when the
 *  contact's `has_photo` is true (a no-photo contact returns `null`, not an
 *  error). Routed by the owning `listId`. */
export const getContactPhoto = async (
  id: string,
  listId: string | null = null,
): Promise<ContactPhoto | null> =>
  JSON.parse(await CalFfi.getContactPhotoJson(id, listId)) as ContactPhoto | null;

/** Set (or replace) a contact's avatar. `data` is base64 image bytes. Routed by
 *  `listId`. Rejects (Unsupported) on a provider that doesn't model photos. */
export const setContactPhoto = (
  id: string,
  photo: ContactPhoto,
  listId: string | null = null,
): Promise<void> =>
  CalFfi.setContactPhotoJson(id, listId, JSON.stringify(photo));

/** Remove a contact's avatar (other fields untouched). Routed by `listId`. */
export const deleteContactPhoto = (
  id: string,
  listId: string | null = null,
): Promise<void> => CalFfi.deleteContactPhoto(id, listId);

// ── Contact sync (§10.5) ──────────────────────────────────────────────────────
//
// The contact-sync core lives in host-core (shared with the desktop). It warms
// every external book's listing + per-list caches; the desktop wraps it in a
// tokio worker loop, mobile drives it from the manual button / foreground (no
// background loop while suspended). A finished pass fires the native
// `onContactsSynced` event; the screen seeds its footer from
// `getContactsSyncStatus` and reconciles on that event. The interval +
// include-read-only prefs are device-local (a per-device cadence, not synced).

/** Contact-sync status (the `ContactsSyncStatus` wire shape) — the Settings
 *  footer + the seed for the interval picker / include-read-only toggle. */
export interface ContactsSyncStatus {
  /** RFC-3339 of the last successful pass, or null when never synced. */
  last_synced_at: string | null;
  interval_minutes: number;
  in_flight: boolean;
  include_read_only_on_sync: boolean;
}

/** Payload of the `onContactsSynced` event (the `ContactsSyncedPayload` shape). */
export interface ContactsSyncedPayload {
  last_synced_at: string;
  succeeded_accounts: string[];
  failed_accounts: string[];
}

/** Run one contact-sync pass now (warms every external book's cache).
 *  `includeReadOnly`: `null` reads the persisted pref (matches the desktop
 *  manual button); `true`/`false` overrides it. Resolves `false` when a pass
 *  was already in flight. */
export const syncContactsNow = (
  includeReadOnly: boolean | null = null,
): Promise<boolean> => CalFfi.syncContactsNow(includeReadOnly);

/** The current contact-sync status. */
export const getContactsSyncStatus = async (): Promise<ContactsSyncStatus> =>
  JSON.parse(await CalFfi.getContactsSyncStatusJson()) as ContactsSyncStatus;

/** Persist the periodic-sync interval (minutes); the Host clamps to [1, 1440]
 *  and returns the clamped value. Device-local. */
export const setContactsSyncInterval = (minutes: number): Promise<number> =>
  CalFfi.setContactsSyncInterval(minutes);

/** Persist the "also pull read-only directories" toggle. Device-local. */
export const setContactsIncludeReadOnlyOnSync = (
  enabled: boolean,
): Promise<void> => CalFfi.setContactsIncludeReadOnlyOnSync(enabled);

/** Drop every external book's contact cache + reset "last synced" to never.
 *  Resolves to the number of accounts the invalidate succeeded against. */
export const clearContactsCache = (): Promise<number> =>
  CalFfi.clearContactsCache();
