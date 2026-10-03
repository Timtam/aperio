import { useEffect, useState } from 'react';
import { AccessibilityInfo, Linking, PermissionsAndroid, Platform } from 'react-native';
import AsyncStorage from '@react-native-async-storage/async-storage';

import {
  answerOutcome,
  gainedAccess,
  grantedOutcome,
  missingStores,
  type AskFor,
  type AskOutcome,
  type DeviceAccessPair,
  type DeviceStores,
  type OsAccess,
  type OsAccessReport,
} from '@aperio/shared';

import i18n from '../../i18n';
import {
  deviceCalendarAccess,
  noteDeviceCalendarAsked,
  requestDeviceCalendarAccess,
} from '../api/accounts';
import { refreshExternalCache } from '../api/sync';
import { holdingSpeech } from '../a11y/speechHold';
import {
  isAppLockEngaged,
  isOsSheetOpen,
  whenAppLockReleased,
  whileOsSheetOpen,
} from './appLock';
import { settleExternalCaches } from './cacheSettle';

/**
 * The device's own calendars and reminders: asking the OS at start, the
 * account's "Allow access…" repair, and noticing a grant given in the OS
 * settings.
 *
 * At start, ask the OS when it has never asked on this phone and a "this
 * device" account exists (decision 166). A phone Aperio was moved to brings
 * the account and its cache along, but not the grant: iOS never asked there,
 * every read of the account failed, and its cache froze. Whether to ask is the
 * core's rule (`device_calendar_access_json` carries its answer); this runs the
 * prompt, reads the answer back, reloads the account when something was
 * granted, and says one sentence about it.
 *
 * Android cannot say whether it ever asked; Aperio notes every request it
 * makes on the device, outside Auto Backup, so the start check asks there
 * once per device, and again on a new phone (decision 172).
 *
 * The start check runs once per process. An answer the user already gave is
 * theirs, and iOS would not show the prompt again anyway: after that, only the
 * user's own "Allow access…" asks, or leads to the OS settings.
 */

/** How long the day-start review waits for the check at most. Long enough to
 *  answer an alert, reload and hear the sentence; a check that never settles
 *  must not cost the morning review. */
const SETTLE_CAP_MS = 120_000;
/** After an OS alert the system's own screen-change speech comes first, and a
 *  sentence spoken at once is cut off (the AppLockGate pattern). */
const ANNOUNCE_DELAY_MS = 400;
/** Past this, stop waiting for VoiceOver to report the sentence finished. The
 *  longest (a partial grant with the other store's way back) takes a few
 *  seconds, but the clock also runs while speech queued ahead of it is said,
 *  such as the focus read-out after an OS alert. */
const SPEAK_CAP_MS = 20_000;

let started = false;
let startSettled = false;
let settle: () => void = () => {};
const settled = new Promise<void>((resolve) => {
  settle = resolve;
});

/** Flows that ask, reload and speak: the start check and the account's
 *  repair. The foreground check stays out of them, so one grant is not
 *  reloaded and said twice. */
let flowBusy = 0;
/** The user left for the OS settings from the repair dialog; the return says
 *  what came of it, even when nothing did. */
let settingsTrip = false;
/** The app came to the front under the lock; the check runs after unlock. */
let foregroundPending = false;

/** A reload for a `restorable` report was kicked; until a read shows the
 *  cache no longer withholds the store, the same grant is not new. */
let restoreKicked = false;

let lastReport: OsAccessReport | null = null;
const reportListeners = new Set<(report: OsAccessReport) => void>();

/** The access as the last run of Aperio saw it, on this device only. A
 *  grant given in the OS settings may end Aperio (iOS does that when its
 *  access changes); the next start then compares with this. The warm pass of
 *  that start may already have read the account again and cleared the
 *  cache's `restorable`, but not this. */
