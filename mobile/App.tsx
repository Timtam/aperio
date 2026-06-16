import { StatusBar } from 'expo-status-bar';
import { useCallback, useState } from 'react';
import {
  AccessibilityActionEvent,
  AccessibilityInfo,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import CalFfi from './modules/cal-ffi';

// Pilot screen whose only job is to prove, on a real iPhone under
// VoiceOver, that the building blocks Aperio cares about actually
// work through React Native:
//   - semantic headings and labels,
//   - custom accessibility ACTIONS surfaced in the VoiceOver
//     "Actions" rotor (swipe up/down on a focused element), and
//   - dynamic announcements (live region + announceForAccessibility).
// UI strings are German on purpose so they can be heard with the
// German VoiceOver voice while testing.

type Task = {
  id: string;
  title: string;
  done: boolean;
  due: string;
};

const INITIAL_TASKS: Task[] = [
  { id: '1', title: 'Einkaufsliste schreiben', done: false, due: 'heute' },
  { id: '2', title: 'Zahnarzttermin vereinbaren', done: false, due: 'heute' },
  { id: '3', title: 'Steuerunterlagen sortieren', done: true, due: 'gestern' },
];

export default function App() {
  const [tasks, setTasks] = useState<Task[]>(INITIAL_TASKS);
  const [status, setStatus] = useState('Bereit. Wische zu einer Aufgabe.');

  // Vertical-slice proof: this calls the Rust `cal-core` parser through the
  // cal-ffi UniFFI native module — real Rust running inside the RN process.
  const sampleEntry = 'Dr. Alice Schmidt <alice@example.com>';
  const parsedAttendee = CalFfi.parseAttendee(sampleEntry);

  const announce = useCallback((message: string) => {
    setStatus(message);
    AccessibilityInfo.announceForAccessibility(message);
  }, []);

  const setTask = useCallback((id: string, update: (task: Task) => Task) => {
    setTasks((prev) => prev.map((t) => (t.id === id ? update(t) : t)));
  }, []);

  const completeTask = useCallback(
    (task: Task) => {
      setTask(task.id, (t) => ({ ...t, done: true }));
      announce(`${task.title} erledigt.`);
    },
    [announce, setTask],
  );

  const rescheduleTask = useCallback(
    (task: Task) => {
      setTask(task.id, (t) => ({ ...t, due: 'morgen' }));
      announce(`${task.title} auf morgen verschoben.`);
    },
    [announce, setTask],
  );

  const toggleTask = useCallback(
    (task: Task) => {
      const nowDone = !task.done;
      setTask(task.id, (t) => ({ ...t, done: nowDone }));
      announce(`${task.title} ${nowDone ? 'erledigt' : 'wieder offen'}.`);
    },
    [announce, setTask],
  );

  const onAction = useCallback(
    (task: Task, event: AccessibilityActionEvent) => {
      switch (event.nativeEvent.actionName) {
        case 'complete':
          completeTask(task);
          break;
        case 'reschedule':
          rescheduleTask(task);
          break;
      }
    },
    [completeTask, rescheduleTask],
  );

  return (
    <View style={styles.screen}>
      <StatusBar style="auto" />
      <ScrollView contentContainerStyle={styles.content}>
        <Text accessibilityRole="header" style={styles.heading}>
          Aperio – Barrierefreiheits-Pilot
        </Text>

        <View style={styles.rustBox}>
          <Text accessibilityRole="header" style={styles.subheading}>
            Rust-Brücke (cal-ffi)
          </Text>
          <Text style={styles.rustLine}>Eingabe: {sampleEntry}</Text>
          <Text style={styles.rustLine}>
            Name aus Rust: {parsedAttendee.name ?? '—'}
          </Text>
          <Text style={styles.rustLine}>
            E-Mail aus Rust: {parsedAttendee.email}
          </Text>
        </View>

        <Text style={styles.intro}>
          Aktiviere VoiceOver. Wische zu einer Aufgabe und dann mit einem
          Finger nach oben oder unten, um die Aktionen „Erledigen“ und „Auf
          morgen verschieben“ zu hören. Doppeltippen schaltet die Aufgabe um.
        </Text>

        <Text
          style={styles.status}
          accessibilityLiveRegion="polite"
          accessibilityRole="text"
        >
          {status}
        </Text>

        <View accessibilityRole="list" style={styles.list}>
          {tasks.map((task) => (
            <Pressable
              key={task.id}
              accessible
              accessibilityRole="button"
              accessibilityLabel={`${task.title}, ${
                task.done ? 'erledigt' : 'offen'
              }, fällig ${task.due}`}
              accessibilityHint="Doppeltippen schaltet die Aufgabe um. Nach oben oder unten wischen für weitere Aktionen."
              accessibilityActions={[
                { name: 'complete', label: 'Erledigen' },
                { name: 'reschedule', label: 'Auf morgen verschieben' },
              ]}
              onAccessibilityAction={(event) => onAction(task, event)}
              onPress={() => toggleTask(task)}
              style={({ pressed }) => [styles.task, pressed && styles.taskPressed]}
            >
              <Text style={styles.taskCheck}>{task.done ? '✓' : '○'}</Text>
              <View style={styles.taskBody}>
                <Text style={[styles.taskTitle, task.done && styles.taskTitleDone]}>
                  {task.title}
                </Text>
                <Text style={styles.taskDue}>fällig {task.due}</Text>
              </View>
            </Pressable>
          ))}
        </View>
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  screen: {
    flex: 1,
    backgroundColor: '#ffffff',
  },
  content: {
    paddingTop: 72,
    paddingHorizontal: 20,
    paddingBottom: 40,
    gap: 16,
  },
  heading: {
    fontSize: 26,
    fontWeight: '700',
    color: '#10131a',
  },
  subheading: {
    fontSize: 20,
    fontWeight: '700',
    color: '#10131a',
  },
  rustBox: {
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#bcd0c7',
    backgroundColor: '#eef6f1',
    gap: 6,
  },
  rustLine: {
    fontSize: 16,
    color: '#1d3a2f',
  },
  intro: {
    fontSize: 16,
    lineHeight: 22,
    color: '#2b3240',
  },
  status: {
    fontSize: 15,
    fontWeight: '600',
    color: '#1d4ed8',
  },
  list: {
    gap: 12,
  },
  task: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 14,
    padding: 16,
    borderRadius: 12,
    borderWidth: 1,
    borderColor: '#c9d2e0',
    backgroundColor: '#f4f7fb',
  },
  taskPressed: {
    backgroundColor: '#e4ebf5',
  },
  taskCheck: {
    fontSize: 22,
    width: 26,
    textAlign: 'center',
    color: '#10131a',
  },
  taskBody: {
    flex: 1,
  },
  taskTitle: {
    fontSize: 18,
    color: '#10131a',
  },
  taskTitleDone: {
    textDecorationLine: 'line-through',
    color: '#5b6573',
  },
  taskDue: {
    fontSize: 14,
    color: '#5b6573',
    marginTop: 2,
  },
});
