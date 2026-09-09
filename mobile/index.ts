import { registerRootComponent } from 'expo';

import { installTextCollation } from '@aperio/shared';

// Initialise i18next (shared translations) before the app renders.
import i18n from './i18n';
import CalFfi from './modules/cal-ffi';
import App from './src/App';

// The core's text-ordering rule, reached through a SYNCHRONOUS bridge call
// (`Function`, not `AsyncFunction`) because its callers are `Array.sort`
// comparators. Installed before the first render for the same reason the
// desktop awaits its WebAssembly module at startup — see DESIGN §4.3 for the
// door, §4.4 for the rule that goes through it.
//
// Bound to the language the USER chose. Every `localeCompare` this replaces
// passed `undefined` and so followed the DEVICE, which is why a German list
// could order differently on a phone than on the desktop.
const installCollation = () =>
  installTextCollation({
    compareNames: (a, b) => CalFfi.compareNames(a, b, i18n.language),
    compareTitles: (a, b) => CalFfi.compareTitles(a, b, i18n.language),
  });
installCollation();
i18n.on('languageChanged', installCollation);

// registerRootComponent calls AppRegistry.registerComponent('main', () => App);
// It also ensures that whether you load the app in Expo Go or in a native build,
// the environment is set up appropriately
registerRootComponent(App);
