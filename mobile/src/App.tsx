import { createNativeBottomTabNavigator } from '@bottom-tabs/react-navigation';
import AsyncStorage from '@react-native-async-storage/async-storage';
import {
  NavigationContainer,
  type NavigationState,
  type PartialState,
} from '@react-navigation/native';
import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { StatusBar } from 'expo-status-bar';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { SafeAreaProvider } from 'react-native-safe-area-context';

// Activate the app-wide VoiceOver gesture host (window-level magic tap +
// three-finger swipe routing) from app start, independent of which screen loads
// first.
import './a11y/gestureHost';
import { useSyncTriggers } from './api/syncTriggers';
import DayStartReviewModal from './components/DayStartReviewModal';
import { EventScopeDialogHost } from './components/EventScopeDialogHost';
import { FirstLaunchWizardGate } from './components/FirstLaunchWizardGate';
import { SyncStatusButton } from './components/SyncStatusButton';
import { useCacheUpdates } from './state/cacheObserver';
import { armStartupGate } from './state/startupGate';
import { SyncStatusContext } from './state/syncStatusContext';
import { ThemeProvider, useTheme, navigationThemeFor } from './theme';
import { loadThemeModePref } from './theme/themeMode';
import { readStartOnToday } from './settings/startOnToday';
import { navigationRef } from './navigation/navigationRef';
import type { RootStackParamList, RootTabParamList } from './navigation/types';
import { useReminderTriggers } from './reminders/scheduler';
import { useAppBadge } from './state/useAppBadge';
import { useDayStartChecks } from './state/useDayStartChecks';
import { useStoredLanguage } from './settings/language';
import { useSyncStatus } from './state/useSyncStatus';
import AccountsScreen from './screens/AccountsScreen';
import ContactEditorModal from './screens/ContactEditorModal';
import ColorLabelsScreen from './screens/ColorLabelsScreen';
import ConflictsScreen from './screens/ConflictsScreen';
import ContactListsScreen from './screens/ContactListsScreen';
import ContactsScreen from './screens/ContactsScreen';
import ContactsSettingsScreen from './screens/ContactsSettingsScreen';
import AgendaScreen from './screens/AgendaScreen';
import CalendarEditorModal from './screens/CalendarEditorModal';
import CalendarsScreen from './screens/CalendarsScreen';
import EventEditorModal from './screens/EventEditorModal';
import EventsScreen from './screens/EventsScreen';
import GeneralSettingsScreen from './screens/GeneralSettingsScreen';
import ListEditorModal from './screens/ListEditorModal';
import ListsScreen from './screens/ListsScreen';
import LogsScreen from './screens/LogsScreen';
import MonthScreen from './screens/MonthScreen';
import MoveCopyModal from './screens/MoveCopyModal';
import PlanTaskModal from './screens/PlanTaskModal';
import QuickAddEventModal from './screens/QuickAddEventModal';
import QuickAddTaskModal from './screens/QuickAddTaskModal';
import RemindersScreen from './screens/RemindersScreen';
import SearchScreen from './screens/SearchScreen';
import SettingsScreen from './screens/SettingsScreen';
import SyncScreen from './screens/SyncScreen';
import TaskEditorModal from './screens/TaskEditorModal';
import TaskMembersScreen from './screens/TaskMembersScreen';
import TaskSettingsScreen from './screens/TaskSettingsScreen';
import TasksScreen from './screens/TasksScreen';
import WeekScreen from './screens/WeekScreen';
import YearScreen from './screens/YearScreen';
import { TaskStoreProvider } from './state/taskStore';

// Aperio mobile — navigation host (the faithful desktop port).
//
// A bottom-tab shell is the primary nav — the mobile equivalent of the desktop
// sidebar, and the predictable, screen-reader-friendly home for the primary
// views: Tasks, Calendar, Contacts, Settings. Each tab is a native-stack so
// drill-downs (Lists) and modal editors (TaskEditor / EventEditor /
// ContactEditor) push within their tab. Tab headers + titles come from i18next
// (@aperio/locales); every domain type from @aperio/shared; persistence through
// the shared Rust core via the cal-ffi bridge.

