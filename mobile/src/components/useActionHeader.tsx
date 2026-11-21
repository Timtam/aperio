import { useEffect, type ReactElement } from 'react';

import { HeaderTextButton } from './HeaderTextButton';

/** The screen's main action in the header: Save in an editor, Send in the
 *  feedback form. */
export interface HeaderAction {
  label: string;
  onPress: () => void;
  /** A write runs, or the form cannot be sent yet: the button stays in place
   *  and says it is unavailable. */
  disabled?: boolean;
}

/**
 * Install the screen's main action as a header-right button, so it is always
 * on screen. At the end of a long form it sat below the fold, and with the
 * keyboard open VoiceOver did not scroll down to it: a VoiceOver user reached
 * Save or Send only with a three-finger scroll (Toni, builds 72 and 74). In the
 * header it is two swipes from the title, and a sighted user sees it without
 * scrolling either.
 *
 * `action` null takes the button away, for a screen state that has nothing to
 * save (a notice that can only be closed, a contact shown read-only).
 */
export function useActionHeader(
  navigation: {
    setOptions: (options: { headerRight: (() => ReactElement) | undefined }) => void;
  },
  action: HeaderAction | null,
): void {
  const label = action?.label;
  const onPress = action?.onPress;
  const disabled = action?.disabled ?? false;
  useEffect(() => {
    navigation.setOptions({
      headerRight:
        label != null && onPress != null
          ? () => <HeaderTextButton label={label} disabled={disabled} onPress={onPress} />
          : undefined,
    });
  }, [navigation, label, onPress, disabled]);
}
