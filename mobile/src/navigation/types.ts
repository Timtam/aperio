import type { NativeStackScreenProps } from '@react-navigation/native-stack';

import type { CarryableFields, CarryScope, EventGroup } from '@aperio/shared';
import type { CalendarEvent } from '../api/calendar';
import type { NewGroupMember } from '../api/eventGroups';

/**
 * The root native-stack route table. Must be a `type` alias (React Navigation
 * 7's typing pattern), not an interface.
 *
 *   Tasks       — the grouped task list (initial route).
 *   TaskEditor  — create (`taskId: null`) or edit a task; presented as a modal.
 *   Lists       — task-list catalog + selection + creation.
 *   Accounts    — connected-account list + add (non-OAuth kinds) + delete.
 *   Events      — the selected day's events (across calendars) + day nav.
 *   Week        — the anchor week's events + tasks, grouped by day.
 *   Month       — the anchor month's events + tasks, grouped by day.
 *   Agenda      — a ~30-day forward list of events grouped by day.
 *   EventEditor — create (`eventId: null`) or edit an event; presented as a modal.
 *   Sync        — configure the sync target + run a round + read status.
 *   Settings    — app-config hub: language override + links to Accounts / Sync.
 *   Contacts    — address books + their contacts; add / edit / delete.
 *   ContactEditor — create (`contactId: null`) or edit a contact; modal.
 *   Reminders   — read-only overview of upcoming reminder triggers.
 */
