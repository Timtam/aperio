import { useEffect, type ReactElement } from 'react';
import { BackHandler } from 'react-native';
import { useTranslation } from 'react-i18next';

import { HeaderCancelButton } from './HeaderCancelButton';

/** What else decides how the screen may be left, beyond the plain button. */
export interface CancelHeaderHold {
  /** A write is in flight: the screen is not left until it is done, as the
   *  desktop's dialogs are not closed (decision 210) — a write left behind
   *  would run unseen, and what it left would never be said on screen. The
   *  button stays, marked unavailable, the swipe is off and the Android back
   *  button is taken. The screen's own navigation after the write goes on. */
  writing?: boolean;
  /** The screen shows a notice that can only be closed: the button says Close
   *  and does this, and the swipe is off. */
  close?: () => void;
}

/**
 * Install a header-left button that leaves the screen — the first element a
 * screen-reader user reaches. Called from the screen itself, so it covers
 * every stack the screen is registered in.
 *
 * `labelKey` names what leaving MEANS here. An editor cancels; a catalogue
 * screen goes back, and calling that "cancel" would suggest the toggles just
 * made are about to be undone; a list whose writes happen only when asked
 * (the import list) closes.
 *
 * `hold` is the one place a screen changes that: a header set from two places
 * left whichever ran last, not the one that applies.
 */
export function useCancelHeader(
  navigation: {
    setOptions: (options: {
      headerLeft: () => ReactElement;
      gestureEnabled: boolean;
    }) => void;
    goBack: () => void;
  },
  labelKey: 'mobile.cancel' | 'mobile.back' | 'dialogs.close' = 'mobile.cancel',
  hold: CancelHeaderHold = {},
): void {
  const { t } = useTranslation();
  const { writing = false, close } = hold;
  useEffect(() => {
    navigation.setOptions({
      headerLeft: () =>
        close != null ? (
          <HeaderCancelButton label={t('dialogs.close')} onPress={close} />
        ) : (
          <HeaderCancelButton
            label={t(labelKey)}
            disabled={writing}
            onPress={() => navigation.goBack()}
          />
        ),
      // A custom header-left hides the NATIVE back button, and the swipe-back
      // gesture hangs off it. Say so explicitly: nobody loses a way out by
      // gaining one — except while a write runs, or a notice must be closed.
      gestureEnabled: close == null && !writing,
    });
  }, [navigation, t, labelKey, writing, close]);
  useEffect(() => {
    if (!writing) return;
    // Taken while the write runs; a dialog above the screen (the wait) gets
    // the back button first either way.
    const back = BackHandler.addEventListener('hardwareBackPress', () => true);
    return () => back.remove();
  }, [writing]);
}
