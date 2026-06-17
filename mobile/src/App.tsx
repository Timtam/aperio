import { NavigationContainer } from '@react-navigation/native';
import { createNativeStackNavigator } from '@react-navigation/native-stack';
import { StatusBar } from 'expo-status-bar';
import { useTranslation } from 'react-i18next';
import { SafeAreaProvider } from 'react-native-safe-area-context';

import type { RootStackParamList } from './navigation/types';
import AccountsScreen from './screens/AccountsScreen';
import ListsScreen from './screens/ListsScreen';
import TaskEditorModal from './screens/TaskEditorModal';
import TasksScreen from './screens/TasksScreen';
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
              name="TaskEditor"
              component={TaskEditorModal}
              options={{ presentation: 'modal', title: t('mobile.newTaskLabel') }}
            />
          </Stack.Navigator>
        </TaskStoreProvider>
      </NavigationContainer>
    </SafeAreaProvider>
  );
}
