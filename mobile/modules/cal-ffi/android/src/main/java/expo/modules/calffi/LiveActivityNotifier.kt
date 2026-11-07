package expo.modules.calffi

import android.app.AlarmManager
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.text.format.DateFormat
import android.util.Log
import androidx.core.app.AlarmManagerCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

// The Android Live Activity of an appointment (DESIGN-live-activities.md,
// "Android"): from its last reminder until it ends, an ongoing notification
// with its time, place and what comes next, promoted to a Live Update shortly
// before the start where Android allows it.
//
// JavaScript hands over the plan the core computed (`shared/liveActivityAndroid
// .ts`) and leaves the reminders it holds out of Expo. From then on this works
// alone: ONE exact alarm for the next boundary, whose receiver shows what runs
// and arms the next. It never opens the core — that is expensive, and was
// refused for exactly such entry points (TODO A7) —, it reads only its files.
//
// Three files besides the plan, so a new hand-over never forgets what happened:
// the reminders that rang (each rings once), the swipes (the appointment stays
// away until a reminder of its own), and which notification is up.
//
// Every entry point is synchronized: the hand-over runs on Expo's module queue,
// the receivers on the main thread, and both rewrite the same files.

private const val TAG = "aperio-live"
private const val LOG_TAG = "AperioLive"
private const val ALARM_REQUEST = 0x4c41
private const val SILENT_CHANNEL = "reminders-silent"
private const val DEFAULT_CHANNEL = "reminders"

// The extra behind `Notification.Builder.setRequestPromotedOngoing` (API 36.1,
// newer than the SDK this builds against) — the one androidx writes too. An
// older Android ignores it.
private const val EXTRA_REQUEST_PROMOTED_ONGOING = "android.requestPromotedOngoing"

const val LIVE_ACTION_TICK = "expo.modules.calffi.LIVE_ACTIVITY_TICK"
const val LIVE_ACTION_DISMISSED = "expo.modules.calffi.LIVE_ACTIVITY_DISMISSED"

private data class LiveSection(
  val from: Long,
  val until: Long,
  val key: String,
  val eventId: String,
  val calendarId: String,
  val title: String,
  val location: String?,
  val start: Long,
  val end: Long,
  val followUpTitle: String?,
  val followUpStart: Long?,
  val started: Boolean,
  val promoted: Boolean,
  val channelId: String,
  /** The reminders this section IS, or null. */
  val signal: List<String>?,
  val chainEnd: Long,
)

private data class LiveTexts(
  val beginsAt: String,
  val runsUntil: String,
  val followUp: String,
  val separator: String,
)

private fun JSONObject.optText(name: String): String? =
  if (!has(name) || isNull(name)) null else optString(name).takeIf { it.isNotEmpty() }

private fun JSONObject.optMillis(name: String): Long? =
  if (!has(name) || isNull(name)) null else optLong(name)

private fun JSONObject.optStrings(name: String): List<String>? {
  if (!has(name) || isNull(name)) return null
  val array = optJSONArray(name) ?: return null
  return (0 until array.length()).map { array.optString(it) }.filter { it.isNotEmpty() }.ifEmpty { null }
}

object LiveActivityNotifier {
  private fun dir(context: Context): File = File(context.filesDir, "live-activities")

  private fun file(context: Context, name: String): File = File(dir(context), name)

  /** Replace a file through a neighbour and a rename, as WidgetStore does: a
   *  reader must never see half a document. */
  private fun write(context: Context, name: String, text: String) {
    val target = file(context, name)
    target.parentFile?.mkdirs()
    val scratch = File(target.parentFile, "$name.writing")
    scratch.writeText(text)
    if (!scratch.renameTo(target)) {
      target.delete()
      if (!scratch.renameTo(target)) scratch.delete()
    }
  }

  private fun readObject(context: Context, name: String): JSONObject? =
    try {
      val f = file(context, name)
      if (f.exists()) JSONObject(f.readText()) else null
    } catch (_: Exception) {
      null
    }

  private fun readTexts(plan: JSONObject): LiveTexts {
    val t = plan.optJSONObject("texts") ?: JSONObject()
    return LiveTexts(
      beginsAt = t.optString("beginsAt", "{time}"),
      runsUntil = t.optString("runsUntil", "{time}"),
      followUp = t.optString("followUp", "{title} {time}"),
      separator = t.optString("separator", " · "),
    )
  }

