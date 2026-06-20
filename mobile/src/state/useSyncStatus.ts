import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessibilityInfo, AppState } from 'react-native';

import { syncConflictCount, syncStatus, type SyncStatus } from '../api/sync';
import { subscribeSyncActivity } from './syncActivity';

// App-wide sync-status surfacing — the mobile twin of the desktop
// SyncStatusIndicator + useSync (DESIGN §19.9). Polls the (already-bridged)
// engine status + unresolved-conflict count, derives the same priority tone
// (schema_too_old > conflict > error > uploading > synced > off), and exposes a
// Settings-tab badge value for sighted users. For screen-reader users it
// ANNOUNCES only the attention-class transitions (schema/conflict/error) — on
// mobile `announceForAccessibility` interrupts the reader, so the benign
// synced/uploading states stay silent (the badge carries them visually); the
// desktop's always-visible badge + polite live region can afford to speak them,
// a one-shot interruption can't. The actionable detail + fixes live on the Sync
// screen (the badge just points there).

export type SyncTone = 'schema_too_old' | 'conflict' | 'error' | 'uploading' | 'synced' | 'off';

const POLL_MS = 30000;

/** Structural equality for the polled status — a fresh-but-identical object
 *  must NOT trigger a re-render. The shape is small + flat, so a JSON compare
 *  is both correct and cheap at the 30s cadence. */
function sameStatus(a: SyncStatus | null, b: SyncStatus | null): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function toneOf(status: SyncStatus | null, conflictCount: number): SyncTone {
  if (status?.schema_too_old) return 'schema_too_old';
  if (conflictCount > 0) return 'conflict';
  if (status?.configured && (status.sustained_failure || status.last_error_code != null))
    return 'error';
  if (status?.in_flight) return 'uploading';
  if (status?.configured) return 'synced';
  return 'off';
}

export interface SyncStatusInfo {
  tone: SyncTone;
  /** The localized state label (mirrors the desktop badge label). */
  label: string;
  conflictCount: number;
  status: SyncStatus | null;
  /** The Settings-tab badge value: the conflict count, "!" for error /
   *  schema-too-old, or undefined when there's nothing to flag. */
  badge: number | string | undefined;
}

/** Mount once near the app root: polls the sync status + announces
 *  attention-class transitions; returns the current badge/label for the shell. */
export function useSyncStatus(): SyncStatusInfo {
  const { t } = useTranslation();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [conflictCount, setConflictCount] = useState(0);

  const refresh = useCallback(async () => {
    try {
      const [s, c] = await Promise.all([syncStatus(), syncConflictCount()]);
      // Only re-render when something ACTUALLY changed. The 30s poll otherwise
      // hands React a fresh status object every tick, re-rendering the whole app
      // shell (and the native nav/tab views) for nothing — which on iOS resets
      // the VoiceOver cursor mid-navigation (focus "jumps"/blocks every poll).
      setStatus((prev) => (sameStatus(prev, s) ? prev : s));
      setConflictCount((prev) => (prev === c ? prev : c));
    } catch {
      // Best-effort — a transient bridge error just keeps the last known state.
    }
  }, []);

  // Poll on mount + every 30s while active + on each foreground resume (a
  // background round may have changed the state while we were away).
  useEffect(() => {
    void refresh();
    const id = setInterval(() => void refresh(), POLL_MS);
    const sub = AppState.addEventListener('change', (s) => {
      if (s === 'active') void refresh();
    });
    return () => {
      clearInterval(id);
      sub.remove();
    };
  }, [refresh]);

  // Re-read the status the moment a sync round finishes — the 30s poll is too
  // coarse to catch the just-settled state (a fresh conflict, a cleared error,
  // an updated last_synced_at). The indicator itself flips to "uploading" while
  // the round runs via syncActivity; this just pulls the result. Guarded by
  // sameStatus, so it only re-renders the shell on a real change.
  useEffect(
    () =>
      subscribeSyncActivity((active) => {
        if (!active) void refresh();
      }),
    [refresh],
  );

  const tone = toneOf(status, conflictCount);
  const isAuthError = tone === 'error' && status?.last_error_code === 'auth';
  // A peer turned on E2E (§19.7): the round fails with `encryption_required`.
  // It lands in the 'error' tone, but the fix is "enter the dataset passphrase"
  // (the Sync-screen adopt banner), NOT a network/credential check — so it gets
  // its own actionable label instead of the misleading "No connection".
  const isAdoptRequired = tone === 'error' && status?.last_error_code === 'encryption_required';

  const label = (() => {
    switch (tone) {
      case 'schema_too_old':
        return t('syncStatus.schemaTooOld');
      case 'conflict':
        return conflictCount === 1
          ? t('syncStatus.conflict_one')
          : t('syncStatus.conflict_other', { count: conflictCount });
      case 'error':
        return isAuthError
          ? t('syncStatus.authFailed')
          : isAdoptRequired
            ? t('syncStatus.adoptEncryptionRequired')
            : status?.sustained_failure
              ? t('syncStatus.sustainedFailure')
              : t('syncStatus.noConnection');
      case 'uploading':
        return t('syncStatus.uploading');
      case 'synced':
        return t('syncStatus.synced');
      case 'off':
        return t('syncStatus.off');
    }
  })();

  // Announce only attention-class transitions (schema/conflict/error). Track the
  // previous tone so a re-render doesn't re-speak the same state, and skip the
  // first observed tone (no transition to announce on launch).
  const prevTone = useRef<SyncTone | null>(null);
  useEffect(() => {
    if (prevTone.current === tone) return;
    const first = prevTone.current === null;
    prevTone.current = tone;
    if (first) return;
    if (tone === 'schema_too_old' || tone === 'conflict' || tone === 'error') {
      AccessibilityInfo.announceForAccessibility(`${t('syncStatus.label')}: ${label}`);
    }
  }, [tone, label, t]);

  const badge: number | string | undefined =
    tone === 'conflict'
      ? conflictCount
      : tone === 'error' || tone === 'schema_too_old'
        ? '!'
        : undefined;

  return { tone, label, conflictCount, status, badge };
}