const SEEN_KEY = 'aperio.deviceAccess.lastSeen';
const seenAtStart: Promise<DeviceAccessPair | null> = AsyncStorage.getItem(SEEN_KEY)
  .then((raw) => (raw ? (JSON.parse(raw) as DeviceAccessPair) : null))
  .catch(() => null);
let seenWritten: string | null = null;

function rememberSeen(report: OsAccessReport): void {
  const seen = JSON.stringify({ calendar: report.calendar, tasks: report.tasks });
  if (seen === seenWritten) return;
  seenWritten = seen;
  // After the start's own read of the old value.
  void seenAtStart.then(() => AsyncStorage.setItem(SEEN_KEY, seen)).catch(() => {});
}

/** Read the OS's answer, remember it, and hand it to whoever shows it. */
async function readDeviceAccess(): Promise<OsAccessReport> {
  const report = await deviceCalendarAccess();
  lastReport = report;
  if (!report.restorable) restoreKicked = false;
  rememberSeen(report);
  reportListeners.forEach((cb) => cb(report));
  return report;
}

/** Read the access again after the screen changed it itself (adding the
 *  account), so the next look does not take that grant for a new one. */
export async function refreshDeviceAccessReport(): Promise<void> {
  await readDeviceAccess().catch(() => {});
}

/** The latest access report, read again when a screen using it mounts and
 *  whenever a flow here reads it. `null` until the first read. */
export function useDeviceAccessReport(): OsAccessReport | null {
  const [report, setReport] = useState(lastReport);
  useEffect(() => {
    reportListeners.add(setReport);
    void readDeviceAccess().catch(() => {});
    return () => {
      reportListeners.delete(setReport);
    };
  }, []);
  return report;
}

/** Resolves once the start check has finished — asked, reloaded and said, or
 *  found nothing to ask — or after a cap, whichever comes first. The day-start
 *  review waits for it, so it neither opens under the alert, nor judges the
 *  frozen cache a grant is about to replace, nor cuts the sentence off with
 *  its own "checking". */
export function whenDeviceAccessSettled(): Promise<void> {
  return Promise.race([
    settled,
    new Promise<void>((resolve) => setTimeout(resolve, SETTLE_CAP_MS)),
  ]);
}

/**
 * The one sentence about the device stores after `report`. A grant of one
 * store goes on with what holds the other back, as the repair dialog would
 * say it: Aperio's settings, a question still to come, or a policy. A
 * refusal names the stores `asked` about that are still missing (without
 * `asked`, every missing one).
 */
function sentence(
  outcome: AskOutcome,
  report: OsAccessReport,
  name: string,
  asked?: AskFor,
  atStart = false,
): string {
  switch (outcome) {
    case 'granted':
      return i18n.t('mobile.deviceAccess.granted', { name });
    case 'calendarsOnly':
      return `${i18n.t('mobile.deviceAccess.calendarsOnly', { name })} ${leftSentence('reminders', report.tasks)}`;
    case 'remindersOnly':
      return `${i18n.t('mobile.deviceAccess.remindersOnly', { name })} ${leftSentence('calendars', report.calendar)}`;
    case 'denied': {
      // Android asks again on the next "Allow access…", and has no
      // reminders to name.
      // At start the user never chose "Allow access…": say where it is.
      if (Platform.OS === 'android') {
        return atStart
          ? i18n.t('mobile.deviceAccess.deniedAndroidStart', { name })
          : i18n.t('mobile.deviceAccess.deniedAndroid');
      }
      const refused = asked != null ? askedAndMissing(asked, report) : 'none';
      const what = refused !== 'none' ? refused : missingStores(report);
      return i18n.t('mobile.deviceAccess.denied', { what: storesPhrase(what) });
    }
  }
}

/** What holds back the store a partial grant left out, in one clause: the
 *  dialog explains a restriction at length, a spoken sentence only names it. */
