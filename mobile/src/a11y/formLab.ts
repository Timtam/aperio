import { useSyncExternalStore } from 'react';
import { Platform } from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';

import { A11yGestures } from '../../modules/a11y-gestures';
import { logLine } from '../api/logs';

// The VoiceOver form lab of a diagnostic build (iOS only, temporary). With the
// keyboard up, VoiceOver swipes from the last field above the keyboard into
// the keyboard instead of on through the form; the lab carries the candidate
// fixes behind switches, so the tester can compare them in one build. The
// winner gets built in properly and the lab goes again. See
// modules/a11y-gestures (FormLab) and modules/a11y-form.
//
// Kept on this device only (AsyncStorage), never synced; every switch starts
// off, so the app behaves as without the lab until one is turned on.

export type WindowGestures = 'normal' | 'narrow' | 'off';

export interface FormLabFlags {
  /** The form lists its fields to VoiceOver itself, as a table does. */
  container: boolean;
  /** The form scrolls so the next field is above the keyboard. */
  focusFollows: boolean;
  /** The keyboard closes when VoiceOver focus leaves the text field. */
  resignOnLeave: boolean;
  /** The window's three-finger scroll handling: as before, left/right only,
   *  or handed back to iOS. */
  windowGestures: WindowGestures;
  /** What VoiceOver did, without labels or values, into Aperio's log. */
  log: boolean;
}

const KEY = 'aperio.a11yFormLab.v1';
const OFF: FormLabFlags = {
  container: false,
  focusFollows: false,
  resignOnLeave: false,
  windowGestures: 'normal',
  log: false,
};

let flags: FormLabFlags = OFF;
const listeners = new Set<() => void>();

function push(): void {
  if (Platform.OS !== 'ios') return;
  A11yGestures.setLabFlags({ ...flags });
}

function parse(raw: string | null): FormLabFlags {
  if (raw == null) return OFF;
  try {
    const stored = JSON.parse(raw) as Partial<FormLabFlags>;
    const mode = stored.windowGestures;
    return {
      container: stored.container === true,
      focusFollows: stored.focusFollows === true,
      resignOnLeave: stored.resignOnLeave === true,
      windowGestures: mode === 'narrow' || mode === 'off' ? mode : 'normal',
      log: stored.log === true,
    };
  } catch {
    return OFF;
  }
}

/** Change switches: kept on this device, handed to the native side at once. */
export function setFormLab(patch: Partial<FormLabFlags>): void {
  flags = { ...flags, ...patch };
  push();
  listeners.forEach((listener) => listener());
  void AsyncStorage.setItem(KEY, JSON.stringify(flags)).catch(() => undefined);
}

export function getFormLab(): FormLabFlags {
  return flags;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useFormLab(): FormLabFlags {
  return useSyncExternalStore(subscribe, getFormLab);
}

if (Platform.OS === 'ios') {
  // The native lab's lines into Aperio's log; it only sends them while the
  // log switch is on.
  A11yGestures.addListener('labLog', ({ line }) => {
    void logLine('info', `a11y-lab ${line}`);
  });
  void AsyncStorage.getItem(KEY)
    .then((raw) => {
      flags = parse(raw);
      push();
      listeners.forEach((listener) => listener());
    })
    .catch(() => undefined);
}
