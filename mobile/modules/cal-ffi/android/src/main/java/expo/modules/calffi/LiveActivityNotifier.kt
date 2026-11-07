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
import androidx.core.app.AlarmManagerCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.util.Date

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
// the signals that rang (a reminder rings once), the appointments swiped away
// (they stay away), and which notification is up.

private const val TAG = "aperio-live"
private const val CATCH_UP_GRACE_MS = 5 * 60 * 1000L
private const val ALARM_REQUEST = 0x4c41

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
  val signal: String?,
  val chainEnd: Long,
)

private data class LiveTexts(
  val beginsAt: String,
  val runsUntil: String,
  val followUp: String,
  val separator: String,
)

private fun JSONObject.optText(name: String): String? =
  if (isNull(name)) null else optString(name).takeIf { it.isNotEmpty() }

private fun JSONObject.optMillis(name: String): Long? = if (isNull(name)) null else optLong(name)

object LiveActivityNotifier {
  private fun dir(context: Context): File = File(context.filesDir, "live-activities")

  private fun file(context: Context, name: String): File = File(dir(context), name)

  /** Replace a file through a neighbour and a rename, as WidgetStore does: the
   *  receiver can run at any moment and must never read half a document. */
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
          channelId = s.optString("channelId", "reminders"),
          signal = s.optText("signal"),
          chainEnd = s.optLong("chainEnd"),
        ),
      )
    }
    return out
  }

  /** The signals that rang, and the swiped appointments, each with the time
   *  until which they matter. */
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
   * Take over a new plan. `freshStart` says Expo still rang every reminder up
   * to now (the switch was just turned on, or this is the first hand-over):
   * then every signal already past counts as rung, and comes back silently.
   */
  fun handOver(context: Context, planJson: String, freshStart: Boolean) {
    val plan = JSONObject(planJson)
    write(context, "plan.json", plan.toString())
    if (freshStart) {
      val now = System.currentTimeMillis()
      val rang = readTimed(context, "rang.json", now)
      for (section in readSections(plan)) {
        if (section.signal != null && section.from <= now) rang[section.signal] = section.chainEnd
      }
      writeTimed(context, "rang.json", rang)
    }
    refresh(context)
  }

  /** Switched off: nothing shows, nothing is armed, nothing is kept. */
  fun clear(context: Context) {
    cancelAlarm(context)
    val state = readObject(context, "state.json")
    if (state != null) NotificationManagerCompat.from(context).cancel(TAG, state.optInt("id"))
    for (name in listOf("plan.json", "state.json", "rang.json")) file(context, name).delete()
  }

  /** A swipe on the notification: that appointment stays away. A timeout or
   *  the end of the chain is no swipe. */
  fun dismissed(context: Context, key: String, plannedEnd: Long) {
    val now = System.currentTimeMillis()
    if (key.isEmpty() || now >= plannedEnd - 1000) return
    val swiped = readTimed(context, "dismissed.json", now)
    swiped[key] = plannedEnd
    writeTimed(context, "dismissed.json", swiped)
    file(context, "state.json").delete()
  }

  /**
   * Show what runs now, and arm the alarm for the next boundary. Called by the
   * hand-over, by the alarm, and after a reboot, an update or a clock change.
   */
  fun refresh(context: Context) {
    val now = System.currentTimeMillis()
    val plan = readObject(context, "plan.json")
    if (plan == null) {
      cancelAlarm(context)
      return
    }
    val sections = readSections(plan)
    val texts = readTexts(plan)
    val rang = readTimed(context, "rang.json", now)
    val swiped = readTimed(context, "dismissed.json", now)
    writeTimed(context, "rang.json", rang)
    writeTimed(context, "dismissed.json", swiped)
    val manager = NotificationManagerCompat.from(context)
    val state = readObject(context, "state.json")
    val shownId = state?.optInt("id")

    val current = sections.firstOrNull { it.from <= now && now < it.until }
    if (current == null || swiped.containsKey(current.key)) {
      if (shownId != null) manager.cancel(TAG, shownId)
      file(context, "state.json").delete()
    } else {
      val signal = current.signal
      val rings = signal != null && !rang.containsKey(signal)
      val id: Int
      if (rings) {
        // The reminder itself: a NEW notification, so it rings with the
        // reminder's sound and is announced — unless it is too late for that.
        id = notificationId(signal!!)
        if (shownId != null && shownId != id) manager.cancel(TAG, shownId)
        rang[signal] = current.chainEnd
        writeTimed(context, "rang.json", rang)
      } else {
        id = shownId ?: notificationId(current.key)
      }
      val loud = rings && now - current.from <= CATCH_UP_GRACE_MS
      post(context, manager, id, current, texts, loud, now)
      val next = JSONObject().put("id", id).put("key", current.key)
      write(context, "state.json", next.toString())
    }

    val boundary =
      sequenceOf(current?.until, sections.map { it.from }.filter { it > now }.minOrNull())
        .filterNotNull()
        .minOrNull()
    if (boundary != null) armAlarm(context, boundary) else cancelAlarm(context)
  }

  private fun notificationId(text: String): Int = text.hashCode() and 0x7fffffff

  private fun time(context: Context, millis: Long): String =
    DateFormat.getTimeFormat(context).format(Date(millis))

  /** "Beginnt um 10:00 · Raum 2.14 · Danach: Review um 11:00" — clock times,
   *  not "in 10 minutes": TalkBack does not read the countdown, and this text
   *  changes only at a boundary. */
  private fun sentence(context: Context, s: LiveSection, texts: LiveTexts): String {
    val parts = ArrayList<String>()
    parts.add(
      if (s.started) {
        texts.runsUntil.replace("{time}", time(context, s.end))
      } else {
        texts.beginsAt.replace("{time}", time(context, s.start))
      },
    )
    s.location?.let { parts.add(it) }
    if (s.followUpTitle != null && s.followUpStart != null) {
      parts.add(
        texts.followUp
          .replace("{title}", s.followUpTitle)
          .replace("{time}", time(context, s.followUpStart)),
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
    texts: LiveTexts,
    loud: Boolean,
    now: Long,
  ) {
    if (!manager.areNotificationsEnabled()) return
    val text = sentence(context, s, texts)
    val builder =
      NotificationCompat.Builder(context, s.channelId)
        .setSmallIcon(smallIcon(context))
        .setContentTitle(s.title)
        .setContentText(text)
        .setStyle(NotificationCompat.BigTextStyle().bigText(text))
        .setTicker("${s.title}${texts.separator}$text")
        .setCategory(NotificationCompat.CATEGORY_EVENT)
        .setOngoing(true)
        .setOnlyAlertOnce(!loud)
        .setSilent(!loud)
        // The countdown runs to the start, then to the end. Seen, not read:
        // the sentence above says the same in words.
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
    try {
      manager.notify(TAG, id, builder.build())
    } catch (_: SecurityException) {
      // Notifications not allowed: nothing to show, nothing lost — the
      // reminders it would carry are Expo's again once permission is back.
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
    val format = java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", java.util.Locale.US)
    format.timeZone = java.util.TimeZone.getTimeZone("UTC")
    return format.format(Date(millis))
  }

  private fun alarmIntent(context: Context): PendingIntent =
    PendingIntent.getBroadcast(
      context,
      ALARM_REQUEST,
      Intent(context, LiveActivityReceiver::class.java).setAction(LIVE_ACTION_TICK),
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

  /** Exact where allowed: on Android 12 without the exact-alarm right, as
   *  Expo does, an inexact one — later, but never lost. */
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

  /** `{supported, allowed}`: Android 16 or later can promote, and whether it
   *  may for Aperio. */
  fun promotionJson(context: Context): String {
    val supported = Build.VERSION.SDK_INT >= 36
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

/** The alarm, and a swipe on the notification. */
class LiveActivityReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    when (intent.action) {
      LIVE_ACTION_TICK -> LiveActivityNotifier.refresh(context)
      LIVE_ACTION_DISMISSED -> {
        LiveActivityNotifier.dismissed(
          context,
          intent.getStringExtra("key") ?: "",
          intent.getLongExtra("plannedEnd", 0L),
        )
        LiveActivityNotifier.refresh(context)
      }
    }
  }
}

/** A reboot, an update or a clock change: alarms are gone or wrong, so the
 *  current section is shown again and the next boundary armed. */
class LiveActivityRestartReceiver : BroadcastReceiver() {
  override fun onReceive(context: Context, intent: Intent) {
    when (intent.action) {
      Intent.ACTION_BOOT_COMPLETED,
      Intent.ACTION_MY_PACKAGE_REPLACED,
      Intent.ACTION_TIME_CHANGED,
      Intent.ACTION_TIMEZONE_CHANGED,
      -> LiveActivityNotifier.refresh(context)
    }
  }
}
