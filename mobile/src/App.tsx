import { createBottomTabNavigator } from '@react-navigation/bottom-tabs';
import { NavigationContainer } from '@react-navigation/native';
import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { StatusBar } from 'expo-status-bar';
import { useTranslation } from 'react-i18next';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import { useSyncTriggers } from './api/syncTriggers';
import type { RootStackParamList, RootTabParamList } from './navigation/types';
import { useReminderTriggers } from './reminders/scheduler';
import { useStoredLanguage } from './settings/language';
import AccountsScreen from './screens/AccountsScreen';
import ContactEditorModal from './screens/ContactEditorModal';
import ColorLabelsScreen from './screens/ColorLabelsScreen';
import ContactsScreen from './screens/ContactsScreen';
import AgendaScreen from './screens/AgendaScreen';
import CalendarEditorModal from './screens/CalendarEditorModal';
import CalendarsScreen from './screens/CalendarsScreen';
import EventEditorModal from './screens/EventEditorModal';
import EventsScreen from './screens/EventsScreen';
import ListEditorModal from './screens/ListEditorModal';
import ListsScreen from './screens/ListsScreen';
import RemindersScreen from './screens/RemindersScreen';
import SettingsScreen from './screens/SettingsScreen';
import SyncScreen from './screens/SyncScreen';
import TaskEditorModal from './screens/TaskEditorModal';
import TasksScreen from './screens/TasksScreen';
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

const Tab = createBottomTabNavigator<RootTabParamList>();
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
      <TasksStack.Screen
        name="TaskEditor"
        component={TaskEditorModal}
        options={{ presentation: 'modal', title: t('mobile.newTaskLabel') }}
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
        name="Agenda"
        component={AgendaScreen}
        options={{ title: t('views.agenda.title') }}
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
        name="Reminders"
        component={RemindersScreen}
        options={{ title: t('dialogs.reminders.title') }}
      />
      <SettingsStack.Screen
        name="ColorLabels"
        component={ColorLabelsScreen}
        options={{ title: t('dialogs.colorLabels.title') }}
      />
    </SettingsStack.Navigator>
  );
}

export default function App() {
  const { t } = useTranslation();
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
  return (
    <SafeAreaProvider>
      <StatusBar style="auto" />
      <NavigationContainer>
        <TaskStoreProvider>
          <Tab.Navigator initialRouteName="TasksTab">
            <Tab.Screen
              name="TasksTab"
              component={TasksStackNav}
              options={{ headerShown: false, title: t('views.tasks.title') }}
            />
            <Tab.Screen
              name="CalendarTab"
              component={CalendarStackNav}
              options={{ headerShown: false, title: t('mobile.eventsButtonLabel') }}
            />
            <Tab.Screen
              name="ContactsTab"
              component={ContactsStackNav}
              options={{ headerShown: false, title: t('sidebar.contactLists') }}
            />
            <Tab.Screen
              name="SettingsTab"
              component={SettingsStackNav}
              options={{ headerShown: false, title: t('dialogs.settings.title') }}
            />
          </Tab.Navigator>
        </TaskStoreProvider>
      </NavigationContainer>
    </SafeAreaProvider>
  );
}
