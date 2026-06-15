import { invoke } from '@tauri-apps/api/core';

/**
 * Mirror webview `console.*` output into the Rust/tracing stream (target
 * `aperio::webview`) so frontend logs land in the same sinks as backend logs.
 * Patches `console.{log,info,warn,error,debug}` to also forward to the
 * `frontend_log` Tauri command, while still calling the original (so the
 * browser devtools console keeps working too).
 *
 * Installed in EVERY build (see `main.tsx`): in dev it surfaces in the
 * terminal; in release it flows into the persistent log file so a user's
 * exported log captures frontend errors. Messages are truncated to 4000
 * chars; failures to forward are swallowed (never let logging throw).
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
