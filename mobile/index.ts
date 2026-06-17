import { registerRootComponent } from 'expo';

// Initialise i18next (shared translations) before the app renders.
import './i18n';
import App from './App';

// registerRootComponent calls AppRegistry.registerComponent('main', () => App);
// It also ensures that whether you load the app in Expo Go or in a native build,
// the environment is set up appropriately
registerRootComponent(App);