export type RootStackParamList = {
  Tasks: undefined;
  // `parentId` (when creating) nests the new task under that parent — its list
  // is then locked to the parent's. `initialTitle`/`initialScheduledDate` seed a
  // fresh task with the values typed into the quick-add before "More details …".
  TaskEditor: {
    taskId: string | null;
    listId: string;
    parentId?: string | null;
    initialTitle?: string;
    initialScheduledDate?: string;
  };
  // One-tap task capture (title + optional day + list); "More details …" hands
  // off to TaskEditor. The mobile twin of the desktop QuickAddTaskDialog.
  // `initialScheduledDate` (YYYY-MM-DD) anchors it to a tapped calendar day.
  QuickAdd: { initialScheduledDate?: string } | undefined;
  // One-tap EVENT capture (title + date/time + calendar); "More details …" hands
  // off to EventEditor. The mobile twin of the desktop QuickAddDialog. `anchor`
  // (YYYY-MM-DD) seeds the day; `calendarId` pre-selects the calendar.
  QuickAddEvent: { calendarId: string; anchor?: string };
  // Plan-from-backlog quick scheduler (Today / Tomorrow / Next Monday / custom /
  // back-to-backlog). The mobile twin of the desktop PlanTaskDialog.
  PlanTask: { taskId: string; listId: string };
  // Move / copy a task to another list or an event to another calendar (with the
  // occurrence-vs-series scope for a recurring event). The mobile twin of the
  // desktop MoveCopyDialog. A task is re-fetched by id; an event carries its full
  // row (an expanded occurrence is synthetic, so there's no id to re-fetch it by).
  MoveCopy:
    | { kind: 'task'; taskId: string; listId: string }
    | { kind: 'event'; event: CalendarEvent };
  // "These events mean the same appointment" (DESIGN-event-groups.md). The
  // anchor rides in whole: the screen needs its title and start as the
  // signature it stores, and re-fetching would only risk a different answer.
  EventGroup: { event: CalendarEvent };
  // "Carry this change to the other copies?" — asked AFTER an edit to a
  // grouped event was saved (DESIGN-event-groups.md, Stufe 2). Everything it
  // needs rides in: re-reading would risk a different answer than the one the
  // question was asked about.
  EventGroupCarry: {
    group: EventGroup;
    anchor: { calendar_id: string; event_id: string };
    before: CarryableFields;
    after: CarryableFields;
    /** Which occurrences the carry is about — `occurrence` and `future` do to
     *  each copy what the edit did to the anchor (carve one out / cut the
     *  series in two) rather than moving its whole series. */
    scope?: CarryScope;
    occurrence?: string | null;
    /** The row the anchor's edit left behind, when it made one. The copies the
     *  carry creates are tied to it afterwards, so an occurrence or
     *  "and all following" edit does not quietly undo the grouping from that
     *  point on. */
    successor?: NewGroupMember | null;
  };
  Lists: undefined;
  ListEditor: { listId: string };
  // Manage a shared list's membership (§9.7) — only reachable for lists whose
  // adapter declares `manageable`. `listName` seeds the header before the store
  // resolves.
  TaskMembers: { listId: string; listName: string };
  Accounts: undefined;
  // `anchor` (ISO instant) seeds the initial day/window when arriving from the
  // Day⇄Agenda switcher, so switching views keeps the selected date.
  Events: { anchor?: string } | undefined;
  Week: { anchor?: string } | undefined;
  Month: { anchor?: string } | undefined;
  Agenda: { anchor?: string } | undefined;
  // A ~12-month overview; each month opens the Month view. `anchor` (ISO) seeds
  // the displayed year from the Day⇄…⇄Year switcher.
  Year: { anchor?: string } | undefined;
  // `occurrence` (an RFC-3339 instant) marks that a single occurrence of a
  // recurring series was opened — the editor seeds its dates from it and offers
  // the "this occurrence vs whole series" edit scope.
  EventEditor: {
    eventId: string | null;
    calendarId: string;
    occurrence?: string | null;
    // `anchor` (YYYY-MM-DD) seeds a NEW event's date from the tapped calendar
    // day instead of today (the per-day "+ new event" affordance).
    anchor?: string;
    // `initialTitle` seeds a fresh event with the title typed into the
    // event quick-add before "More details …". It doubles as the name shown in
    // the read-only summary a SYNTHETIC birthday event opens instead of the
    // editor — such an id has no fetchable row behind it.
    initialTitle?: string;
    // `initialTime` (HH:MM) seeds a fresh event's start time — carries the
    // quick-add's picked time over the "More details …" hand-off instead of
    // re-deriving the default slot. Ignored when editing.
    initialTime?: string;
    // The scope the up-front edit prompt resolved to (see eventEditScope). Seeds
    // the editor's edit scope; absent ⇒ 'occurrence'. When set, the editor
    // confirms the scope read-only instead of showing the segmented control.
    initialScope?: 'occurrence' | 'series' | 'this_and_future';
  };
  Calendars: undefined;
  CalendarEditor: { calendarId: string };
  Sync: undefined;
  Settings: undefined;
  // General app config (language / first day of week / default reminder sound) —
  // the desktop Settings "General" tab, pushed from the Settings hub.
  General: undefined;
  TaskSettings: undefined;
  ColorLabels: undefined;
  // Diagnostics: the rolling log — detail level, recent lines, redacted export.
  Logs: undefined;
  Contacts: undefined;
  ContactLists: undefined;
  ContactEditor: { contactId: string | null; listId: string };
  // Contacts sync + privacy settings (§10.5/§10.6): manual sync, interval,
  // include-read-only-directories, clear cache, provider privacy policies.
  ContactSettings: undefined;
  Reminders: undefined;
  Search: undefined;
  Conflicts: undefined;
};

/**
 * The bottom-tab shell — the primary navigation (the mobile equivalent of the
 * desktop sidebar). Each tab hosts a native-stack over a subset of
 * `RootStackParamList`:
 *   TasksTab     — Tasks → Lists → TaskEditor (modal).
 *   CalendarTab  — Events → EventEditor (modal).
 *   ContactsTab  — Contacts → ContactEditor (modal).
 *   SettingsTab  — Settings → Accounts / Sync.
 * Cross-navigator `navigation.navigate('SomeScreen')` still resolves via the
 * global augmentation below (React Navigation finds the route in whichever
 * nested stack registers it + switches tabs as needed).
 */
export type RootTabParamList = {
  TasksTab: undefined;
  CalendarTab: undefined;
  ContactsTab: undefined;
  SettingsTab: undefined;
};

/** Per-screen props helper: `RootStackScreenProps<'TaskEditor'>` gives a typed
 *  `route.params` + `navigation`. */
export type RootStackScreenProps<T extends keyof RootStackParamList> =
  NativeStackScreenProps<RootStackParamList, T>;

// Make `useNavigation()` (without an explicit generic) resolve against the root
// param list app-wide — the v7 global augmentation.
declare global {
  // eslint-disable-next-line @typescript-eslint/no-namespace
  namespace ReactNavigation {
    interface RootParamList extends RootStackParamList {}
  }
}
