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
import type { ContainerColor } from '@aperio/shared';

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
  organization: string | null;
  emails: string[];
  phone_numbers: string[];
  /** ISO `YYYY-MM-DD`, or null. */
  birthday: string | null;
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
  organization: string | null;
  emails: string[];
  phone_numbers: string[];
  birthday: string | null;
  notes: string | null;
  addresses: ContactAddress[];
  members: unknown[] | null;
  photo: unknown | null;
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
