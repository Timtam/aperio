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

    /// `{available, enabled, canSchedule, previews, build}`.
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
}
