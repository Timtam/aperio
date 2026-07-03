import { useCallback, useEffect, useState } from 'react';
import { Appearance } from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';

// Device-local light/dark/system theme mode, the mobile twin of the desktop's
// `src/state/themeMode.ts`. 'system' (the default) follows the OS appearance
// live via useColorScheme; 'light'/'dark' pin the palette. Stored per device
// in AsyncStorage (NOT synced — a bright-office phone and an OLED tablet want
// different modes) and mirrored into a module-level cache with listeners, so
// the ThemeProvider re-resolves immediately when the setting changes without
// an async read on every render. The high-contrast accessibility override in
// ThemeProvider always wins over this choice.

export type ThemeModeChoice = 'system' | 'light' | 'dark';

const KEY = 'aperio.ui.themeMode';

let cached: ThemeModeChoice = 'system';
let loaded = false;
const listeners = new Set<(mode: ThemeModeChoice) => void>();

function isChoice(raw: string | null): raw is ThemeModeChoice {
  return raw === 'system' || raw === 'light' || raw === 'dark';
}

function notify(): void {
  for (const listener of listeners) listener(cached);
}

/** Propagate a pinned choice to the OS-drawn surfaces too — the software
 *  keyboard, native date/time picker sheets and alerts follow the app-level
 *  colour scheme, not our JS tokens (the mobile analogue of the desktop's
 *  `color-scheme` CSS). `null` restores the system follow. */
function applyNativeColorScheme(mode: ThemeModeChoice): void {
  try {
    // 'unspecified' restores the system follow (this RN's setColorScheme
    // takes 'light' | 'dark' | 'unspecified').
    Appearance.setColorScheme(mode === 'system' ? 'unspecified' : mode);
  } catch {
    // Older runtime without the setter — the JS palette still applies.
  }
}

/** One-shot load of the stored pref into the cache (idempotent — every hook
 *  mount and the App-start gate call it; only the first actually reads).
 *  Exported for App.tsx's readiness gate so a pinned theme is resolved
 *  BEFORE the first frame instead of racing it. */
export async function loadThemeModePref(): Promise<void> {
  if (loaded) {
    return;
  }
  loaded = true;
  try {
    const raw = await AsyncStorage.getItem(KEY);
    if (isChoice(raw) && raw !== cached) {
      cached = raw;
      notify();
    }
  } catch {
    // Best-effort; the default (system) stays.
  }
  applyNativeColorScheme(cached);
}

async function persist(next: ThemeModeChoice): Promise<void> {
  cached = next;
  notify();
  applyNativeColorScheme(next);
  try {
    await AsyncStorage.setItem(KEY, next);
  } catch {
    // Best-effort — the choice still applies for this session.
  }
}

/** The current choice, live: re-renders when the setting changes anywhere
 *  (settings screen) and after the initial async load. ThemeProvider's hook. */
export function useThemeModeChoice(): ThemeModeChoice {
  const [choice, setChoice] = useState<ThemeModeChoice>(cached);
  useEffect(() => {
    listeners.add(setChoice);
    setChoice(cached);
    void loadThemeModePref();
    return () => {
      listeners.delete(setChoice);
    };
  }, []);
  return choice;
}

/** Settings hook: the current choice + a setter that persists + notifies. */
export function useThemeModePref(): [
  ThemeModeChoice,
  (next: ThemeModeChoice) => void,
] {
  const choice = useThemeModeChoice();
  const set = useCallback((next: ThemeModeChoice) => {
    void persist(next);
  }, []);
  return [choice, set];
}
