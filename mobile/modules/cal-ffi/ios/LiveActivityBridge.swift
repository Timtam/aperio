import ActivityKit
import Foundation
import UIKit
import UserNotifications

// Carries out what `shared/liveActivityIos.ts` decides (DESIGN-live-activities
// .md, "iOS"): list, start, schedule, change and end Aperio's Live Activities.
// It decides nothing itself.
//
// iOS 17 is the floor: the widget extension that draws an activity needs it,
// and an activity started on iOS 16 would be one nothing can show. Scheduling
// ahead needs iOS 26.

enum LiveActivityBridgeError: Error, CustomStringConvertible {
    case unavailable(String)
    case badContent(String)
    case notFound(String)

    var description: String {
        switch self {
        case let .unavailable(what): return "live activities unavailable: \(what)"
        case let .badContent(what): return "live activity content unreadable: \(what)"
        case let .notFound(id): return "no live activity \(id)"
        }
    }
}

enum LiveActivityBridge {
    private static func json(_ value: Any) -> String {
        guard let data = try? JSONSerialization.data(withJSONObject: value),
              let text = String(data: data, encoding: .utf8)
        else { return "null" }
        return text
    }

    static func instant(_ raw: String) -> Date? {
        let fractional = ISO8601DateFormatter()
        fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = fractional.date(from: raw) { return date }
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        return plain.date(from: raw)
    }

    /// `{available, enabled, canSchedule, previews, build, endedByWake}`.
    ///
    /// Not available on an iPad, which has no Live Activities: a switch there
    /// would send the user to look for a setting that does not exist.
    /// `previews` is the notification preview setting (`always`,
    /// `whenUnlocked`, `never`), which the activity follows on the lock screen.
    static func supportJson() async -> String {
        let idiom = await MainActor.run { UIDevice.current.userInterfaceIdiom }
        var available = false
        var enabled = false
        var canSchedule = false
        if #available(iOS 17.0, *), idiom == .phone {
            available = true
            enabled = ActivityAuthorizationInfo().areActivitiesEnabled
            if #available(iOS 26.0, *) {
                canSchedule = true
            }
        }
        let settings = await UNUserNotificationCenter.current().notificationSettings()
        let previews: String
        switch settings.showPreviewsSetting {
        case .always: previews = "always"
        case .whenAuthenticated: previews = "whenUnlocked"
        case .never: previews = "never"
        @unknown default: previews = "always"
        }
        // The app's build, so a change since the last pass reads as an update
        // of the app and not as a swipe (`reconcileIosActivities`).
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""
        return json([
            "available": available,
            "enabled": enabled,
            "canSchedule": canSchedule,
            "previews": previews,
            "build": build,
            // Gone because the wake ended them, not because they were swiped.
            "endedByWake": EndedByWake.recent(),
        ])
    }

    /// `[{id, chainKey, state}]`. The state is read by its name: `.pending`
    /// exists only from iOS 26, and a switch over it would not build below.
    static func listJson() -> String {
        guard #available(iOS 17.0, *) else { return "[]" }
        let rows: [[String: Any]] = Activity<AperioEventAttributes>.activities.map { activity in
            [
                "id": activity.id,
                "chainKey": activity.attributes.chainKey,
                "state": String(describing: activity.activityState),
            ]
        }
        return json(rows)
    }

    @available(iOS 17.0, *)
    private static func content(
        _ contentJson: String
    ) throws -> ActivityContent<AperioEventAttributes.ContentState> {
        guard let data = contentJson.data(using: .utf8) else {
            throw LiveActivityBridgeError.badContent("not UTF-8")
        }
        let state: AperioEventAttributes.ContentState
        do {
            state = try JSONDecoder().decode(AperioEventAttributes.ContentState.self, from: data)
        } catch {
            throw LiveActivityBridgeError.badContent("\(error)")
        }
        return ActivityContent(
            state: state,
            staleDate: instant(state.staleAt),
            relevanceScore: state.relevance
        )
    }

    /// Start now, without an alert: the reminder has rung already.
    static func start(chainKey: String, contentJson: String) throws -> String {
        guard #available(iOS 17.0, *) else {
            throw LiveActivityBridgeError.unavailable("needs iOS 17")
        }
        let activityContent = try content(contentJson)
        let activity = try Activity<AperioEventAttributes>.request(
            attributes: AperioEventAttributes(chainKey: chainKey),
            content: activityContent,
            pushType: nil
        )
        return activity.id
    }

    /// Schedule ahead (iOS 26): iOS starts the activity at `startAt`, with an
    /// alert, also while Aperio is in the background.
    ///
    /// `startDate:` rather than `start:`: Apple renamed the label after the
    /// first iOS 26 SDK and back-dated both to 26.0. The old name builds on
    /// every iOS 26 SDK, the new one only on the later ones.
    static func schedule(
        chainKey: String,
        contentJson: String,
        startAt: String,
        alertTitle: String,
        alertBody: String
    ) throws -> String {
        guard #available(iOS 26.0, *) else {
            throw LiveActivityBridgeError.unavailable("scheduling needs iOS 26")
        }
        guard let date = instant(startAt) else {
            throw LiveActivityBridgeError.badContent("start \(startAt)")
        }
        let alert = AlertConfiguration(
            title: LocalizedStringResource(stringLiteral: alertTitle),
            body: LocalizedStringResource(stringLiteral: alertBody),
            sound: .default
        )
        let activityContent = try content(contentJson)
        let activity = try Activity<AperioEventAttributes>.request(
            attributes: AperioEventAttributes(chainKey: chainKey),
            content: activityContent,
            pushType: nil,
            style: .standard,
            alertConfiguration: alert,
            startDate: date
        )
        return activity.id
    }

    static func update(id: String, contentJson: String) async throws {
        guard #available(iOS 17.0, *) else {
            throw LiveActivityBridgeError.unavailable("needs iOS 17")
        }
        guard let activity = Activity<AperioEventAttributes>.activities.first(where: { $0.id == id })
        else { throw LiveActivityBridgeError.notFound(id) }
        let activityContent = try content(contentJson)
        await activity.update(activityContent)
    }

    /// End at once: an appointment that is over, or one that no longer is.
    static func end(id: String) async {
        guard #available(iOS 17.0, *) else { return }
        guard let activity = Activity<AperioEventAttributes>.activities.first(where: { $0.id == id })
        else { return }
        await activity.end(nil, dismissalPolicy: .immediate)
    }

    /// End now with this final content, and let iOS remove it at `at`, at
    /// the latest four hours on (`ActivityUIDismissalPolicy.after`). It leaves
    /// the Dynamic Island at once and can no longer change: the trial
    /// `endEarly` in `shared/liveActivityIos.ts`.
    static func endAfter(id: String, contentJson: String, at: String) async throws {
        guard #available(iOS 17.0, *) else {
            throw LiveActivityBridgeError.unavailable("needs iOS 17")
        }
        guard let date = instant(at) else {
            throw LiveActivityBridgeError.badContent("end \(at)")
        }
        guard let activity = Activity<AperioEventAttributes>.activities.first(where: { $0.id == id })
        else { throw LiveActivityBridgeError.notFound(id) }
        let activityContent = try content(contentJson)
        await activity.end(activityContent, dismissalPolicy: .after(date))
    }

    /// End every running or waiting activity whose end, as the last pass
    /// handed it over (`ActivityEnds`), has passed. Returns how many.
    ///
    /// This is no rule of its own: the pass decided when each activity ends,
    /// and this only notices that the moment has come. It runs first thing
    /// when iOS wakes Aperio, before the round's JS, the network or the plan,
    /// because a wake may last seconds.
    ///
    /// The overdue ends are taken out of the store in one step, so a launch
    /// and a wake that run at once do not both end, and count, the same one.
    static func endOverdue(now: Date = Date()) async -> Int {
        guard #available(iOS 17.0, *) else { return 0 }
        let due = ActivityEnds.takeOverdue(now: now)
        guard !due.isEmpty else { return 0 }
        var ended: [String] = []
        for activity in Activity<AperioEventAttributes>.activities where due.contains(activity.id) {
            // By name, as in `listJson`: `.pending` exists only from iOS 26.
            let state = String(describing: activity.activityState)
            if state == "ended" || state == "dismissed" { continue }
            await activity.end(nil, dismissalPolicy: .immediate)
            ended.append(activity.id)
        }
        EndedByWake.add(ended, at: now)
        return ended.count
    }
}

