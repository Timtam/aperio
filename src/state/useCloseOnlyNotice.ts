import { useCallback, useEffect, useRef, useState } from 'react';

import { useAnnouncer } from '../a11y/announcerContext';

/**
 * The notice a dialog shows (`CloseOnlyNotice`), and how to show it. A dialog
 * closed while its write was on the way is gone: the sentence is announced
 * instead, so it is never lost.
 */
export function useCloseOnlyNotice(): {
  notice: string | null;
  showNotice: (sentence: string) => void;
} {
  const announce = useAnnouncer();
  const [notice, setNotice] = useState<string | null>(null);
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const showNotice = useCallback(
    (sentence: string) => {
      if (mounted.current) setNotice(sentence);
      else announce(sentence);
    },
    [announce],
  );
  return { notice, showNotice };
}
