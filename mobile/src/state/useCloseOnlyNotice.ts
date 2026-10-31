import { useCallback, useEffect, useRef, useState } from 'react';
import { AccessibilityInfo } from 'react-native';

/**
 * The notice a screen shows (`CloseOnlyNotice`), and how to show it. A screen
 * left while its write was on the way is gone: the sentence is announced
 * instead, so it is never lost. Mirrors the desktop's hook.
 */
export function useCloseOnlyNotice(): {
  notice: string | null;
  showNotice: (sentence: string) => void;
} {
  const [notice, setNotice] = useState<string | null>(null);
  const shown = useRef(false);
  useEffect(() => {
    shown.current = true;
    return () => {
      shown.current = false;
    };
  }, []);
  const showNotice = useCallback((sentence: string) => {
    if (shown.current) setNotice(sentence);
    else AccessibilityInfo.announceForAccessibility(sentence);
  }, []);
  return { notice, showNotice };
}