const Tab = createNativeBottomTabNavigator<RootTabParamList>();
const TasksStack = createNativeStackNavigator<RootStackParamList>();
const CalendarStack = createNativeStackNavigator<RootStackParamList>();
const ContactsStack = createNativeStackNavigator<RootStackParamList>();
const SettingsStack = createNativeStackNavigator<RootStackParamList>();

// Persisted last-open navigation state (the active tab + the view/route within
// it), so reopening the app restores where you left off — like the desktop.
// Survives launches via AsyncStorage; a corrupt value just falls back to the
// default tab.
const NAV_STATE_KEY = 'aperio.nav.state';

// Screens that keep the native bottom tab bar: the four tab roots PLUS the
// alternate calendar views (Week/Month/Agenda/Year are siblings of the Day view
// reached via `navigation.replace`, not drill-downs, so the bar stays). On a
// real drill-down (Sync, Accounts, the editors, Lists, …) the bar is hidden so
// it doesn't sit in the screen's VoiceOver swipe order — a sub-screen should
// never let the user swipe left into the tab bar (reported on iOS).
const TAB_ROOT_ROUTES = new Set([
  'Tasks',
  'Events',
  'Contacts',
  'Settings',
  'Week',
  'Month',
  'Agenda',
  'Year',
]);

// The header sync indicator, shared by every main screen. The native tab bar
// announces each tab's own title fine, but it has no slot for an EXTRA custom
// accessible control like this status pill — so it lives in the header instead.
const syncHeaderRight = () => <SyncStatusButton />;

// Native header page titles ("Monatsansicht", …) render very large by
// default; a smaller title keeps the chrome proportionate to the content.
const stackScreenOptions = {
  headerTitleStyle: { fontSize: 15, fontWeight: '600' as const },
};

/** Name of the deepest focused route across the nested navigators. */
function deepestRouteName(
  state: NavigationState | PartialState<NavigationState> | undefined,
): string | undefined {
  if (state == null || state.routes.length === 0) return undefined;
  const index = state.index ?? state.routes.length - 1;
  const route = state.routes[index];
  return route.state ? deepestRouteName(route.state) : route.name;
}

/** Rewrite every restored route's `anchor` param to today's ISO (recursing
 *  into nested navigators), keeping route names + all other params intact.
 *  Backs the "start on today" pref: the app reopens on the same view/tab but
 *  centred on today. Screens seed their day from `route.params.anchor`, so
 *  overwriting it is all that's needed. */
function seedAnchorsToToday(
  state: PartialState<NavigationState>,
): PartialState<NavigationState> {
  const todayIso = new Date().toISOString();
  return {
    ...state,
    routes: state.routes.map((route) => {
      const params = route.params as Record<string, unknown> | undefined;
      return {
        ...route,
        ...(params && 'anchor' in params
          ? { params: { ...params, anchor: todayIso } }
          : {}),
        ...(route.state ? { state: seedAnchorsToToday(route.state) } : {}),
      };
    }),
  };
}

function TasksStackNav() {
  const { t } = useTranslation();
  return (
    <TasksStack.Navigator initialRouteName="Tasks" screenOptions={stackScreenOptions}>
      <TasksStack.Screen
        name="Tasks"
        component={TasksScreen}
        options={{ title: t('views.tasks.title'), headerRight: syncHeaderRight }}
      />
      <TasksStack.Screen
        name="Lists"
        component={ListsScreen}
        options={{ title: t('mobile.listsButtonLabel') }}
      />
      <TasksStack.Screen
        name="ListEditor"
        component={ListEditorModal}
        options={{ presentation: 'modal', title: t('mobile.manageList') }}
      />
      {/* Members manager — pushed from the list editor; its own header title is
          set per-list in the screen. */}
      <TasksStack.Screen
        name="TaskMembers"
        component={TaskMembersScreen}
        options={{ title: t('mobile.manageList') }}
      />
      <TasksStack.Screen
        name="QuickAdd"
        component={QuickAddTaskModal}
        options={{ presentation: 'modal', title: t('dialogs.quickAddTask.title') }}
      />
      <TasksStack.Screen
        name="TaskEditor"
        component={TaskEditorModal}
        options={{ presentation: 'modal', title: t('mobile.newTaskLabel') }}
      />
      <TasksStack.Screen
        name="PlanTask"
        component={PlanTaskModal}
        options={{ presentation: 'modal', title: t('mobile.plan') }}
      />
      <TasksStack.Screen
        name="MoveCopy"
        component={MoveCopyModal}
        options={{ presentation: 'modal', title: t('mobile.moveCopy') }}
      />
      <TasksStack.Screen
        name="Search"
        component={SearchScreen}
        options={{ presentation: 'modal', title: t('dialogs.search.title') }}
      />
      {/* Search lists BOTH kinds, so an event hit must open its editor on THIS
          stack (else navigate() bubbles to the Calendar tab + strands Search). */}
      <TasksStack.Screen
        name="EventEditor"
        component={EventEditorModal}
        options={{ presentation: 'modal', title: t('dialogs.event.newTitle') }}
      />
    </TasksStack.Navigator>
  );
}

