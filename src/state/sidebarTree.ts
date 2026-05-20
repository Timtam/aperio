import type { Account, Calendar, TaskList } from '../api/types';

/**
 * Sidebar tree model.
 *
 * The runtime structure the SidebarTree component renders. Three
 * levels:
 *
 *   1. AccountNode      (top-level)
 *   2. SectionNode      (Calendars / Tasks under an account)
 *   3. LeafNode         (individual calendar or task list)
 *
 * The model is regenerated on every render of the sidebar from the
 * raw account / calendar / task-list lists in the calendar store —
 * cheap (O(n) over a few dozen items at worst), no memoisation
 * needed unless the lists get pathological.
 *
 * Decisions:
 *
 *   - The local account is hard-coded as `LOCAL_ID = "local"` to
 *     match the backend constant. Sort puts it first; everything
 *     else is alphabetical by display name.
 *
 *   - A section is only emitted when there's at least one child of
 *     that kind. An EWS account with calendars but no tasks shows
 *     only the Calendars section.
 *
 *   - An account with zero calendars *and* zero task lists shows
 *     up as an empty account row. This is the "I just added it
 *     and listing is still loading / it's broken" case — the row
 *     is still tabbable but flagged so the UI can render a hint.
 */

export const LOCAL_ACCOUNT_ID = 'local';

export type SectionKind = 'calendars' | 'tasks';

export interface LeafNode {
  key: string;
  kind: SectionKind;
  /** The original container id (calendar or task-list id). Used by
   *  rename / toggle / delete actions. */
  containerId: string;
  name: string;
  /** Hex color, when the container declares one. */
  colorHex: string | null;
  /** True for read-only sources (iCal feed etc). The UI hides the
   *  delete affordance and disables rename push. */
  readOnly: boolean;
  /** True when the container is currently in the visible set. */
  selected: boolean;
}

export interface SectionNode {
  key: string;
  kind: SectionKind;
  /** Display label — "Kalender" / "Aufgaben". Resolved by the
   *  rendering component from i18n keys, not stored here. */
  labelKey: 'calendars' | 'tasks';
  children: LeafNode[];
}

export interface AccountNode {
  key: string;
  accountId: string;
  /** Human-readable name from the `accounts` table. For the local
   *  account this is the seeded "Local" / "Lokal" string. */
  displayName: string;
  /** "local" / "caldav" / "google" / ... — used by the UI to show
   *  a small badge or icon hint. */
  adapterKind: string;
  /** When the account has no children yet (just registered, lists
   *  still loading), `children` is empty and `isEmpty` is true. */
  isEmpty: boolean;
  children: SectionNode[];
}

/**
 * Build the tree from the calendar-store snapshot. Pure function —
 * deterministic order, no IO.
 */
export function buildSidebarTree(input: {
  accounts: Account[];
  calendars: Calendar[];
  taskLists: TaskList[];
  selectedCalendarIds: Set<string>;
  selectedTaskListIds: Set<string>;
}): AccountNode[] {
  const { accounts, calendars, taskLists, selectedCalendarIds, selectedTaskListIds } = input;

  // Group calendars / task lists by account id.
  const calsByAccount = new Map<string, Calendar[]>();
  for (const c of calendars) {
    const arr = calsByAccount.get(c.account_id) ?? [];
    arr.push(c);
    calsByAccount.set(c.account_id, arr);
  }
  const tlsByAccount = new Map<string, TaskList[]>();
  for (const l of taskLists) {
    const arr = tlsByAccount.get(l.account_id) ?? [];
    arr.push(l);
    tlsByAccount.set(l.account_id, arr);
  }

  // Synthesise a local account entry if the backend didn't return
  // one — old DBs without a seeded local row would otherwise lose
  // their calendars. (The migration seeds it, but defence-in-depth
  // is cheap here.)
  const haveLocal = accounts.some((a) => a.id === LOCAL_ACCOUNT_ID);
  const allAccounts: Account[] = haveLocal
    ? accounts
    : [
        ...accounts,
        {
          id: LOCAL_ACCOUNT_ID,
          adapter_kind: 'local',
          display_name: 'Local',
          config_json: '{}',
          created_at: '',
          updated_at: '',
        },
      ];

  // Stable order: local first, then alphabetical by display name.
  const sortedAccounts = [...allAccounts].sort((a, b) => {
    if (a.id === LOCAL_ACCOUNT_ID && b.id !== LOCAL_ACCOUNT_ID) return -1;
    if (b.id === LOCAL_ACCOUNT_ID && a.id !== LOCAL_ACCOUNT_ID) return 1;
    return a.display_name.localeCompare(b.display_name, undefined, {
      sensitivity: 'base',
    });
  });

  return sortedAccounts.map((acc): AccountNode => {
    const accountKey = `account:${acc.id}`;
    const cals = (calsByAccount.get(acc.id) ?? [])
      .slice()
      .sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
      );
    const tls = (tlsByAccount.get(acc.id) ?? [])
      .slice()
      .sort((a, b) =>
        a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
      );

    const sections: SectionNode[] = [];
    if (cals.length > 0) {
      sections.push({
        key: `${accountKey}#calendars`,
        kind: 'calendars',
        labelKey: 'calendars',
        children: cals.map((c) => ({
          key: `${accountKey}#calendars#${c.id}`,
          kind: 'calendars',
          containerId: c.id,
          name: c.name,
          colorHex: c.color?.hex ?? null,
          readOnly: c.read_only,
          selected: selectedCalendarIds.has(c.id),
        })),
      });
    }
    if (tls.length > 0) {
      sections.push({
        key: `${accountKey}#tasks`,
        kind: 'tasks',
        labelKey: 'tasks',
        children: tls.map((l) => ({
          key: `${accountKey}#tasks#${l.id}`,
          kind: 'tasks',
          containerId: l.id,
          name: l.name,
          colorHex: l.color?.hex ?? null,
          readOnly: l.read_only,
          selected: selectedTaskListIds.has(l.id),
        })),
      });
    }

    return {
      key: accountKey,
      accountId: acc.id,
      displayName: acc.display_name,
      adapterKind: acc.adapter_kind,
      isEmpty: sections.length === 0,
      children: sections,
    };
  });
}

/**
 * Tri-state for a parent: based on its descendant leaves.
 *
 *   - `'unchecked'` ⇒ none of the leaves are selected
 *   - `'mixed'`     ⇒ some leaves are selected, others not
 *   - `'checked'`   ⇒ every leaf is selected
 *
 * Used for both `aria-checked` on parent treeitems and the visual
 * checkbox state. The space-toggle on a parent flips every leaf to
 * the inverse of the current parent state (checked / mixed → all
 * off; unchecked → all on).
 */
export type TriState = 'checked' | 'mixed' | 'unchecked';

export function parentTriState(leaves: LeafNode[]): TriState {
  if (leaves.length === 0) return 'unchecked';
  const selected = leaves.filter((l) => l.selected).length;
  if (selected === 0) return 'unchecked';
  if (selected === leaves.length) return 'checked';
  return 'mixed';
}

export function accountTriState(account: AccountNode): TriState {
  const leaves = account.children.flatMap((s) => s.children);
  return parentTriState(leaves);
}
