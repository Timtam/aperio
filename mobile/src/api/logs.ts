// Mobile diagnostics/logs api-client — JSON-free passthrough over the Host's
// log facades (the rolling-file sink lives in the cal-ffi Host, the read/export
// + redaction in host-core). Mirrors the desktop commands/logs.rs surface. The
// log level is a DEVICE-LOCAL pref; the export is redacted by default.

import CalFfi from '../../modules/cal-ffi';

export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace';

/** The persisted log level, or the default when unset. */
export const getLogLevel = (): Promise<string> => CalFfi.getLogLevel();

/** Live-reload the filter + persist the choice (device-local). */
export const setLogLevel = (level: LogLevel): Promise<void> => CalFfi.setLogLevel(level);

/** Tail of the newest log file for the viewer (default 500 lines). */
export const getRecentLogs = (lines: number | null = null): Promise<string> =>
  CalFfi.getRecentLogs(lines);

/** The full (optionally redacted, default true) log bundle, capped to ~2 MB. */
export const collectLogs = (redact = true): Promise<string> => CalFfi.collectLogs(redact);

/** Remove the rotated log files (the active one is kept). */
export const clearLogs = (): Promise<void> => CalFfi.clearLogs();

/** The on-disk logs directory, for display. */
export const logsDirPath = (): Promise<string> => CalFfi.logsDirPath();

/** Write one line into the app's rolling log. Never throws — a diagnostic that
 *  can fail the thing it is diagnosing is worse than no diagnostic. */
export const logLine = (level: LogLevel, message: string): Promise<void> =>
  CalFfi.logLine(level, message).catch(() => undefined);
