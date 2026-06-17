import type { NativeStackScreenProps } from '@react-navigation/native-stack';

/**
 * The root native-stack route table. Must be a `type` alias (React Navigation
 * 7's typing pattern), not an interface.
 *
 *   Tasks       — the grouped task list (initial route).
 *   TaskEditor  — create (`taskId: null`) or edit a task; presented as a modal.
 *   Lists       — task-list catalog + selection + creation.
 *   Accounts    — connected-account list + add (non-OAuth kinds) + delete.
 *   Events      — the selected day's events (across calendars) + day nav.
 *   EventEditor — create (`eventId: null`) or edit an event; presented as a modal.
 */
export type RootStackParamList = {
  Tasks: undefined;
  TaskEditor: { taskId: string | null; listId: string };
  Lists: undefined;
  Accounts: undefined;
  Events: undefined;
  EventEditor: { eventId: string | null; calendarId: string };
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