function CalendarStackNav() {
  const { t } = useTranslation();
  return (
    <CalendarStack.Navigator initialRouteName="Events" screenOptions={stackScreenOptions}>
      <CalendarStack.Screen
        name="Events"
        component={EventsScreen}
        options={{ title: t('mobile.eventsTitle'), headerRight: syncHeaderRight }}
      />
      <CalendarStack.Screen
        name="Week"
        component={WeekScreen}
        options={{ title: t('views.week.title'), headerRight: syncHeaderRight }}
      />
      <CalendarStack.Screen
        name="Month"
        component={MonthScreen}
        options={{ title: t('views.month.title'), headerRight: syncHeaderRight }}
      />
      <CalendarStack.Screen
        name="Agenda"
        component={AgendaScreen}
        options={{ title: t('views.agenda.title'), headerRight: syncHeaderRight }}
      />
      <CalendarStack.Screen
        name="Year"
        component={YearScreen}
        options={{ title: t('views.year.title'), headerRight: syncHeaderRight }}
      />
      <CalendarStack.Screen
        name="Calendars"
        component={CalendarsScreen}
        options={{ title: t('sidebar.calendars') }}
      />
      <CalendarStack.Screen
        name="CalendarEditor"
        component={CalendarEditorModal}
        options={{ presentation: 'modal', title: t('sidebar.calendars') }}
      />
      <CalendarStack.Screen
        name="EventEditor"
        component={EventEditorModal}
        options={{ presentation: 'modal', title: t('dialogs.event.newTitle') }}
      />
      {/* Quick-adds reachable from the calendar create affordances (header
          "New Event", per-day "+ new event / + new task", day activation) so
          the modal stays in the calendar tab instead of bubbling to Tasks. */}
      <CalendarStack.Screen
        name="QuickAddEvent"
        component={QuickAddEventModal}
        options={{ presentation: 'modal', title: t('dialogs.quickAdd.title') }}
      />
      <CalendarStack.Screen
        name="QuickAdd"
        component={QuickAddTaskModal}
        options={{ presentation: 'modal', title: t('dialogs.quickAddTask.title') }}
      />
      <CalendarStack.Screen
        name="MoveCopy"
        component={MoveCopyModal}
        options={{ presentation: 'modal', title: t('mobile.moveCopy') }}
      />
      <CalendarStack.Screen
        name="Search"
        component={SearchScreen}
        options={{ presentation: 'modal', title: t('dialogs.search.title') }}
      />
      {/* Search lists BOTH kinds, so a task hit must open its editor on THIS
          stack (else navigate() bubbles to the Tasks tab + strands Search). */}
      <CalendarStack.Screen
        name="TaskEditor"
        component={TaskEditorModal}
        options={{ presentation: 'modal', title: t('mobile.newTaskLabel') }}
      />
    </CalendarStack.Navigator>
  );
}

