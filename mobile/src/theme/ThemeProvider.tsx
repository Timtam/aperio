import { type ReactNode, useEffect, useState } from 'react';
import {
  AccessibilityInfo,
  Platform,
  useColorScheme,
  type EmitterSubscription,
} from 'react-native';

import { ThemeContext } from './context';
import { useThemeModeChoice } from './themeMode';
import { THEMES, type ThemeMode } from './tokens';

// Theming, mirroring the desktop:
//   - light / dark ← the device-local theme-mode setting; its default
//     'system' follows useColorScheme() live, 'light'/'dark' pin the palette
//     (Settings → Allgemein, same three-way choice as the desktop).
//   - high-contrast ← the platform "increase contrast" accessibility flag
//     (iOS "Increase Contrast" = darkerSystemColors; Android = highTextContrast),
//     which wins over the light/dark choice when on, matching the desktop
//     forced-colors override.

/** Whether the platform's high-contrast accessibility setting is on. iOS and
 *  Android expose different flags; query whichever exists. */
async function readHighContrast(): Promise<boolean> {
  try {
    if (Platform.OS === 'ios') {
      return await AccessibilityInfo.isDarkerSystemColorsEnabled();
    }
    if (Platform.OS === 'android') {
      return await AccessibilityInfo.isHighTextContrastEnabled();
    }
  } catch {
    // Either method may be unavailable on a given platform/version — treat an
    // error as "high contrast off" and fall back to the light/dark scheme.
  }
  return false;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const scheme = useColorScheme();
  const choice = useThemeModeChoice();
  const [highContrast, setHighContrast] = useState(false);

  useEffect(() => {
    let active = true;
    void readHighContrast().then((on) => {
      if (active) setHighContrast(on);
    });

    // Subscribe to the platform-appropriate change event so a mid-session
    // accessibility toggle re-themes live.
    const eventName =
      Platform.OS === 'ios'
        ? 'darkerSystemColorsChanged'
        : 'highTextContrastChanged';
    let sub: EmitterSubscription | undefined;
    try {
      sub = AccessibilityInfo.addEventListener(eventName, (on: boolean) => {
        setHighContrast(on);
      });
    } catch {
      // Event unsupported on this platform — the one-shot read above still
      // applies the initial value.
    }
    return () => {
      active = false;
      sub?.remove();
    };
  }, []);

  const mode: ThemeMode = highContrast
    ? 'highContrast'
    : choice === 'system'
      ? scheme === 'dark'
        ? 'dark'
        : 'light'
      : choice;

  return (
    <ThemeContext.Provider value={THEMES[mode]}>
      {children}
    </ThemeContext.Provider>
  );
}
