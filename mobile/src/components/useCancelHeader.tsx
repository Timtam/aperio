import { useEffect, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';

import { HeaderCancelButton } from './HeaderCancelButton';

/** Install a Cancel button as a modal screen's header-left (the first element a
 *  screen-reader user reaches). Called from the screen itself, so it covers
 *  every stack the screen is registered in. */
export function useCancelHeader(navigation: {
  setOptions: (options: { headerLeft: () => ReactElement }) => void;
  goBack: () => void;
}): void {
  const { t } = useTranslation();
  useEffect(() => {
    navigation.setOptions({
      headerLeft: () => (
        <HeaderCancelButton label={t('mobile.cancel')} onPress={() => navigation.goBack()} />
      ),
    });
  }, [navigation, t]);
}
