import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { installConsoleBridge } from './dev/consoleBridge';
import { applyUiScale, readUiScale } from './state/uiScale';
import './i18n';
import './styles.css';

// Mirror webview console output into the Rust log stream in EVERY build.
// In dev it surfaces in the terminal; in release it flows into the
// persistent log file so a user's exported log (Settings → Protokolle)
// captures frontend errors too.
installConsoleBridge();

// Apply the device-local UI scale (root font-size) BEFORE the first paint so
// the interface never renders at 100% and then jumps. Every rem token follows.
applyUiScale(readUiScale());

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