  private fun readSections(plan: JSONObject): List<LiveSection> {
    val list = plan.optJSONArray("sections") ?: return emptyList()
    val out = ArrayList<LiveSection>(list.length())
    for (i in 0 until list.length()) {
      val s = list.optJSONObject(i) ?: continue
      out.add(
        LiveSection(
          from = s.optLong("from"),
          until = s.optLong("until"),
          key = s.optString("key"),
          eventId = s.optString("eventId"),
          calendarId = s.optString("calendarId"),
          title = s.optString("title"),
          location = s.optText("location"),
          start = s.optLong("start"),
          end = s.optLong("end"),
          followUpTitle = s.optText("followUpTitle"),
          followUpStart = s.optMillis("followUpStart"),
          started = s.optBoolean("started"),
          promoted = s.optBoolean("promoted"),
          channelId = s.optText("channelId") ?: DEFAULT_CHANNEL,
          signal = s.optStrings("signal"),
          chainEnd = s.optLong("chainEnd"),
        ),
      )
    }
    return out
  }

  /** Keys with the time until which they matter, pruned on read. */
  private fun readTimed(context: Context, name: String, now: Long): MutableMap<String, Long> {
    val obj = readObject(context, name) ?: return mutableMapOf()
    val out = mutableMapOf<String, Long>()
    for (key in obj.keys()) {
      val until = obj.optLong(key)
      if (until > now) out[key] = until
    }
    return out
  }

  private fun writeTimed(context: Context, name: String, map: Map<String, Long>) {
    val obj = JSONObject()
    for ((key, until) in map) obj.put(key, until)
    write(context, name, obj.toString())
  }

  /**
   * Take over a new plan. `rangJson` lists the reminders Expo rang (the past
   * ones it still held): they count as rung. A section whose channel is not
   * known now keeps the one the previous plan had.
   */
  @Synchronized
  fun handOver(context: Context, planJson: String, rangJson: String) {
    val plan = JSONObject(planJson)
    val sections = plan.optJSONArray("sections") ?: JSONArray()
    // What the previous plan knew about the channels.
    val known = mutableMapOf<String, String>()
    readObject(context, "plan.json")?.let { previous ->
      for (s in readSections(previous)) {
        s.signal?.forEach { known["signal:$it"] = s.channelId }
        known.putIfAbsent("key:${s.key}", s.channelId)
      }
    }
    for (i in 0 until sections.length()) {
      val s = sections.optJSONObject(i) ?: continue
      if (s.optText("channelId") == null) {
        val inherited =
          s.optStrings("signal")?.firstNotNullOfOrNull { known["signal:$it"] }
            ?: known["key:${s.optString("key")}"]
        if (inherited != null) s.put("channelId", inherited)
      }
    }
    try {
      write(context, "plan.json", plan.toString())
      val now = System.currentTimeMillis()
      val rang = readTimed(context, "rang.json", now)
      val horizon = readSections(plan).maxOfOrNull { it.chainEnd } ?: now
      val expo = JSONArray(rangJson)
      for (i in 0 until expo.length()) rang[expo.optString(i)] = maxOf(horizon, now + 60_000L)
      writeTimed(context, "rang.json", rang)
      refreshLocked(context)
    } catch (e: Exception) {
      // Not taken over: JavaScript gives every reminder back to Expo, so
      // nothing of this plan may ring.
      clearLocked(context)
      throw e
    }
  }

  /** Switched off: nothing shows, nothing is armed, nothing is kept. */
  @Synchronized
  fun clear(context: Context) = clearLocked(context)

  private fun clearLocked(context: Context) {
    cancelAlarm(context)
    readObject(context, "state.json")?.let {
      NotificationManagerCompat.from(context).cancel(TAG, it.optInt("id"))
    }
    for (name in listOf("plan.json", "state.json", "rang.json")) file(context, name).delete()
  }

  /**
   * A swipe on the notification: that appointment, and the rest of its chain,
   * stay away until a reminder of its own. A timeout or the end of the chain
   * is no swipe.
   */
  @Synchronized
  fun dismissed(context: Context, key: String, plannedEnd: Long) {
    val now = System.currentTimeMillis()
    if (now >= plannedEnd - 1000) return
    val swiped = readTimed(context, "dismissed.json", now)
    if (key.isNotEmpty()) swiped[key] = plannedEnd
    swiped["chain:$plannedEnd"] = plannedEnd
    writeTimed(context, "dismissed.json", swiped)
    file(context, "state.json").delete()
    refreshLocked(context)
  }

