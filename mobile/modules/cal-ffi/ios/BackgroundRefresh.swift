import BackgroundTasks
import ExpoModulesCore
import Foundation
import UIKit

// A SECOND background wake-up, of the class iOS actually schedules through the
// day.
//
// `expo-background-task` submits a `BGProcessingTaskRequest`. That is the class
// for long maintenance work — Apple's own words: "a processing task that can
// take minutes to complete" — and the system runs it when the device is idle,
// by preference on a charger, which in practice means overnight. Pick the phone
// up and it is cancelled. For a calendar that is the wrong class entirely: the
// app was being woken once or twice a day, so the widget and the reminders sat
// on data that old.
//
// `BGAppRefreshTask` is the other class: about thirty seconds, but scheduled
// across the day against the user's own usage pattern. Apple names exactly this
// use for it — fetching new content and updating widgets.
//
// So both, side by side, under separate identifiers: the processing task keeps
// doing the heavy overnight round, and this one is the frequent short catch-up.
// One identifier could not do both — a second `submit` for the same identifier
// replaces the pending request, so the two classes would keep cancelling each
// other.
//
// It runs the SAME work: `runTasks` executes every registered TaskManager task
// for this launch reason, which is the app's background-sync task. There is no
// second code path to keep in step. One thing comes before it: Live Activities
// whose end has passed are ended here, natively, before the JS bundle, the
// network or the plan (`LiveActivityBridge.endOverdue`). A wake comes minutes
// late at best and may last seconds; on Toni's iPhone (build 71, 2026-11-17)
// one woke ten minutes after an appointment's end, died eight seconds later on
// hung requests, and the activity said "over" for two more hours.
//
// Everything here fails soft. No task service, no permitted identifier, a
// refused submit — each ends in a log line (Aperio's own, `NativeLog`), and the
// app is exactly as it was before this file existed.

enum BackgroundRefresh {
  /// Must match `BGTaskSchedulerPermittedIdentifiers` in app.json. iOS refuses
  /// to register an identifier the Info.plist does not list.
  static let identifier = "com.aperio.mobile.refresh"

  /// Apple's floor for this class. A smaller request is not an error, it is
  /// simply ignored, and asking for something we cannot have makes the log
  /// harder to read later.
  private static let floorMinutes = 15

  private static let lock = NSLock()
  private static var minutes = floorMinutes
  private static var wanted = false

  /// Start (or re-arm) the refresh task. Called by the JS layer whenever the
  /// background-sync preference is on, which includes every app start.
  static func enable(minutes requested: Int) {
    lock.lock()
    minutes = max(floorMinutes, requested)
    wanted = true
    lock.unlock()
    schedule()
  }

  /// Stop it and drop the pending request, so turning background sync off in
  /// settings takes effect without waiting for the next wake-up.
  static func disable() {
    lock.lock()
    wanted = false
    lock.unlock()
    BGTaskScheduler.shared.cancel(taskRequestWithIdentifier: identifier)
  }

  /// Each Live Activity's end, handed over by every Live Activity pass
  /// (`activityEnds` in `shared/liveActivityIos.ts`) and kept across launches
  /// (`ActivityEnds`). Only a running Aperio can end one — iOS has no
  /// scheduled end without a push server — so the next wake-up is brought
  /// forward to the earliest end when it comes before the interval is up.
  /// Without this an activity said "over" on the lock screen all evening
  /// (Toni's iPhone, 2026-11-10).
  ///
  /// Re-armed only when the earliest end changed: every pass hands them over,
  /// and submitting an unchanged request again would only push the
  /// interval's floor later.
  static func follow(endsJson: String) {
    let before = ActivityEnds.earliest()
    ActivityEnds.save(json: endsJson)
    if ActivityEnds.earliest() != before { schedule() }
  }

