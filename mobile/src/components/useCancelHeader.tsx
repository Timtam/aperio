import { useEffect, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

import { HeaderCancelButton } from './HeaderCancelButton';

/**
 * Install a header-left button that leaves the screen — the first element a
 * screen-reader user reaches. Called from the screen itself, so it covers
 * every stack the screen is registered in.
 *
 * `labelKey` names what leaving MEANS here. An editor cancels; a catalogue
 * screen goes back, and calling that "cancel" would suggest the toggles just
 * made are about to be undone.
 */
export function useCancelHeader(
  navigation: {
    setOptions: (options: {
      headerLeft: () => ReactElement;
      gestureEnabled: boolean;
    }) => void;
    goBack: () => void;
  },
  labelKey: 'mobile.cancel' | 'mobile.back' = 'mobile.cancel',
): void {
  const { t } = useTranslation();
  useEffect(() => {
    navigation.setOptions({
      headerLeft: () => (
        <HeaderCancelButton label={t(labelKey)} onPress={() => navigation.goBack()} />
      ),
      // A custom header-left hides the NATIVE back button, and the swipe-back
      // gesture hangs off it. Say so explicitly: nobody loses a way out by
      // gaining one.
      gestureEnabled: true,
    });
  }, [navigation, t, labelKey]);
}