  /** Show what runs now, and arm the alarm for the next boundary. */
  @Synchronized
  fun refresh(context: Context) = refreshLocked(context)

  private fun refreshLocked(context: Context) {
    val now = System.currentTimeMillis()
    val plan = readObject(context, "plan.json")
    if (plan == null) {
      cancelAlarm(context)
      readObject(context, "state.json")?.let {
        NotificationManagerCompat.from(context).cancel(TAG, it.optInt("id"))
      }
      file(context, "state.json").delete()
      return
    }
    val sections = readSections(plan)
    val current = sections.firstOrNull { it.from <= now && now < it.until }
    // The next boundary is armed FIRST, so a failure below cannot break the
    // chain of alarms that every later reminder hangs on.
    val boundary =
      sequenceOf(current?.until, sections.map { it.from }.filter { it > now }.minOrNull())
        .filterNotNull()
        .minOrNull()
    if (boundary != null) armAlarm(context, boundary) else cancelAlarm(context)
    show(context, plan, sections, current, now)
  }

  private fun show(
    context: Context,
    plan: JSONObject,
    sections: List<LiveSection>,
    current: LiveSection?,
    now: Long,
  ) {
    val manager = NotificationManagerCompat.from(context)
    val state = readObject(context, "state.json")
    val shownId = state?.optInt("id")
    val rang = readTimed(context, "rang.json", now)
    val swiped = readTimed(context, "dismissed.json", now)

    if (current == null) {
      if (shownId != null) manager.cancel(TAG, shownId)
      file(context, "state.json").delete()
      writeTimed(context, "dismissed.json", swiped)
      return
    }
    // Every reminder of this chain that is due and has not rung — also one
    // whose section an alarm came too late for. Late, but never lost: it rings
    // now, once, for all of them.
    val due =
      sections.filter { s ->
        s.chainEnd == current.chainEnd &&
          s.from <= now &&
          s.signal != null &&
          s.signal.none { rang.containsKey(it) }
      }
    val rings = due.isNotEmpty()
    if (rings) {
      // A reminder the user has not seen comes even after a swipe.
      swiped.remove(current.key)
      swiped.remove("chain:${current.chainEnd}")
      for (s in due) s.signal?.forEach { rang[it] = maxOf(current.chainEnd, now + 60_000L) }
      writeTimed(context, "rang.json", rang)
    }
    writeTimed(context, "dismissed.json", swiped)
    if (!rings && (swiped.containsKey(current.key) || swiped.containsKey("chain:${current.chainEnd}"))) {
      if (shownId != null) manager.cancel(TAG, shownId)
      file(context, "state.json").delete()
      return
    }

    val id: Int
    if (rings) {
      // The reminder itself: a NEW notification, so it rings with the
      // reminder's sound and is announced.
      id = notificationId(due.last().signal!!.joinToString("|"))
      if (shownId != null && shownId != id) manager.cancel(TAG, shownId)
    } else {
      id = shownId ?: notificationId(current.key)
    }
    val texts = readTexts(plan)
    val locale = WidgetStore.localeFor(plan.optText("language"))
    val text = sentence(context, current, texts, locale)
    // Nothing changed since the last post: posting again would cost a redraw
    // and, after a lost notification, an announcement.
    val fingerprint = listOf(id, current.title, text, current.channelId, current.promoted, current.started).hashCode()
    if (!rings && state != null && state.optInt("fingerprint") == fingerprint && state.optInt("id") == id) {
      return
    }
    post(context, manager, id, current, text, rings, now)
    write(
      context,
      "state.json",
      JSONObject().put("id", id).put("key", current.key).put("fingerprint", fingerprint).toString(),
    )
  }

  private fun notificationId(text: String): Int = text.hashCode() and 0x7fffffff

  /** A clock time in Aperio's language, on the phone's 12- or 24-hour clock. */
  private fun time(context: Context, millis: Long, locale: Locale): String {
    val skeleton = if (DateFormat.is24HourFormat(context)) "Hm" else "hm"
    val pattern = DateFormat.getBestDateTimePattern(locale, skeleton)
    return SimpleDateFormat(pattern, locale).format(Date(millis))
  }