function leftSentence(store: 'calendars' | 'reminders', access: OsAccess | null): string {
  if (access === 'restricted') {
    return store === 'calendars'
      ? i18n.t('mobile.deviceAccess.leftRestrictedCalendars')
      : i18n.t('mobile.deviceAccess.leftRestrictedReminders');
  }
  if (access === 'not_asked') {
    return store === 'calendars'
      ? i18n.t('mobile.deviceAccess.leftAskCalendars')
      : i18n.t('mobile.deviceAccess.leftAskReminders');
  }
  return store === 'calendars'
    ? i18n.t('mobile.deviceAccess.leftSettingsCalendars')
    : i18n.t('mobile.deviceAccess.leftSettingsReminders');
}

/** The stores a prompt asked about that are still not readable. */
function askedAndMissing(asked: AskFor, report: OsAccessReport): DeviceStores {
  const calendars = asked.events && report.calendar !== 'full';
  const reminders = asked.reminders && report.tasks != null && report.tasks !== 'full';
  if (calendars && reminders) return 'both';
  if (calendars) return 'calendars';
  if (reminders) return 'reminders';
  return 'none';
}

/**
 * Say `message` and resolve once VoiceOver has finished it (iOS reports that),
 * or after a cap. Queued, so it follows whatever is being said rather than
 * cutting it off.
 *
 * Without a screen reader, and on Android, it resolves once the sentence is
 * handed over: only iOS reports a finished announcement, and TalkBack queues
 * announcements itself. Waiting there for a report that never comes only
 * held the flow busy for the whole cap, and with it the "Allow access…" the
 * sentence had just suggested pressing again.
 */
async function sayAndWait(message: string): Promise<void> {
  // Not under the unlock prompt: a slow answer to Android's permission
  // dialog may have re-locked the app meanwhile.
  await whenAppLockReleased();
  const screenReader = await AccessibilityInfo.isScreenReaderEnabled().catch(() => false);
  await new Promise((resolve) => setTimeout(resolve, ANNOUNCE_DELAY_MS));
  if (!screenReader || Platform.OS !== 'ios') {
    AccessibilityInfo.announceForAccessibility(message);
    return;
  }
  await new Promise<void>((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      clearTimeout(cap);
      subscription?.remove();
      resolve();
    };
    const subscription = AccessibilityInfo.addEventListener('announcementFinished', (event) => {
      if (event.announcement === message) finish();
    });
    const cap = setTimeout(finish, SPEAK_CAP_MS);
    AccessibilityInfo.announceForAccessibilityWithOptions(message, { queue: true });
  });
}

/**
 * Reload the device accounts when something became readable, and say what
 * came of it. With `settle`, also wait for the reload: the day-start review
 * reads the account next.
 */
async function reloadAndSay(
  outcome: AskOutcome,
  report: OsAccessReport,
  name: string,
  settle: boolean,
  asked?: AskFor,
  atStart = false,
): Promise<void> {
  const reload = outcome !== 'denied';
  // Kick the reload first, so "… wird aktualisiert" is true while it is
  // said; the refresh cues queue behind the sentence instead of cutting it.
  if (reload) {
    restoreKicked = true;
    await refreshExternalCache().catch(() => {});
  }
  await holdingSpeech(() => sayAndWait(sentence(outcome, report, name, asked, atStart)));
  if (reload && settle) await settleExternalCaches(async () => {});
}

