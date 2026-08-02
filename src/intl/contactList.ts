import type { ContactList } from '../api/types';

/** Sentinel id the EWS adapter emits for its synthetic
 *  "Globale Adressliste" entry (see Rust `GAL_LIST_ID` constant
 *  in `crates/adapter-ews/src/contacts.rs`). Mirroring it
 *  here lets the frontend swap the backend's hardcoded English
 *  label for a localized one without changing the wire shape. */
export const EWS_GAL_LIST_ID = 'ews-gal';

/** Sentinel for the writable Google personal address book
 *  (`GOOGLE_CONTACT_LIST_ID` in the Rust adapter). */
export const GOOGLE_CONTACTS_LIST_ID = 'google-contacts';

/** Sentinel for the read-only auto-collected "Other contacts"
 *  list backed by Gmail's history of sent addresses. */
export const GOOGLE_OTHER_CONTACTS_LIST_ID = 'google-other-contacts';

/** Sentinel for the read-only Workspace / G Suite directory —
 *  Google's equivalent of the EWS GAL. Empty for personal
 *  `@gmail.com` accounts. */
export const GOOGLE_DIRECTORY_LIST_ID = 'google-directory';

/** Sentinel for the read-only Microsoft Graph "Suggested People"
 *  list — Outlook's relevance-ranked stream of people the user
 *  interacts with, backed by `/me/people`. Picked over
 *  `Directory.Read.All` because it doesn't require the admin
 *  consent most tenants gate that scope behind. */
export const GRAPH_SUGGESTED_PEOPLE_LIST_ID = 'graph-suggested-people';

/** Translation-table for system-managed list ids whose names the
 *  backend hardcodes in English. Add new entries here when more
 *  adapters surface synthetic read-only lists. */
const SYSTEM_LIST_I18N_KEY: Record<string, string> = {
  [EWS_GAL_LIST_ID]: 'views.contacts.galListName',
  [GOOGLE_CONTACTS_LIST_ID]: 'views.contacts.googleListName',
  [GOOGLE_OTHER_CONTACTS_LIST_ID]: 'views.contacts.googleOtherListName',
  [GOOGLE_DIRECTORY_LIST_ID]: 'views.contacts.googleDirectoryListName',
  [GRAPH_SUGGESTED_PEOPLE_LIST_ID]: 'views.contacts.graphSuggestedPeopleListName',
};

/** Resolve the display name for a `ContactList`. Synthetic
 *  read-only lists (e.g. the EWS GAL) come from the backend with
 *  a fixed English label; consumers route those through this
 *  helper so the rendered text follows the active UI language.
 *  Real user lists pass through unchanged. */
export function getContactListDisplayName(
  list: Pick<ContactList, 'id' | 'name'>,
  t: (key: string) => string,
): string {
  const key = SYSTEM_LIST_I18N_KEY[list.id];
  return key ? t(key) : list.name;
}
