import { useCallback, useEffect, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  AppState,
  Keyboard,
  Modal,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useTranslation } from 'react-i18next';

import i18n from '../../i18n';
import {
  authenticate,
  deviceEnrollment,
  disableAfterLostEnrollment,
  isAuthenticating,
  readAppLockEnabled,
  setAppLockCoverVisible,
  subscribeAppLockEnabled,
} from '../state/appLock';
import { AppLockLockedContext } from '../state/appLockContext';
import { useThemedStyles, type ThemeColors } from '../theme';

// The optional app lock (Settings → General, device-local, default off): a
// full-screen cover over the running app that only the OS authentication
// (Face ID / Touch ID / device code) opens. The app CONTENT stays mounted
// behind it — sync, reminders and navigation state keep working.
//
// The cover is a React Native <Modal>, NOT an in-tree overlay view: every
// dialog in this app is itself an RN Modal, which lives in its own native
// window ABOVE the whole React root — an in-tree cover would sit UNDER an
// open editor or the day-start review. As a Modal presented at lock time the
// cover stacks above whatever was open, it owns screen-reader and keyboard
// focus (no traversal into hidden content), and the soft keyboard is
// dismissed on lock so nothing stays attached to a hidden input. Flows that
// AUTO-OPEN modals (the day-start review, the first-launch wizard) hold
// while `AppLockLockedContext` is true, and the window-level VoiceOver
// gestures are refused while the cover is up (`setAppLockCoverVisible`) — a
// modal presented or a gesture routed WHILE the cover is up would act above
// or behind it.
//
// While the pref read hasn't answered yet (a few ms at cold start) the
// content is hidden by an IN-TREE cover instead: presenting and dismissing a
// native Modal on every launch would flash and churn screen-reader focus for
// the default-off majority, and no dialog can be open that early.
//
// Lock points: cold start (armed while foregrounded; a background relaunch by
// the OS sync task arms the prompt for the first real open instead of burning
// it into the void), and a foreground-return after at least RELOCK_AFTER_MS
// in the background. While the app is merely inactive/backgrounded with the
// lock enabled, the same cover (without the button or the "locked" claim)
// hides the content so the OS app-switcher snapshot shows the cover, not the
// calendar. (On Android the snapshot capture can win that race — accepted:
// the lock itself, not the thumbnail, is the protection.)
//
// The prompt shows automatically ONCE per lock engagement; a cancelled or
// failed attempt parks on the cover with its Unlock button — re-prompting on
// every AppState flip would trap the user in a sheet loop, because
// dismissing the sheet itself fires 'active'. LEAVING the app while parked
// re-arms the auto-prompt, so a return hours later greets the user with the
// sheet again rather than a hunt for the button.

/** Background time after which a foreground-return re-locks. */
const RELOCK_AFTER_MS = 60_000;

/** Deferral for announcements that follow an OS-sheet or cover dismissal —
 *  spoken immediately they lose the race against the system's own
 *  screen-change chatter and are clobbered mid-word. */
const ANNOUNCE_DELAY_MS = 400;

function announceSoon(message: string): void {
  setTimeout(() => AccessibilityInfo.announceForAccessibility(message), ANNOUNCE_DELAY_MS);
}