/// Each activity's end as the last Live Activity pass handed it over
/// (`activityEnds` in `shared/liveActivityIos.ts`), kept across launches: a
/// launch into the background reaches the wake-up handler before any JS runs.
enum ActivityEnds {
    private static let key = "aperio.liveActivities.ends.v1"
    private static let lock = NSLock()

    /// Replace what is kept with `json`, an object `{id: RFC 3339}`. Anything
    /// unreadable keeps nothing, which ends nothing early.
    static func save(json: String) {
        var kept: [String: String] = [:]
        if let data = json.data(using: .utf8),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: String] {
            kept = object.filter { LiveActivityBridge.instant($0.value) != nil }
        }
        lock.lock()
        defer { lock.unlock() }
        UserDefaults.standard.set(kept, forKey: key)
    }

    static func load() -> [String: Date] {
        lock.lock()
        let raw = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        lock.unlock()
        return raw.compactMapValues { LiveActivityBridge.instant($0) }
    }

    /// Remove and return the ids whose end is `now` or earlier, in one step.
    static func takeOverdue(now: Date) -> Set<String> {
        lock.lock()
        defer { lock.unlock() }
        let raw = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        var due = Set<String>()
        var kept: [String: String] = [:]
        for (id, value) in raw {
            if let end = LiveActivityBridge.instant(value), end <= now {
                due.insert(id)
            } else {
                kept[id] = value
            }
        }
        if !due.isEmpty {
            UserDefaults.standard.set(kept, forKey: key)
        }
        return due
    }

    /// The earliest end still to come, or nil.
    static func earliest(after now: Date = Date()) -> Date? {
        load().values.filter { $0 > now }.min()
    }
}

/// The activities the wake ended at their end, for a day: the next pass reads
/// them (`supportJson`, `endedByWake`) so that one gone this way is not taken
/// for a swipe. An activity merely found gone after its end may still have
/// been swiped before it.
enum EndedByWake {
    private static let key = "aperio.liveActivities.endedByWake.v1"
    private static let lock = NSLock()
    private static let keptFor: TimeInterval = 24 * 60 * 60

    static func add(_ ids: [String], at now: Date) {
        guard !ids.isEmpty else { return }
        lock.lock()
        defer { lock.unlock() }
        var raw = fresh(now: now)
        let stamp = ISO8601DateFormatter().string(from: now)
        for id in ids {
            raw[id] = stamp
        }
        UserDefaults.standard.set(raw, forKey: key)
    }

    static func recent(now: Date = Date()) -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return Array(fresh(now: now).keys)
    }

    /// The entries of the last day; the caller holds the lock.
    private static func fresh(now: Date) -> [String: String] {
        let raw = UserDefaults.standard.dictionary(forKey: key) as? [String: String] ?? [:]
        return raw.filter { _, stamp in
            guard let at = LiveActivityBridge.instant(stamp) else { return false }
            return now.timeIntervalSince(at) < keptFor
        }
    }
}