  /** "Beginnt um 10:00 · Raum 2.14 · Danach: Review um 11:00" — clock times,
   *  not "in 10 minutes": TalkBack does not read the countdown, and this text
   *  changes only at a boundary. */
  private fun sentence(context: Context, s: LiveSection, texts: LiveTexts, locale: Locale): String {
    val parts = ArrayList<String>()
    parts.add(
      if (s.started) {
        texts.runsUntil.replace("{time}", time(context, s.end, locale))
      } else {
        texts.beginsAt.replace("{time}", time(context, s.start, locale))
      },
    )
    s.location?.let { parts.add(it) }
    if (s.followUpTitle != null && s.followUpStart != null) {
      parts.add(
        texts.followUp
          .replace("{title}", s.followUpTitle)
          .replace("{time}", time(context, s.followUpStart, locale)),
      )
    }
    return parts.joinToString(texts.separator)
  }

  private fun smallIcon(context: Context): Int {
    val id = context.resources.getIdentifier("notification_icon", "drawable", context.packageName)
    return if (id != 0) id else context.applicationInfo.icon
  }

  private fun post(
    context: Context,
    manager: NotificationManagerCompat,
    id: Int,
    s: LiveSection,
    text: String,
    loud: Boolean,
    now: Long,
  ) {
    if (!manager.areNotificationsEnabled()) return
    val builder =
      NotificationCompat.Builder(context, s.channelId)
        .setSmallIcon(smallIcon(context))
        .setContentTitle(s.title)
        .setContentText(text)
        .setStyle(NotificationCompat.BigTextStyle().bigText(text))
        .setCategory(NotificationCompat.CATEGORY_EVENT)
        .setOngoing(true)
        .setOnlyAlertOnce(!loud)
        .setSilent(!loud)
        // The countdown runs to the start, then to the end. Seen, not read:
        // the sentence says the same in words.
        .setShowWhen(true)
        .setWhen(if (s.started) s.end else s.start)
        .setUsesChronometer(true)
        .setChronometerCountDown(true)
        // Gone at the end of the chain even if no alarm comes.
        .setTimeoutAfter((s.chainEnd - now).coerceAtLeast(1000L))
        .setContentIntent(openIntent(context, s, id))
        .setDeleteIntent(dismissIntent(context, s, id))
        // Asked for only shortly before the start (Google: an event "about to
        // start", not an upcoming one); never colorized, which blocks it.
        .addExtras(Bundle().apply { putBoolean(EXTRA_REQUEST_PROMOTED_ONGOING, s.promoted) })
    // Before Android 8 there are no channels: the notification itself has to
    // say whether it rings, as Expo's reminders do there.
    if (Build.VERSION.SDK_INT < 26) {
      if (loud && s.channelId != SILENT_CHANNEL) {
        builder.setDefaults(NotificationCompat.DEFAULT_ALL)
        builder.setPriority(NotificationCompat.PRIORITY_HIGH)
      } else {
        builder.setPriority(NotificationCompat.PRIORITY_LOW)
      }
    }
    try {
      manager.notify(TAG, id, builder.build())
    } catch (e: SecurityException) {
      // Notifications not allowed: nothing to show. The pass gives the
      // reminders back to Expo once the permission is missing.
      Log.w(LOG_TAG, "live activity not posted", e)
    }
  }