  /// Submit the next request. One is pending at a time; the handler re-arms
  /// after every run, because a request is consumed by being run.
  static func schedule() {
    lock.lock()
    let armed = wanted
    let delay = Double(minutes) * 60
    lock.unlock()
    guard armed else { return }
    let end = ActivityEnds.earliest()

    let request = BGAppRefreshTaskRequest(identifier: identifier)
    // A floor, not an appointment: the system decides the actual moment from
    // battery, network and how the app is used. An activity that ends before
    // the interval is up brings it forward; one already past is the round
    // under way's to end.
    var begin = Date(timeIntervalSinceNow: delay)
    if let end = end, end < begin {
      begin = end
    }
    request.earliestBeginDate = begin
    do {
      try BGTaskScheduler.shared.submit(request)
    } catch {
      // `notPermitted` on a build whose Info.plist lacks the identifier or the
      // `fetch` background mode; `unavailable` when the user has turned
      // Background App Refresh off. Neither is worth a warning, but the log
      // says why no wake-up came.
      NSLog("[Aperio] background refresh not scheduled: \(error)")
      NativeLog.write("info", "background refresh not scheduled: \(error)")
    }
  }

  /// End the overdue Live Activities, run the app's registered background
  /// tasks, then re-arm.
  ///
  /// The re-arm happens on every exit path INCLUDING failure and expiry — a
  /// request that is not replaced is a task that never runs again, which would
  /// turn one bad round into a permanent one.
  fileprivate static func run(_ task: BGTask) {
    // The task existing at all proves it was wanted; a launch straight into the
    // background may reach here before the JS layer has called `enable`.
    lock.lock()
    wanted = true
    lock.unlock()

    let round = Round(task)
    NativeLog.write("info", "background refresh: woken")
    task.expirationHandler = {
      round.finish(success: false, how: "ran out of time")
    }
    Task {
      let ended = await LiveActivityBridge.endOverdue()
      if ended > 0 {
        NativeLog.write("info", "background refresh: ended \(ended) live activities that were over")
      }
      guard let service = AperioTaskServiceHelper.sharedTaskService() else {
        round.finish(success: false, how: "found no task service")
        return
      }
      service.runTasks(with: EXTaskLaunchReasonBackgroundTask, userInfo: nil) { _ in
        round.finish(success: true, how: "done")
      }
    }
  }

  /// One run of the task, finished exactly once: by the tasks completing or
  /// by iOS calling time, whichever comes first. Shared between iOS's
  /// expiration handler and the run; `finished` is guarded by its lock.
  private final class Round: @unchecked Sendable {
    private let task: BGTask
    private let began = Date()
    private let lock = NSLock()
    private var finished = false

    init(_ task: BGTask) {
      self.task = task
    }

    func finish(success: Bool, how: String) {
      lock.lock()
      let first = !finished
      finished = true
      lock.unlock()
      guard first else { return }
      let seconds = String(format: "%.1f", Date().timeIntervalSince(began))
      NSLog("[Aperio] background refresh \(how) after \(seconds) s")
      NativeLog.write(success ? "info" : "warn", "background refresh: \(how) after \(seconds) s")
      task.setTaskCompleted(success: success)
      BackgroundRefresh.schedule()
    }
  }
}

/// Registered through `expo-module.config.json`. The handler MUST be installed
/// before `didFinishLaunchingWithOptions` returns — iOS raises otherwise when a
/// task for the identifier is delivered — which is the whole reason this is an
/// app-delegate subscriber rather than something the module sets up lazily.
public class AperioBackgroundRefreshSubscriber: ExpoAppDelegateSubscriber {
  public func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
  ) -> Bool {
    BGTaskScheduler.shared.register(
      forTaskWithIdentifier: BackgroundRefresh.identifier, using: nil
    ) { task in
      BackgroundRefresh.run(task)
    }
    // Any launch, also one into the background for another reason (the
    // overnight processing task among them): what is over is ended at once.
    Task {
      let ended = await LiveActivityBridge.endOverdue()
      if ended > 0 {
        NativeLog.write("info", "launch: ended \(ended) live activities that were over")
      }
    }
    return true
  }
}
