import { NavigationContainer } from '@react-navigation/native';
import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { StatusBar } from 'expo-status-bar';
import { useTranslation } from 'react-i18next';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import { useSyncTriggers } from './api/syncTriggers';
import type { RootStackParamList } from './navigation/types';
import AccountsScreen from './screens/AccountsScreen';
import EventEditorModal from './screens/EventEditorModal';
import EventsScreen from './screens/EventsScreen';
import ListsScreen from './screens/ListsScreen';
import SettingsScreen from './screens/SettingsScreen';
import SyncScreen from './screens/SyncScreen';
import TaskEditorModal from './screens/TaskEditorModal';
import TasksScreen from './screens/TasksScreen';
import { useStoredLanguage } from './settings/language';
import { TaskStoreProvider } from './state/taskStore';

// Aperio mobile — navigation host for the faithful tasks port (M-series).
//
// Provider order (per the React Navigation 7 docs for Expo SDK 56):
// SafeAreaProvider > NavigationContainer > TaskStoreProvider > Navigator. The
// store sits inside the container so screens and the store both see navigation,
// and wraps the navigator so every screen consumes the one store.
//
// We use a native-stack: its header title is announced by TalkBack/VoiceOver
// (so screen titles come from i18n), and it needs no gesture-handler. The
// editor is presented as a modal. The UI is rebuilt per platform; every string
// comes from i18next (@aperio/locales), every domain type from @aperio/shared,
// and persistence runs through the shared Rust core via the cal-ffi bridge.

const Stack = createNativeStackNavigator<RootStackParamList>();

export default function App() {
  const { t } = useTranslation();
  // Apply the stored language override (if any) over the device-locale default.
  useStoredLanguage();
  // JS-driven sync: full round on launch + every foreground-resume, a push on
  // background, and a debounced push after each mutation (wired in the api
  // clients). The mobile stand-in for the desktop SyncScheduler.
  useSyncTriggers();
  return (
    <SafeAreaProvider>
      <StatusBar style="auto" />
      <NavigationContainer>
        <TaskStoreProvider>
          <Stack.Navigator initialRouteName="Tasks">
            <Stack.Screen
              name="Tasks"
              component={TasksScreen}
              options={{ title: t('views.tasks.title') }}
            />
            <Stack.Screen
              name="Lists"
              component={ListsScreen}
              options={{ title: t('mobile.listsButtonLabel') }}
            />
            <Stack.Screen
              name="Accounts"
              component={AccountsScreen}
              options={{ title: t('dialogs.accounts.title') }}
            />
            <Stack.Screen
              name="Events"
              component={EventsScreen}
              options={{ title: t('mobile.eventsTitle') }}
            />
            <Stack.Screen
              name="Sync"
              component={SyncScreen}
              options={{ title: t('mobile.syncTitle') }}
            />
            <Stack.Screen
              name="Settings"
              component={SettingsScreen}
              options={{ title: t('dialogs.settings.title') }}
            />
            <Stack.Screen
              name="TaskEditor"
              component={TaskEditorModal}
              options={{ presentation: 'modal', title: t('mobile.newTaskLabel') }}
            />
            <Stack.Screen
              name="EventEditor"
              component={EventEditorModal}
              options={{ presentation: 'modal', title: t('dialogs.event.newTitle') }}
            />
          </Stack.Navigator>
        </TaskStoreProvider>
      </NavigationContainer>
    </SafeAreaProvider>
  );
}
