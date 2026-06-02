import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import { installConsoleBridge } from './dev/consoleBridge';
import './i18n';
import './styles.css';

// DEV: mirror webview console output into the Rust/terminal log stream.
// (Vite's `import.meta.env` isn't typed in this project, so read it
// defensively rather than pulling in `vite/client`.)
const isDev =
  (import.meta as unknown as { env?: { DEV?: boolean } }).env?.DEV ?? false;
if (isDev) {
  installConsoleBridge();
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
