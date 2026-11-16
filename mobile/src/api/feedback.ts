// Mobile feedback api-client: the phone side of host_core::feedback, the twin
// of the desktop's feedback commands. The phone adds what only it knows (store
// version, build number, system version, device model) as `HostDetails`; the
// core composes the line and does the rest.

import { Platform } from 'react-native';

import type {
  FeedbackDraft,
  FeedbackInfo,
  FeedbackOutcome,
  HostDetails,
} from '@aperio/shared';

import CalFfi from '../../modules/cal-ffi';

/** What the phone knows about itself that the core cannot read. */
export function hostDetails(): HostDetails {
  let version = '';
  let build = '';
  try {
    ({ version, build } = CalFfi.appVersion());
  } catch {
    // A native module from before `appVersion`: the line goes without them.
  }
  const android = Platform.OS === 'android' ? Platform.constants : null;
  return {
    app_version: version || null,
    build: build || null,
    // Android's `Platform.Version` is the API level; `Release` is the number
    // people know ("15").
    os_version: android ? android.Release || null : String(Platform.Version),
    device: android ? android.Model || null : null,
  };
}

/** Whether this build can send feedback, and the system line that goes along. */
export const feedbackInfo = async (): Promise<FeedbackInfo> =>
  JSON.parse(await CalFfi.feedbackInfoJson(JSON.stringify(hostDetails()))) as FeedbackInfo;

/** The redacted log text a message would carry, for the form to show. */
export const feedbackLogs = (): Promise<string> => CalFfi.feedbackLogs();

/** Send one message. Every ending is an outcome, never a thrown error. */
export const sendFeedback = async (draft: FeedbackDraft): Promise<FeedbackOutcome> =>
  JSON.parse(
    await CalFfi.sendFeedbackJson(JSON.stringify(draft), JSON.stringify(hostDetails())),
  ) as FeedbackOutcome;
