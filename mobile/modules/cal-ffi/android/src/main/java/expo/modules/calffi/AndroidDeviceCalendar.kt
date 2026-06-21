package expo.modules.calffi

import android.Manifest
import android.content.ContentUris
import android.content.ContentValues
import android.content.Context
import android.content.pm.PackageManager
import android.provider.CalendarContract
import androidx.core.content.ContextCompat
import java.time.Instant
import org.json.JSONArray
import org.json.JSONObject
import uniffi.cal_ffi.DeviceCalException
import uniffi.cal_ffi.DeviceEventStoreBridge

/**
 * Android implementation of the Rust [DeviceEventStoreBridge] foreign trait —
 * the platform half of the device-local calendar adapter, over the system
 * CalendarProvider. Calendar-only: Android has no system Reminders app, so
 * [supportsReminders] is `false` and the reminder methods are inert (the Rust
 * adapter then declares only `Capability::Calendar`).
 *
 * It emits the same small intermediate shape the iOS bridge does
 * (`id`/`name`/`read_only`/`color_hex` for calendars; `id`/`calendar_id`/`title`/
 * `start`/`end`/… for events); the Rust adapter maps it onto `cal_core`. Dates
 * cross as RFC-3339 (UTC `Instant`s); CalendarProvider stores epoch millis.
 *
 * The runtime permission REQUEST happens in the RN layer (`PermissionsAndroid`)
 * before the account is added — [requestAccess] only reports whether access is
 * already granted, and the data methods throw [DeviceCalException.PermissionDenied]
 * if it was revoked.
 */
class AndroidDeviceCalendar(private val context: Context) : DeviceEventStoreBridge {

  override fun requestAccess(events: Boolean, reminders: Boolean): Boolean = hasReadPermission()

  // Android has no system reminders app — the adapter stays calendar-only.
  override fun supportsReminders(): Boolean = false

  // ── Calendar reads ──

  override fun listCalendars(): String {
    requireRead()
    val out = JSONArray()
    val projection = arrayOf(
      CalendarContract.Calendars._ID,
      CalendarContract.Calendars.CALENDAR_DISPLAY_NAME,
      CalendarContract.Calendars.CALENDAR_COLOR,
      CalendarContract.Calendars.CALENDAR_ACCESS_LEVEL,
    )
    context.contentResolver.query(
      CalendarContract.Calendars.CONTENT_URI, projection, null, null, null,
    )?.use { cursor ->
      while (cursor.moveToNext()) {
        val obj = JSONObject()
        obj.put("id", cursor.getLong(0).toString())
        obj.put("name", cursor.getString(1) ?: "")
        obj.put(
          "read_only",
          cursor.getInt(3) < CalendarContract.Calendars.CAL_ACCESS_CONTRIBUTOR,
        )
        if (!cursor.isNull(2)) obj.put("color_hex", hexColor(cursor.getInt(2)))
        out.put(obj)
      }
    }
    return out.toString()
  }

  override fun getEvents(calendarId: String, start: String, end: String): String {
    requireRead()
    val startMs = parseInstant(start)
    val endMs = parseInstant(end)
    val builder = CalendarContract.Instances.CONTENT_URI.buildUpon()
    ContentUris.appendId(builder, startMs)
    ContentUris.appendId(builder, endMs)
    val projection = arrayOf(
      CalendarContract.Instances.EVENT_ID,
      CalendarContract.Instances.TITLE,
      CalendarContract.Instances.DESCRIPTION,
      CalendarContract.Instances.EVENT_LOCATION,
      CalendarContract.Instances.BEGIN,
      CalendarContract.Instances.END,
      CalendarContract.Instances.ALL_DAY,
    )
    val out = JSONArray()
    // The Instances table is already recurrence-expanded for the window (the
    // CalendarProvider analogue of EventKit's predicate fetch).
    context.contentResolver.query(
      builder.build(), projection,
      "${CalendarContract.Instances.CALENDAR_ID} = ?", arrayOf(calendarId), null,
    )?.use { cursor ->
      while (cursor.moveToNext()) {
        val eventId = cursor.getLong(0)
        val begin = cursor.getLong(4)
        val obj = JSONObject()
        // Suffix the occurrence start so a recurring series' instances stay
        // distinct cal_core ids (baseEventId strips it back off for writes).
        obj.put("id", "$eventId#${begin / 1000}")
        obj.put("calendar_id", calendarId)
        obj.put("title", cursor.getString(1) ?: "")
        if (!cursor.isNull(2)) obj.put("description", cursor.getString(2))
        if (!cursor.isNull(3)) obj.put("location", cursor.getString(3))
        obj.put("start", iso(begin))
        obj.put("end", iso(cursor.getLong(5)))
        obj.put("all_day", cursor.getInt(6) == 1)
        out.put(obj)
      }
    }
    return out.toString()
  }

  // ── Calendar writes ──

