import type { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { useCallback, useEffect, useMemo, useState } from 'react';

import { localDateKey, type TaskList } from '@aperio/shared';

import { listTaskLists } from '../api/client';
import type { RootStackParamList } from '../navigation/types';

// "New task on this day" — the task twin of `useNewEventOnDay`, feeding the
// CalendarActions toolbar button and the day headers' custom actions. The
// target is the task quick-add anchored on the given day; the quick-add picks
// the list itself (last-used), so this hook only needs to know whether any
// writable list exists at all, to grey the button honestly instead of hiding
// it.

export function useNewTaskOnDay(
  navigation: NativeStackNavigationProp<RootStackParamList>,
  anchorDay: Date,
): { addTask: () => void; addTaskOnDay: (dayKey: string) => void; enabled: boolean } {
  const [taskLists, setTaskLists] = useState<TaskList[]>([]);

  // Refresh on focus, like the calendar twin: lists can be added/removed on
  // the Tasks tab and on other devices via sync.
  useEffect(() => {
    const read = () =>
      void listTaskLists()
        .then(setTaskLists)
        .catch(() => setTaskLists([]));
    const unsubscribe = navigation.addListener('focus', read);
    read();
    return unsubscribe;
  }, [navigation]);

  const enabled = useMemo(
    () => taskLists.some((l) => !l.read_only),
    [taskLists],
  );

  const addTaskOnDay = useCallback(
    (dayKey: string) => {
      // → the task quick-add (expands to the full editor via "More details …").
      navigation.navigate('QuickAdd', { initialScheduledDate: dayKey });
    },
    [navigation],
  );

  const addTask = useCallback(
    () => addTaskOnDay(localDateKey(anchorDay)),
    [addTaskOnDay, anchorDay],
  );

  return { addTask, addTaskOnDay, enabled };
}