function ContactsStackNav() {
  const { t } = useTranslation();
  return (
    <ContactsStack.Navigator initialRouteName="Contacts" screenOptions={stackScreenOptions}>
      <ContactsStack.Screen
        name="Contacts"
        component={ContactsScreen}
        options={{ title: t('sidebar.contactLists'), headerRight: syncHeaderRight }}
      />
      <ContactsStack.Screen
        name="ContactLists"
        component={ContactListsScreen}
        options={{ title: t('mobile.manageContactLists') }}
      />
      <ContactsStack.Screen
        name="ContactEditor"
        component={ContactEditorModal}
        options={{ presentation: 'modal', title: t('dialogs.contact.createTitle') }}
      />
    </ContactsStack.Navigator>
  );
}

function SettingsStackNav() {
  const { t } = useTranslation();
  return (
    <SettingsStack.Navigator initialRouteName="Settings" screenOptions={stackScreenOptions}>
      <SettingsStack.Screen
        name="Settings"
        component={SettingsScreen}
        options={{ title: t('dialogs.settings.title') }}
      />
      <SettingsStack.Screen
        name="General"
        component={GeneralSettingsScreen}
        options={{ title: t('dialogs.settings.tabs.general') }}
      />
      <SettingsStack.Screen
        name="Accounts"
        component={AccountsScreen}
        options={{ title: t('dialogs.accounts.title') }}
      />
      <SettingsStack.Screen
        name="Sync"
        component={SyncScreen}
        options={{ title: t('mobile.syncTitle') }}
      />
      <SettingsStack.Screen
        name="Conflicts"
        component={ConflictsScreen}
        options={{ title: t('dialogs.syncConflicts.title') }}
      />
      <SettingsStack.Screen
        name="Reminders"
        component={RemindersScreen}
        options={{ title: t('dialogs.reminders.title') }}
      />
      {/* The reminders overview drills into the underlying item, so its editors
          must live on THIS stack (else navigate() bubbles to another tab). */}
      <SettingsStack.Screen
        name="TaskEditor"
        component={TaskEditorModal}
        options={{ presentation: 'modal', title: t('mobile.newTaskLabel') }}
      />
      <SettingsStack.Screen
        name="EventEditor"
        component={EventEditorModal}
        options={{ presentation: 'modal', title: t('dialogs.event.newTitle') }}
      />
      <SettingsStack.Screen
        name="ColorLabels"
        component={ColorLabelsScreen}
        options={{ title: t('dialogs.colorLabels.title') }}
      />
      <SettingsStack.Screen
        name="Logs"
        component={LogsScreen}
        options={{ title: t('dialogs.settings.tabs.logs') }}
      />
      <SettingsStack.Screen
        name="TaskSettings"
        component={TaskSettingsScreen}
        options={{ title: t('dialogs.settings.tabs.tasks') }}
      />
      {/* Contacts settings live under Settings (not the Contacts tab) — the
          desktop groups them under settings, so this is their only home. */}
      <SettingsStack.Screen
        name="ContactSettings"
        component={ContactsSettingsScreen}
        options={{ title: t('dialogs.settings.tabs.contacts') }}
      />
      {/* The Settings hub cross-links to the calendar/list catalogs (the
          per-container settings live in their editors), so the catalogs AND
          their whole drill chain (editor, members) must live on THIS stack —
          navigate() to a route only another tab's stack knows is silently
          dropped (same rule as the reminders editors above). */}
      <SettingsStack.Screen
        name="Calendars"
        component={CalendarsScreen}
        options={{ title: t('sidebar.calendars') }}
      />
      <SettingsStack.Screen
        name="CalendarEditor"
        component={CalendarEditorModal}
        options={{ presentation: 'modal', title: t('sidebar.calendars') }}
      />
      <SettingsStack.Screen
        name="Lists"
        component={ListsScreen}
        options={{ title: t('mobile.listsButtonLabel') }}
      />
      <SettingsStack.Screen
        name="ListEditor"
        component={ListEditorModal}
        options={{ presentation: 'modal', title: t('mobile.manageList') }}
      />
      <SettingsStack.Screen
        name="TaskMembers"
        component={TaskMembersScreen}
        options={{ title: t('mobile.manageList') }}
      />
    </SettingsStack.Navigator>
  );
}

