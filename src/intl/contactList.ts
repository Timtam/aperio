import type { ContactList } from '../api/types';

/** Sentinel id the EWS adapter emits for its synthetic
 *  "Globale Adressliste" entry (see Rust `GAL_LIST_ID` constant
 *  in `crates/cal-adapter-ews/src/contacts.rs`). Mirroring it
 *  here lets the frontend swap the backend's hardcoded English
 *  label for a localized one without changing the wire shape. */
export const EWS_GAL_LIST_ID = 'ews-gal';

/** Translation-table for system-managed list ids whose names the
 *  backend hardcodes in English. Add new entries here when more
 *  adapters surface synthetic read-only lists. */
const SYSTEM_LIST_I18N_KEY: Record<string, string> = {
  [EWS_GAL_LIST_ID]: 'views.contacts.galListName',
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