export function AppLockGate({ children }: { children: React.ReactNode }) {
  const { t } = useTranslation();
  const styles = useThemedStyles(makeStyles);
  // null = the pref read hasn't answered yet: keep the (in-tree) cover up —
  // the alternative is one frame of calendar before a lock that claims the
  // app was never open.
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [locked, setLocked] = useState(true);
  const [covered, setCovered] = useState(false);
  // Remount key for the cover Modal: bumped on every lock engagement so a
  // native present that UIKit silently dropped (visible flipped while another
  // modal was mid-dismissal) is retried with a fresh host instead of leaving
  // a "locked" app uncovered.
  const [lockEpoch, setLockEpoch] = useState(0);
  const enabledRef = useRef<boolean | null>(null);
  const lockedRef = useRef(true);
  const backgroundAt = useRef<number | null>(null);
  /** Whether THIS lock engagement already showed the OS sheet once. */
  const promptedThisLock = useRef(false);

  const engageLock = useCallback(() => {
    lockedRef.current = true;
    promptedThisLock.current = false;
    setLocked(true);
    setLockEpoch((epoch) => epoch + 1);
    // Nothing may stay attached to the now-hidden content — a soft keyboard
    // over the cover would both look broken and type into a hidden input.
    Keyboard.dismiss();
    AccessibilityInfo.announceForAccessibility(i18n.t('mobile.appLock.lockedTitle'));
  }, []);

  const runUnlock = useCallback(async () => {
    if (!lockedRef.current || isAuthenticating()) return;
    promptedThisLock.current = true;
    // The device's screen lock can disappear while our lock is on (code
    // removed in the OS settings). The OS prompt would only report an error
    // the user cannot fix from inside a locked app — fail OPEN, switch the
    // pref off (the subscription below updates our state), and say so.
    // Only a DEFINITE "nothing enrolled" fails open: a transient check error
    // must not permanently disable the lock, so 'unknown' just proceeds to
    // the prompt (which then succeeds or parks on the cover).
    if ((await deviceEnrollment()) === 'none') {
      await disableAfterLostEnrollment();
      lockedRef.current = false;
      setLocked(false);
      announceSoon(i18n.t('mobile.appLock.disabledLostEnrollment'));
      return;
    }
    if ((await authenticate()) === 'success') {
      lockedRef.current = false;
      backgroundAt.current = null;
      setLocked(false);
      return;
    }
    // Cancelled / failed: park on the cover. Announce it (deferred past the
    // sheet's dismissal chatter) — the cover's button is the only way
    // forward, and the sheet's own closing says nothing about that.
    announceSoon(i18n.t('mobile.appLock.retryHint'));
  }, []);

  // Cold start: read the pref, then either open up or lock. The auto-prompt
  // only fires when the app is actually IN FRONT — the OS background-sync
  // task relaunches a terminated app into the background, where the sheet
  // cannot show; burning the once-per-lock prompt there would demote every
  // real first open to a silent cover. Left un-prompted, the AppState
  // listener below shows the sheet on the first genuine foreground.
  useEffect(() => {
    let cancelled = false;
    void readAppLockEnabled().then((on) => {
      if (cancelled) return;
      enabledRef.current = on;
      setEnabled(on);
      if (on) {
        if (AppState.currentState === 'active') void runUnlock();
      } else {
        lockedRef.current = false;
        setLocked(false);
      }
    });
    return () => {
      cancelled = true;
    };
    // runUnlock reads everything through refs/i18n and never changes.
  }, [runUnlock]);

  // Pref changes apply LIVE (the Settings toggle, the fail-open disable) —
  // enabling arms the next lock point without locking the person who just
  // authenticated to flip the switch; disabling releases a (theoretical)
  // standing lock.
  useEffect(
    () =>
      subscribeAppLockEnabled((on) => {
        enabledRef.current = on;
        setEnabled(on);
        if (!on) {
          lockedRef.current = false;
          setLocked(false);
        }
      }),
    [],
  );

  useEffect(() => {
    const sub = AppState.addEventListener('change', (state) => {
      if (enabledRef.current !== true) return;
      if (state === 'background') {
        // Android has no 'inactive': our OWN auth sheet (the device-code
        // fallback is a separate activity) backgrounds the app. Covering is
        // right either way, but the re-lock clock must not start under the
        // sheet — a slow PIN entry would re-lock the app the moment the
        // successful unlock returns.
        if (!isAuthenticating()) {
          backgroundAt.current ??= Date.now();
          // A genuine departure while parked on the cover re-arms the
          // auto-prompt for the return (hours later, the user should get
          // the sheet, not a button hunt). Sheet dismissals never come
          // through here — they end in 'active'.
          if (lockedRef.current) promptedThisLock.current = false;
        }
        setCovered(true);
        return;
      }
      if (state === 'inactive') {
        // Cover on inactive too (the iOS switcher snapshot is taken here) —
        // but not while the OS auth sheet is up: it flips the app inactive
        // itself, and covering under it would flash for nothing.
        if (!isAuthenticating()) setCovered(true);
        return;
      }
      // active
      const away = backgroundAt.current;
      backgroundAt.current = null;
      setCovered(false);
      if (
        !lockedRef.current &&
        away != null &&
        Date.now() - away >= RELOCK_AFTER_MS
      ) {
        engageLock();
      }
      // Auto-prompt only the FIRST time a lock engagement reaches the
      // foreground — a cancelled sheet also ends in 'active', and prompting
      // again there would loop the sheet forever (the Unlock button exists
      // for exactly that parked state).
      if (lockedRef.current && !promptedThisLock.current && !isAuthenticating()) {
        void runUnlock();
      }
    });
    return () => sub.remove();
  }, [engageLock, runUnlock]);

  const contextLocked = enabled !== false && locked;
  const coverShown = enabled !== false && (locked || covered);
  const showUnlock = enabled === true && locked;

  // Mirror the cover's visibility for the non-React layers (the window-level
  // VoiceOver gesture host refuses gestures while it is up).
  useEffect(() => {
    setAppLockCoverVisible(coverShown);
    return () => setAppLockCoverVisible(false);
  }, [coverShown]);

  const cover = (
    <View style={styles.cover}>
      {/* The "locked" claim only when the lock really holds — while merely
          covered (app switcher / inactive) or before the pref read answered,
          a neutral screen without controls. */}
      <Text style={styles.title} accessibilityRole="header">
        {showUnlock ? t('mobile.appLock.lockedTitle') : 'Aperio'}
      </Text>
      {showUnlock && (
        <Pressable
          accessibilityRole="button"
          accessibilityLabel={t('mobile.appLock.unlockButton')}
          onPress={() => void runUnlock()}
          style={({ pressed }) => [styles.button, pressed && styles.pressed]}
        >
          <Text style={styles.buttonText} importantForAccessibility="no">
            {t('mobile.appLock.unlockButton')}
          </Text>
        </Pressable>
      )}
    </View>
  );

  return (
    <AppLockLockedContext.Provider value={contextLocked}>
      <View
        style={styles.content}
        accessibilityElementsHidden={coverShown}
        importantForAccessibility={coverShown ? 'no-hide-descendants' : 'auto'}
      >
        {children}
      </View>
      {/* Pre-pref-read: an in-tree cover. No dialog can be open that early,
          and presenting+dismissing a native Modal on every cold start would
          flash and churn screen-reader focus for the default-off majority. */}
      {enabled === null && <View style={[styles.inTreeCover, styles.cover]}>{null}</View>}
      <Modal
        key={lockEpoch}
        visible={enabled === true && (locked || covered)}
        animationType="none"
        statusBarTranslucent
        onRequestClose={() => {
          // Android back button: a locked app stays locked.
        }}
      >
        {cover}
      </Modal>
    </AppLockLockedContext.Provider>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    content: { flex: 1 },
    inTreeCover: {
      position: 'absolute',
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
    },
    cover: {
      flex: 1,
      backgroundColor: c.background,
      alignItems: 'center',
      justifyContent: 'center',
      gap: 24,
      padding: 32,
    },
    title: { fontSize: 22, fontWeight: '700', color: c.textLabel },
    button: {
      backgroundColor: c.accent,
      borderRadius: 12,
      paddingHorizontal: 32,
      paddingVertical: 14,
    },
    pressed: { opacity: 0.7 },
    buttonText: { fontSize: 17, fontWeight: '600', color: c.textOnAccent },
  });
