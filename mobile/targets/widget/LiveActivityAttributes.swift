import ActivityKit
import Foundation

// The shape of an Aperio Live Activity (DESIGN-live-activities.md).
//
// This file exists twice, byte for byte: in the app's native module
// (mobile/modules/cal-ffi/ios), which starts and changes activities, and in the
// widget extension (mobile/targets/widget), which draws them. The two targets
// share no source, and ActivityKit pairs them by type name and Codable shape:
// a difference starts activities nothing can draw. src/intl/liveActivityWire
// .test.ts keeps the copies identical and in step with what
// shared/liveActivityIos.ts writes.

struct AperioEventAttributes: ActivityAttributes {
    /// One appointment as the activity shows it. Times are RFC 3339 instants.
    struct Shown: Codable, Hashable {
        var key: String
        var eventId: String
        var calendarId: String
        var start: String
        var end: String
        var title: String
        var location: String?
        var followUpTitle: String?
        var followUpStart: String?
    }

    /// The words that are not data, in the language Aperio runs in.
    struct Words: Codable, Hashable {
        var begins: String
        var ends: String
        /// A template with `{title}` and `{time}`.
        var followUp: String
        var over: String
    }

    struct ContentState: Codable, Hashable {
        /// BCP-47 tag of Aperio's language; the region comes from the phone.
        var locale: String
        var words: Words
        var current: Shown
        /// Shown once the stale date has passed; none shows `current` as over.
        var next: Shown?
        var staleAt: String
        var relevance: Double
        /// The phone shows notification previews only when unlocked (or
        /// never): title, place and follow-up are then marked private.
        var hideOnLockScreen: Bool
    }

    /// The chain's first appointment: what Aperio finds the activity by.
    var chainKey: String
}