/** Run the start check once. Later calls wait for the first. */
export async function runDeviceAccessStartCheck(): Promise<void> {
  if (started) return settled;
  started = true;
  flowBusy += 1;
  try {
    const before = await readDeviceAccess();
    const name = before.account_names[0] ?? '';
    const ask = before.ask_now;
    if (ask == null) {
      // A grant given in the OS settings while Aperio was not running: iOS
      // may end Aperio when its access changes there, so this start is the
      // first to see it. The cache may still withhold the account, or this
      // start's warm pass has already read it; the last run's look says it
      // either way.
      const seen = await seenAtStart;
      const gained = seen != null && gainedAccess(seen, before);
      if (before.account_names.length > 0 && (before.restorable || gained)) {
        await reloadAndSay(grantedOutcome(before), before, name, true);
      }
      return;
    }
    // The alert flips the app inactive; the lock must not cover the app or
    // start its own prompt over it. Android asks through its runtime
    // permission dialog, once per device (decision 172).
    // What the prompt answered is read back from the OS below, not from here.
    await whileOsSheetOpen<unknown>(() =>
      Platform.OS === 'android'
        ? requestAndroidCalendarPermission()
        : requestDeviceCalendarAccess(ask.events, ask.reminders),
    ).catch(() => undefined);
    const after = await readDeviceAccess();
    await reloadAndSay(
      answerOutcome(ask, after),
      after,
      after.account_names[0] ?? name,
      true,
      ask,
      true,
    );
  } catch {
    // Nothing was said. The next start asks again, unless the request was
    // already noted (Android, decision 172); the account's "Allow access…"
    // is the way then.
  } finally {
    flowBusy -= 1;
    startSettled = true;
    settle();
  }
}

/**
 * When Aperio comes back to the front: access granted in the OS settings in
 * between reloads the device account and is said; a trip to the settings from
 * the repair dialog that changed nothing is said too, so the user is not left
 * guessing.
 *
 * Only after the start check, outside the other flows, and only while there
 * is something to notice: with full access at the last look and no trip,
 * nothing is read (each read writes a log line). Under the lock it waits for
 * the unlock ([`runPendingDeviceAccessCheck`]).
 */
export async function runDeviceAccessForegroundCheck(): Promise<void> {
  if (!startSettled || flowBusy > 0 || isOsSheetOpen()) return;
  const trip = settingsTrip;
  const before = lastReport;
  const missing =
    before != null &&
    before.account_names.length > 0 &&
    (before.repair !== 'none' || (before.restorable && !restoreKicked));
  if (!trip && !missing) return;
  if (isAppLockEngaged()) {
    foregroundPending = true;
    return;
  }
  foregroundPending = false;
  settingsTrip = false;
  flowBusy += 1;
  try {
    const after = await readDeviceAccess();
    if (after.account_names.length === 0) return;
    const name = after.account_names[0];
    // A reload already kicked for this grant is not news; a store gained
    // since the last look is.
    const restore = after.restorable && !restoreKicked;
    if (restore || (before != null && gainedAccess(before, after))) {
      await reloadAndSay(grantedOutcome(after), after, name, false);
    } else if (trip) {
      await holdingSpeech(() => sayAndWait(stillMissingSentence(after)));
    }
  } catch {
    // The next return to the front looks again.
  } finally {
    flowBusy -= 1;
  }
}

/** "Access to … is still missing", naming the stores Aperio still may not
 *  read. */
function stillMissingSentence(report: OsAccessReport): string {
  return i18n.t('mobile.deviceAccess.stillMissing', { what: storesPhrase(missingStores(report)) });
}

/** "the calendars", "the reminders" or both, for a sentence about them. */
export function storesPhrase(stores: DeviceStores): string {
  switch (stores) {
    case 'reminders':
      return i18n.t('mobile.deviceAccess.storesReminders');
    case 'both':
      return i18n.t('mobile.deviceAccess.storesBoth');
    default:
      return i18n.t('mobile.deviceAccess.storesCalendars');
  }
}

/** Run a foreground check the lock held back. */
export function runPendingDeviceAccessCheck(): void {
  if (foregroundPending) void runDeviceAccessForegroundCheck();
}

/** A return to the front whose check could not run yet: the lock engaged in
 *  the same moment and took the gate's deferred check with it. It runs after
 *  the unlock ([`runPendingDeviceAccessCheck`]). */
export function markDeviceAccessCheckPending(): void {
  foregroundPending = true;
}

