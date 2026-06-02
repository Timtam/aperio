import { invoke } from '@tauri-apps/api/core';

/**
 * DEV diagnostic: mirror webview `console.*` output into the Rust/tracing
 * stream (target `aperio::webview`) so frontend logs appear in the same dev
 * terminal as backend logs. Patches `console.{log,info,warn,error,debug}` to
 * also forward to the `frontend_log` Tauri command, while still calling the
 * original (so the browser devtools console keeps working too).
 *
 * Install once at startup, gated on `import.meta.env.DEV` — production builds
 * never forward.
 */
export function installConsoleBridge(): void {
  const levels = ['log', 'info', 'warn', 'error', 'debug'] as const;
  for (const level of levels) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      original(...args);
      try {
        const message = args.map(format).join(' ').slice(0, 4000);
        void invoke('frontend_log', { level, message }).catch(() => {
          /* backend not ready / not in Tauri — ignore */
        });
      } catch {
        /* never let logging throw */
      }
    };
  }
}

function format(value: unknown): string {
  if (typeof value === 'string') return value;
  if (value instanceof Error) {
    return `${value.name}: ${value.message}`;
  }
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}