  override fun createEvent(calendarId: String, eventJson: String): String {
    requireWrite()
    val write = JSONObject(eventJson)
    val values = ContentValues()
    values.put(CalendarContract.Events.CALENDAR_ID, calendarId.toLong())
    applyEventValues(write, values)
    val uri = context.contentResolver.insert(CalendarContract.Events.CONTENT_URI, values)
      ?: throw DeviceCalException.Backend("insert event failed")
    return readEventById(ContentUris.parseId(uri), calendarId)
  }

  override fun updateEvent(eventJson: String): String {
    requireWrite()
    val write = JSONObject(eventJson)
    val id = baseEventId(write.optString("id", ""))
      ?: throw DeviceCalException.Backend("event id required for update")
    val calendarId = write.getString("calendar_id")
    val values = ContentValues()
    applyEventValues(write, values)
    context.contentResolver.update(
      ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, id), values, null, null,
    )
    return readEventById(id, calendarId)
  }

  override fun deleteEvent(eventId: String) {
    requireWrite()
    val id = baseEventId(eventId) ?: return // Malformed / already gone — idempotent.
    context.contentResolver.delete(
      ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, id), null, null,
    )
  }

  // ── Reminders: unsupported on Android (no system reminders app) ──

  override fun listReminderLists(): String = "[]"
  override fun getReminders(listId: String): String = "[]"
  override fun createReminder(listId: String, taskJson: String): String =
    throw DeviceCalException.Unavailable()
  override fun updateReminder(taskJson: String): String =
    throw DeviceCalException.Unavailable()
  override fun deleteReminder(taskId: String) {
    throw DeviceCalException.Unavailable()
  }

  // ── Helpers ──

  private fun applyEventValues(write: JSONObject, values: ContentValues) {
    values.put(CalendarContract.Events.TITLE, write.getString("title"))
    if (write.has("description")) {
      values.put(CalendarContract.Events.DESCRIPTION, write.getString("description"))
    }
    if (write.has("location")) {
      values.put(CalendarContract.Events.EVENT_LOCATION, write.getString("location"))
    }
    values.put(CalendarContract.Events.DTSTART, parseInstant(write.getString("start")))
    values.put(CalendarContract.Events.DTEND, parseInstant(write.getString("end")))
    values.put(CalendarContract.Events.ALL_DAY, if (write.optBoolean("all_day", false)) 1 else 0)
    // Required for inserts. The cal_core instants are UTC, so anchor the row to UTC.
    values.put(CalendarContract.Events.EVENT_TIMEZONE, "UTC")
  }

  private fun readEventById(id: Long, calendarId: String): String {
    val projection = arrayOf(
      CalendarContract.Events.TITLE,
      CalendarContract.Events.DESCRIPTION,
      CalendarContract.Events.EVENT_LOCATION,
      CalendarContract.Events.DTSTART,
      CalendarContract.Events.DTEND,
      CalendarContract.Events.ALL_DAY,
    )
    context.contentResolver.query(
      ContentUris.withAppendedId(CalendarContract.Events.CONTENT_URI, id),
      projection, null, null, null,
    )?.use { cursor ->
      if (cursor.moveToFirst()) {
        val dtStart = cursor.getLong(3)
        val obj = JSONObject()
        obj.put("id", "$id#${dtStart / 1000}")
        obj.put("calendar_id", calendarId)
        obj.put("title", cursor.getString(0) ?: "")
        if (!cursor.isNull(1)) obj.put("description", cursor.getString(1))
        if (!cursor.isNull(2)) obj.put("location", cursor.getString(2))
        obj.put("start", iso(dtStart))
        obj.put("end", iso(if (cursor.isNull(4)) dtStart else cursor.getLong(4)))
        obj.put("all_day", cursor.getInt(5) == 1)
        return obj.toString()
      }
    }
    throw DeviceCalException.Backend("event $id not found after write")
  }

  private fun hasReadPermission(): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.READ_CALENDAR) ==
      PackageManager.PERMISSION_GRANTED

  private fun hasWritePermission(): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.WRITE_CALENDAR) ==
      PackageManager.PERMISSION_GRANTED

  private fun requireRead() {
    if (!hasReadPermission()) throw DeviceCalException.PermissionDenied()
  }

  private fun requireWrite() {
    if (!hasWritePermission()) throw DeviceCalException.PermissionDenied()
  }

  /** RFC-3339 → epoch millis. */
  private fun parseInstant(value: String): Long =
    try {
      Instant.parse(value).toEpochMilli()
    } catch (e: Exception) {
      throw DeviceCalException.Backend("invalid instant $value: ${e.message}")
    }

  /** Epoch millis → RFC-3339 (UTC, e.g. `2026-06-21T09:00:00Z`). */
  private fun iso(epochMillis: Long): String = Instant.ofEpochMilli(epochMillis).toString()

  /** CalendarProvider colour int (ARGB) → `#RRGGBB`. */
  private fun hexColor(argb: Int): String = String.format("#%06X", 0xFFFFFF and argb)

  /** Strip the occurrence suffix `getEvents` appends, recovering the numeric id. */
  private fun baseEventId(id: String): Long? = id.substringBefore("#").toLongOrNull()
}