export default function App() {
  // ThemeProvider sits above everything that needs the OS-derived theme (the
  // nav chrome, the status bar, every screen via useThemedStyles).
  return (
    <SafeAreaProvider>
      <ThemeProvider>
        <AppContent />
      </ThemeProvider>
    </SafeAreaProvider>
  );
}

/** Runs the day-start checks (it needs the task store, so it lives inside
 *  TaskStoreProvider) and renders the review modal when the gate opens it. The
 *  modal overlays whatever tab is focused — it's app-global, not per-stack. */
function DayStartChecks() {
  const { reviewOpen, closeReview } = useDayStartChecks();
  return <DayStartReviewModal visible={reviewOpen} onClose={closeReview} />;
}

/** Keeps the app-icon badge (today's open tasks + upcoming events) in sync. It
 *  reads the task store's dataVersion, so it mounts inside TaskStoreProvider. */
function AppBadge() {
  useAppBadge();
  return null;
}

function AppContent() {
  const { t } = useTranslation();
  const theme = useTheme();
  // Apply the stored language override (if any) over the device-locale default.
  useStoredLanguage();
  // JS-driven sync: full round on launch + every foreground-resume, a push on
  // background, and a debounced push after each mutation (wired in the api
  // clients). The mobile stand-in for the desktop SyncScheduler.
  useSyncTriggers();
  // Schedule reminders as ahead-of-time OS notifications (reschedule on launch +
  // foreground + after mutations). The mobile stand-in for the desktop reminder
  // worker; triggers come from the shared core via cal-ffi.
  useReminderTriggers();
  // External-cache live-update: a background refresh / warm pass pushes
  // `cache_updated`; this announces it politely + live-reloads the focused view
  // (the user-chosen behaviour). The bus the screens subscribe to via
  // useCacheReload is driven here.
  useCacheUpdates();
  // App-wide sync status: a Settings-tab badge for sighted users + spoken
  // announcements of attention-class transitions (conflict / failure /
  // schema-too-old) for screen-reader users. The data is already bridged.
  const sync = useSyncStatus();
  // Hide the native tab bar on every drill-down screen (it belongs only on the
  // four tab roots), so VoiceOver can't reach it from a sub-screen's content.
  const [tabBarHidden, setTabBarHidden] = useState(false);
  // Restore the last-open view on launch + persist it on every navigation, so
  // reopening lands where you left off (desktop parity). A corrupt/unreadable
  // saved value falls back to the default initial route.
  const [navReady, setNavReady] = useState(false);
  const [initialNavState, setInitialNavState] = useState<
    PartialState<NavigationState> | undefined
  >(undefined);
  // Once the navigation tree is about to paint, arm the startup gate that
  // holds back the app-global scans (badge, day-start, reminder replan)
  // until the first paint has settled — they otherwise queue their fan-outs
  // ahead of the visible screen's first read on the serial native queue.
  useEffect(() => {
    if (!navReady) return;
    return armStartupGate();
  }, [navReady]);
  useEffect(() => {
    let cancelled = false;
    // Load the device-local theme choice alongside the nav state: both gate
    // `navReady`, so a pinned theme is resolved BEFORE the first visible
    // frame (the splash screen covers the wait) instead of racing it and
    // flashing the system palette on cold start. The synced start-on-today
    // pref joins them so, when on, the restored views open on today.
    const navRestore = Promise.all([
      AsyncStorage.getItem(NAV_STATE_KEY),
      readStartOnToday(),
    ])
      .then(([saved, startToday]) => {
        if (cancelled || saved == null) return;
        try {
          let parsed = JSON.parse(saved) as PartialState<NavigationState>;
          // Start-on-today: keep WHICH view/tab was open, but reset its date to
          // today by rewriting every restored route's `anchor` param (screens
          // seed their day from it). Off (default) restores the last day as-is.
          if (startToday) parsed = seedAnchorsToToday(parsed);
          setInitialNavState(parsed);
          // onStateChange does NOT fire for a restored initialState, so compute
          // the tab-bar visibility from it here — otherwise relaunching the app
          // on a drill-down screen shows the bar (in VoiceOver's swipe order)
          // until the next navigation.
          setTabBarHidden(!TAB_ROOT_ROUTES.has(deepestRouteName(parsed) ?? ''));
        } catch {
          // Corrupt value — ignore and use the default initial route.
        }
      })
      .catch(() => {});
    Promise.allSettled([navRestore, loadThemeModePref()]).then(() => {
      if (!cancelled) setNavReady(true);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  // Memoize the navigator so an unrelated app-shell re-render (e.g. the 30s
  // sync-status poll) doesn't reconcile the native nav/tab views — that
  // reconciliation resets the VoiceOver cursor mid-navigation. It rebuilds ONLY
  // when something the tabs actually render changes (language, the sync badge,
  // tab-bar visibility).
  const tabBadge = sync.badge != null ? String(sync.badge) : undefined;
  const tabs = useMemo(
    () => (
      <Tab.Navigator initialRouteName="TasksTab" tabBarHidden={tabBarHidden}>
        <Tab.Screen
          name="TasksTab"
          component={TasksStackNav}
          options={{
            title: t('views.tasks.title'),
            tabBarIcon: () => ({ sfSymbol: 'checklist' }),
          }}
        />
        <Tab.Screen
          name="CalendarTab"
          component={CalendarStackNav}
          options={{
            title: t('mobile.eventsButtonLabel'),
            tabBarIcon: () => ({ sfSymbol: 'calendar' }),
          }}
        />
        <Tab.Screen
          name="ContactsTab"
          component={ContactsStackNav}
          options={{
            title: t('sidebar.contactLists'),
            tabBarIcon: () => ({ sfSymbol: 'person.2' }),
          }}
        />
        <Tab.Screen
          name="SettingsTab"
          component={SettingsStackNav}
          options={{
            title: t('dialogs.settings.title'),
            tabBarIcon: () => ({ sfSymbol: 'gearshape' }),
            // Flag sync issues on the Settings tab (Sync lives under it) with a
            // badge. The native tab bar has no per-tab slot for an EXTRA custom
            // accessibility announcement (the tab's own title is announced, but
            // there's no hook to fold the sync state into it), so the spoken
            // sync state comes from useSyncStatus's live-region announcements of
            // attention-class transitions plus the header indicator.
            tabBarBadge: tabBadge,
          }}
        />
      </Tab.Navigator>
    ),
    [t, tabBarHidden, tabBadge],
  );
  // Hold the first render until the saved nav state is loaded, so we don't flash
  // the default tab before restoring (the splash screen covers this brief gap).
  if (!navReady) return null;
  return (
    <>
      {/* Dark status-bar glyphs on the light background, light glyphs on the
          dark / high-contrast backgrounds. */}
      <StatusBar style={theme.mode === 'light' ? 'dark' : 'light'} />
      <NavigationContainer
        ref={navigationRef}
        theme={navigationThemeFor(theme)}
        initialState={initialNavState}
        onStateChange={(state) => {
          setTabBarHidden(!TAB_ROOT_ROUTES.has(deepestRouteName(state) ?? ''));
          if (state) void AsyncStorage.setItem(NAV_STATE_KEY, JSON.stringify(state));
        }}
      >
        <TaskStoreProvider>
          {/* Day-start checks (deadline-pin + the review modal) — need the task
              store, so they mount inside the provider; the review modal overlays
              the focused tab when the gate opens it. */}
          <DayStartChecks />
          {/* §19.11 first-launch wizard gate. On a genuinely fresh instance (no
              account / no sync / empty store) it opens the wizard once;
              otherwise it's a no-op. */}
          <FirstLaunchWizardGate />
          {/* App-icon badge: today's open tasks + upcoming events. */}
          <AppBadge />
          {/* Shared event-scope chooser (delete/edit "this occurrence / this and
              all following / whole series"). App-global so the row handlers can
              open it imperatively; overlays whatever tab/editor is focused. */}
          <EventScopeDialogHost />
          {/* The root sync-status poll feeds the per-screen header indicator
              (the native tab bar has no slot for an extra custom control). */}
          <SyncStatusContext.Provider value={sync}>{tabs}</SyncStatusContext.Provider>
        </TaskStoreProvider>
      </NavigationContainer>
    </>
  );
}
