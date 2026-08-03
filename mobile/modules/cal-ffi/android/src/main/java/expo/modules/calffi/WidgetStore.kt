package expo.modules.calffi

import android.content.Context
import java.io.File
import java.text.DateFormat
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale
import java.util.TimeZone
import org.json.JSONArray
import org.json.JSONObject

// The Android side of the widget handover — the twin of
// `modules/cal-ffi/ios/WidgetSnapshotStore.swift` and `targets/widget/*.swift`,
// with one structural simplification: an Android app widget runs in the app's
// OWN process, under the same uid, so there is no App Group to cross. The files
// live in the app's internal storage and both sides just open them.
//
// Everything else is deliberately identical to iOS, including the decision that
// the widget writes the QUESTION and the app answers it: completing a task
// cascades to parents and children, advances a recurring series and queues a
// sync push, and that machinery is the app's JavaScript over the Rust core. A
// broadcast receiver has none of it.

object WidgetStore {
  /** Both files live here; the directory is created on first write. */
  private fun dir(context: Context): File = File(context.filesDir, "widget")

  fun snapshotFile(context: Context): File = File(dir(context), "upcoming.json")

  private fun actionsDir(context: Context): File = File(dir(context), "actions")

  /**
   * Replace the snapshot.
   *
   * Written to a neighbouring temporary file and renamed into place, because the
   * widget can be asked to redraw at any moment: a plain overwrite has a window
   * in which the file is a valid path holding half a document, which the widget
   * would render as "nothing planned".
   */
  fun writeSnapshot(context: Context, json: String) {
    val target = snapshotFile(context)
    target.parentFile?.mkdirs()
    val scratch = File(target.parentFile, "upcoming.json.writing")
    scratch.writeText(json)
    if (!scratch.renameTo(target)) {
      // `renameTo` refuses over an existing file on some filesystems; the
      // delete-then-rename is the fallback, and its (small) window is still
      // narrower than writing the document in place.
      target.delete()
      if (!scratch.renameTo(target)) scratch.delete()
    }
  }

  /** The snapshot, or null when there is none or it cannot be parsed. */
  fun readSnapshot(context: Context): JSONObject? =
    try {
      val file = snapshotFile(context)
      if (!file.exists()) null else JSONObject(file.readText())
    } catch (_: Exception) {
      null
    }

  /**
   * Queue a tap.
   *
   * ONE file per action, never one shared file: the widget's receiver and the
   * app's JavaScript both write here, and a read-modify-write on a common file
   * loses whichever tap lost the race.
   */
  fun enqueue(context: Context, itemId: String, containerId: String) {
    try {
      val directory = actionsDir(context)
      directory.mkdirs()
      val payload =
        JSONObject()
          .put("version", 1)
          // `toggle`, not "complete": under the cycling check-off mode one tap
          // moves a task from open to in progress, and the app decides that.
          .put("action", "toggle")
          .put("itemId", itemId)
          .put("containerId", containerId)
          .put("at", isoNow())
      File(directory, "${java.util.UUID.randomUUID()}.json").writeText(payload.toString())
    } catch (_: Exception) {
      // Silent: a widget has nowhere to report an error to, and the task is
      // still there to tick in the app.
    }
  }

  /** Item ids with a queued tap, so the widget can hide a row it has already
   *  been told about — one that stays put reads as a control that does nothing. */
  fun pendingTaps(context: Context): Set<String> {
    val files = actionsDir(context).listFiles() ?: return emptySet()
    val ids = mutableSetOf<String>()
    for (file in files) {
      try {
        val obj = JSONObject(file.readText())
        if (obj.optString("action") == "toggle") ids.add(obj.optString("itemId"))
      } catch (_: Exception) {
        // A half-written or corrupt file is dropped rather than retried forever.
        file.delete()
      }
    }
    return ids
  }

