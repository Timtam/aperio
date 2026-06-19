import { createNativeBottomTabNavigator } from '@bottom-tabs/react-navigation';
import { NavigationContainer } from '@react-navigation/native';
import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { StatusBar } from 'expo-status-bar';
import { useTranslation } from 'react-i18next';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import { useSyncTriggers } from './api/syncTriggers';
import DayStartReviewModal from './components/DayStartReviewModal';
import { useCacheUpdates } from './state/cacheObserver';
import { ThemeProvider, useTheme, navigationThemeFor } from './theme';
import type { RootStackParamList, RootTabParamList } from './navigation/types';
import { useReminderTriggers } from './reminders/scheduler';
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

function TasksStackNav() {
  const { t } = useTranslation();
  return (
    <TasksStack.Navigator initialRouteName="Tasks">
      <TasksStack.Screen
        name="Tasks"
        component={TasksScreen}
        options={{ title: t('views.tasks.title') }}
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
    <CalendarStack.Navigator initialRouteName="Events">
      <CalendarStack.Screen
        name="Events"
        component={EventsScreen}
        options={{ title: t('mobile.eventsTitle') }}
      />
      <CalendarStack.Screen
        name="Week"
        component={WeekScreen}
        options={{ title: t('views.week.title') }}
      />
      <CalendarStack.Screen
        name="Month"
        component={MonthScreen}
        options={{ title: t('views.month.title') }}
      />
      <CalendarStack.Screen
        name="Agenda"
        component={AgendaScreen}
        options={{ title: t('views.agenda.title') }}
      />
      <CalendarStack.Screen
        name="Year"
        component={YearScreen}
        options={{ title: t('views.year.title') }}
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
    <ContactsStack.Navigator initialRouteName="Contacts">
      <ContactsStack.Screen
        name="Contacts"
        component={ContactsScreen}
        options={{ title: t('sidebar.contactLists') }}
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
    <SettingsStack.Navigator initialRouteName="Settings">
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
  return (
    <>
      {/* Dark status-bar glyphs on the light background, light glyphs on the
          dark / high-contrast backgrounds. */}
      <StatusBar style={theme.mode === 'light' ? 'dark' : 'light'} />
      <NavigationContainer theme={navigationThemeFor(theme)}>
        <TaskStoreProvider>
          {/* Day-start checks (deadline-pin + the review modal) — need the task
              store, so they mount inside the provider; the review modal overlays
              the focused tab when the gate opens it. */}
          <DayStartChecks />
          <Tab.Navigator initialRouteName="TasksTab">
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
                // Flag sync issues on the Settings tab (Sync lives under it) with
                // a badge. The native tab bar exposes no per-tab accessibility
                // label (unlike the JS bottom tabs), so the spoken sync state now
                // comes solely from useSyncStatus's live-region announcements of
                // attention-class transitions, not from the tab's label.
                tabBarBadge: sync.badge != null ? String(sync.badge) : undefined,
              }}
            />
          </Tab.Navigator>
        </TaskStoreProvider>
      </NavigationContainer>
    </>
  );
}