/** Android's answer to the calendar permissions: `blocked` when it reports
 *  that it will not show its dialog again ("don't ask again", a second
 *  refusal), so the screen offers its settings. Android reports a dialog
 *  closed without an answer the same way, and nothing tells the two apart;
 *  the settings text therefore does not claim that Android will not ask, and
 *  the next "Allow access…" still asks first. Every request is noted on the
 *  device, so the start check asks only once (decision 172). */
export async function requestAndroidCalendarPermission(): Promise<
  'granted' | 'denied' | 'blocked'
> {
  // Asked on this device, whatever comes of it: the start check does not ask
  // again (decision 172). Noted BEFORE the dialog, so a process ended while it
  // is up (the user left and Android reclaimed the memory) does not ask again
  // at the next start; a request that then shows nothing is harmless, since
  // "Allow access…" asks.
  noteDeviceCalendarAsked();
  const result = await PermissionsAndroid.requestMultiple([
    PermissionsAndroid.PERMISSIONS.READ_CALENDAR,
    PermissionsAndroid.PERMISSIONS.WRITE_CALENDAR,
  ]);
  const answers = [
    result[PermissionsAndroid.PERMISSIONS.READ_CALENDAR],
    result[PermissionsAndroid.PERMISSIONS.WRITE_CALENDAR],
  ];
  if (answers.every((a) => a === PermissionsAndroid.RESULTS.GRANTED)) return 'granted';
  if (answers.some((a) => a === PermissionsAndroid.RESULTS.NEVER_ASK_AGAIN)) return 'blocked';
  return 'denied';
}

/** Where the account's "Allow access…" ended: done (asked, and said what came
 *  of it), or the detour the screen has to offer, with the report it is
 *  about. */
export interface RepairResult {
  step: 'done' | 'settings' | 'restricted';
  report: OsAccessReport;
}

/**
 * The device account's "Allow access…". Asks where the OS still asks, reloads
 * on a grant and says what came of it; otherwise answers which detour the
 * screen offers: the OS settings, or only an explanation where a policy
 * forbids it. Which one is the core's rule (`repair` in the report).
 */
export async function repairDeviceAccess(): Promise<RepairResult> {
  flowBusy += 1;
  try {
    const before = await readDeviceAccess();
    if (before.repair === 'open_settings') return { step: 'settings', report: before };
    if (before.repair === 'restricted') return { step: 'restricted', report: before };
    const name = before.account_names[0] ?? '';
    if (before.repair === 'none') {
      // The badge was older than the access: it came back meanwhile. Never
      // silent, and the account is read again now.
      await reloadAndSay(grantedOutcome(before), before, name, false);
      return { step: 'done', report: before };
    }
    if (Platform.OS === 'android') {
      const answer = await whileOsSheetOpen(requestAndroidCalendarPermission);
      if (answer === 'blocked') {
        // The settings dialog opens after the unlock a slow answer may
        // have brought, not above the lock's cover.
        await whenAppLockReleased();
        return { step: 'settings', report: before };
      }
      const after = await readDeviceAccess();
      await reloadAndSay(grantedOutcome(after), after, name, false);
      return { step: 'done', report: after };
    }
    // Only what the OS would still ask about; a store already answered is
    // not part of the question (and iOS would not show it again).
    const ask: AskFor = before.ask_now ?? {
      events: before.calendar !== 'full',
      reminders: before.tasks != null && before.tasks !== 'full',
    };
    await whileOsSheetOpen(() =>
      requestDeviceCalendarAccess(ask.events, ask.reminders),
    ).catch(() => false);
    const after = await readDeviceAccess();
    await reloadAndSay(answerOutcome(ask, after), after, name, false, ask);
    return { step: 'done', report: after };
  } finally {
    flowBusy -= 1;
  }
}

/** Open Aperio's page in the OS settings, and remember the trip, so the
 *  return says what came of it. */
export async function openDeviceAccessSettings(): Promise<void> {
  settingsTrip = true;
  await Linking.openSettings().catch(() => {
    settingsTrip = false;
  });
}