  /** Every queued action as a JSON array, each entry carrying the file's `id`. */
  fun pendingJson(context: Context): String {
    val files = actionsDir(context).listFiles() ?: return "[]"
    val out = JSONArray()
    for (file in files) {
      try {
        val obj = JSONObject(file.readText())
        obj.put("id", file.nameWithoutExtension)
        out.put(obj)
      } catch (_: Exception) {
        file.delete()
      }
    }
    return out.toString()
  }

  /** Drop one queued action, after it has been ATTEMPTED — see the JS drain for
   *  why an unperformable action must not linger. */
  fun clearAction(context: Context, id: String) {
    if (id.contains("/") || id.contains("..")) return
    File(actionsDir(context), "$id.json").delete()
  }

  // ── Instants ─────────────────────────────────────────────────────────────
  // `SimpleDateFormat` rather than `java.time`, which would need core library
  // desugaring to reach the app's minimum API level. These run a handful of
  // times per redraw, so the allocation is not worth a build-config change.

  private fun utcFormat(pattern: String): SimpleDateFormat =
    SimpleDateFormat(pattern, Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }

  private fun isoNow(): String =
    utcFormat("yyyy-MM-dd'T'HH:mm:ss.SSS'Z'").format(Date())

  /** Parse an instant the app wrote. The millisecond form is what
   *  `toISOString()` produces; the second-precision one is the fallback. */
  fun parseInstant(raw: String?): Date? {
    if (raw.isNullOrEmpty()) return null
    for (pattern in arrayOf("yyyy-MM-dd'T'HH:mm:ss.SSSXXX", "yyyy-MM-dd'T'HH:mm:ssXXX")) {
      try {
        return utcFormat(pattern).parse(raw)
      } catch (_: Exception) {
        // Try the next shape.
      }
    }
    return null
  }

  /**
   * Aperio's LANGUAGE with the device's REGION.
   *
   * The same split iOS makes, for the same reason: German or English is the
   * app's own setting, which the user may have chosen against their device,
   * while a 24-hour clock and day-before-month are the device's and no app
   * should override them.
   */
  fun localeFor(tag: String?): Locale {
    val language = (tag ?: "").replace('_', '-').substringBefore('-')
    if (language.isEmpty()) return Locale.getDefault()
    return Locale(language, Locale.getDefault().country)
  }

  /** A time in the given locale's format. */
  fun timeText(date: Date, locale: Locale): String =
    DateFormat.getTimeInstance(DateFormat.SHORT, locale).format(date)

  /** A spelled-out day ("Mi., 5. Aug."), or null for today. */
  fun dayText(date: Date, locale: Locale): String? {
    if (isToday(date)) return null
    return SimpleDateFormat("EEE, d. MMM", locale).format(date)
  }

  fun isToday(date: Date): Boolean {
    val a = Calendar.getInstance().apply { time = date }
    val b = Calendar.getInstance()
    return a.get(Calendar.YEAR) == b.get(Calendar.YEAR) &&
      a.get(Calendar.DAY_OF_YEAR) == b.get(Calendar.DAY_OF_YEAR)
  }

  /**
   * The moment a row stops being "next" — the twin of `WidgetItem.expiresAt`.
   *
   * An UNTIMED row with no end stands until its day turns: its instant is local
   * midnight, already in the past for all but the first minute of the day, and
   * taking that at face value would drop every undated task the moment the
   * widget drew itself.
   */
  fun expiresAt(item: JSONObject): Date? {
    parseInstant(item.optString("end", null))?.let { return it }
    val at = parseInstant(item.optString("at", null)) ?: return null
    if (!item.optBoolean("untimed", false)) return at
    val cal = Calendar.getInstance().apply { time = at }
    cal.set(Calendar.HOUR_OF_DAY, 0)
    cal.set(Calendar.MINUTE, 0)
    cal.set(Calendar.SECOND, 0)
    cal.set(Calendar.MILLISECOND, 0)
    cal.add(Calendar.DAY_OF_YEAR, 1)
    return cal.time
  }
}
