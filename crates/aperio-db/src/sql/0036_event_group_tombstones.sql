-- Migration 0036: remembering that a group was dissolved.
--
-- Deleting the row is not enough. The applier never re-applies a device's OWN
-- envelopes, so the two sides of a dissolve see different halves of it: the
-- device that dissolved has already done so and will never hear its own event
-- again, while an UPDATE that another device wrote before it heard about the
-- dissolve still arrives afterwards. With no row left to compare against, that
-- older update simply re-creates the group — on that device only. The others
-- applied the dissolve last and show nothing. Two devices, two answers, and no
-- event left in the log that could settle it.
--
-- So a dissolve leaves a mark that outlives the row: the id and WHEN it was
-- dissolved. An arriving group older than its own tombstone is refused; a
-- genuinely newer one (the user grouped those events again) still lands,
-- because the comparison is on time, not on existence.
--
-- Rows are tiny and never revisited, and a group id is a UUID that is never
-- reused, so they are kept rather than expired: an expiry window would be one
-- more thing to get wrong, and getting it wrong resurrects a group.
CREATE TABLE event_group_tombstones (
    group_id      TEXT NOT NULL PRIMARY KEY,
    dissolved_at  TEXT NOT NULL
);