  /** A tap opens the appointment: the same address a tapped iPhone Live
   *  Activity uses, read by `reminderTapOfUrl`. Straight to the app's
   *  activity — Android 12 refuses a detour through a receiver. */
  private fun openIntent(context: Context, s: LiveSection, id: Int): PendingIntent {
    val uri =
      Uri.parse(
        "aperio://event?eventId=${Uri.encode(s.eventId)}" +
          "&calendarId=${Uri.encode(s.calendarId)}" +
          "&start=${Uri.encode(isoOf(s.start))}" +
          "&title=${Uri.encode(s.title)}",
      )
    val intent =
      Intent(Intent.ACTION_VIEW, uri)
        .setPackage(context.packageName)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    return PendingIntent.getActivity(
      context,
      id,
      intent,
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
  }

  private fun dismissIntent(context: Context, s: LiveSection, id: Int): PendingIntent {
    val intent =
      Intent(context, LiveActivityReceiver::class.java)
        .setAction(LIVE_ACTION_DISMISSED)
        .putExtra("key", s.key)
        .putExtra("plannedEnd", s.chainEnd)
    return PendingIntent.getBroadcast(
      context,
      id,
      intent,
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
  }

  /** RFC 3339 in UTC, the way the router reads an occurrence start. */
  private fun isoOf(millis: Long): String {
    val format = SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", Locale.US)
    format.timeZone = TimeZone.getTimeZone("UTC")
    return format.format(Date(millis))
  }

  private fun alarmIntent(context: Context): PendingIntent =
    PendingIntent.getBroadcast(
      context,
      ALARM_REQUEST,
      Intent(context, LiveActivityReceiver::class.java).setAction(LIVE_ACTION_TICK),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

  /** Exact where allowed (USE_EXACT_ALARM from Android 13, SCHEDULE_EXACT_ALARM
   *  on 12); otherwise inexact, and a late tick still rings what is due. */
  private fun armAlarm(context: Context, at: Long) {
    val alarms = context.getSystemService(Context.ALARM_SERVICE) as AlarmManager
    val pending = alarmIntent(context)
    val exact = Build.VERSION.SDK_INT < 31 || alarms.canScheduleExactAlarms()
    if (exact) {
      AlarmManagerCompat.setExactAndAllowWhileIdle(alarms, AlarmManager.RTC_WAKEUP, at, pending)
    } else {
      AlarmManagerCompat.setAndAllowWhileIdle(alarms, AlarmManager.RTC_WAKEUP, at, pending)
    }
  }

  private fun cancelAlarm(context: Context) {
    val alarms = context.getSystemService(Context.ALARM_SERVICE) as AlarmManager
    alarms.cancel(alarmIntent(context))
  }

  /** Android 16 QPR1 (36.1) or later: the version that promotes on request.
   *  Read without the newer SDK's constants, which this does not build with. */
  private fun canRequestPromotion(): Boolean {
    if (Build.VERSION.SDK_INT > 36) return true
    if (Build.VERSION.SDK_INT < 36) return false
    return try {
      Build.VERSION::class.java.getField("SDK_INT_FULL").getInt(null) >= 3_600_001
    } catch (_: Exception) {
      false
    }
  }

  /** `{supported, allowed}`: whether Android can promote the notification,
   *  and whether it may for Aperio. */
  fun promotionJson(context: Context): String {
    val supported = canRequestPromotion()
    val allowed =
      supported &&
        context.getSystemService(NotificationManager::class.java).canPostPromotedNotifications()
    return JSONObject().put("supported", supported).put("allowed", allowed).toString()
  }

  /** The system page for promoting Aperio's notifications, else its
   *  notification settings. */
  fun openPromotionSettings(context: Context) {
    val promotion =
      if (Build.VERSION.SDK_INT >= 36) {
        Intent(Settings.ACTION_APP_NOTIFICATION_PROMOTION_SETTINGS)
      } else {
        null
      }
    val fallback = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
    for (intent in listOfNotNull(promotion, fallback)) {
      intent.putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
      intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      if (intent.resolveActivity(context.packageManager) != null) {
        context.startActivity(intent)
        return
      }
    }
  }
}

/** The alarm, and a swipe on the notification. A failure is logged, never
 *  thrown: a crash here would stop nothing else, but would hide the reason. */
class LiveActivityReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    try {
      when (intent.action) {
        LIVE_ACTION_TICK -> LiveActivityNotifier.refresh(context)
        LIVE_ACTION_DISMISSED ->
          LiveActivityNotifier.dismissed(
            context,
            intent.getStringExtra("key") ?: "",
            intent.getLongExtra("plannedEnd", 0L),
          )
      }
    } catch (e: Exception) {
      Log.w(LOG_TAG, "live activity receiver failed", e)
    }
  }
}

/** A reboot, an update or a clock change: alarms are gone or wrong, so the
 *  current section is shown again and the next boundary armed. */
class LiveActivityRestartReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    try {
      when (intent.action) {
        Intent.ACTION_BOOT_COMPLETED,
        Intent.ACTION_MY_PACKAGE_REPLACED,
        Intent.ACTION_TIME_CHANGED,
        Intent.ACTION_TIMEZONE_CHANGED,
        -> LiveActivityNotifier.refresh(context)
      }
    } catch (e: Exception) {
      Log.w(LOG_TAG, "live activity restart failed", e)
    }
  }
}
