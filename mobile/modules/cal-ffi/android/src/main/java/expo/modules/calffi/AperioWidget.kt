package expo.modules.calffi

import android.content.Context
import androidx.compose.runtime.Composable
// Glance measures in Compose's `Dp`, not a unit of its own.
import androidx.compose.ui.unit.dp
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.action.ActionParameters
import androidx.glance.action.actionParametersOf
import androidx.glance.appwidget.CheckBox
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.provideContent
// `update` and `updateAll` are EXTENSION functions on GlanceAppWidget, not
// members — calling them without importing the package compiles nowhere.
import androidx.glance.appwidget.update
import androidx.glance.layout.Alignment
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.padding
import androidx.glance.layout.size
import androidx.glance.layout.width
import androidx.glance.semantics.contentDescription
import androidx.glance.semantics.semantics
import androidx.glance.text.Text
import java.util.Date
import java.util.Locale
import org.json.JSONObject

// "Als Nächstes" on the Android home screen — the twin of the iOS list widget,
// reading the same snapshot the app writes.
//
// Built on Glance rather than hand-rolled RemoteViews for one reason that
// outweighs the rest: `CheckBox` carries the checkbox ROLE and state to
// TalkBack. RemoteViews can draw a circle and set a description, but it cannot
// say "this is a checkbox, and it is not ticked" — and on a surface whose whole
// point is one element per task, that is the feature, not the styling.
//
// Android has no lock-screen widgets (dropped after Android 11), so this is the
// home screen only. The iOS lock-screen pair has no counterpart to build.

private const val SUPPORTED_VERSION = 1

/** Rows drawn. The rest stay in the snapshot and move up as the day advances. */
private const val MAX_ROWS = 5

class AperioWidget : GlanceAppWidget() {
  override suspend fun provideGlance(context: Context, id: GlanceId) {
    // Read here, not in the composable: `provideContent` may recompose, and the
    // file should be opened once per update rather than once per frame.
    val snapshot = WidgetStore.readSnapshot(context)
    val ticked = WidgetStore.pendingTaps(context)
    // The fallback wording is resolved HERE, where a Context is in hand, rather
    // than reached for from inside the composition.
    val fallback = context.getString(R.string.aperio_widget_no_data)
    provideContent { Body(snapshot, ticked, Date(), fallback) }
  }
}

@Composable
private fun Body(
  snapshot: JSONObject?,
  ticked: Set<String>,
  now: Date,
  fallback: String,
) {
  val usable = snapshot?.takeIf { it.optInt("version") == SUPPORTED_VERSION }
  val strings = usable?.optJSONObject("strings")
  val locale = WidgetStore.localeFor(usable?.optString("locale"))
  val rows = usable?.let { visibleItems(it, ticked, now) } ?: emptyList()

  Column(modifier = GlanceModifier.fillMaxWidth().padding(8.dp)) {
    if (rows.isEmpty()) {
      // "Nothing planned" and "I have no current data" are different facts and
      // must never render the same way.
      val exhausted = usable == null || isExhausted(usable, now)
      val key = if (exhausted) "stale" else "empty"
      val message = strings?.optString(key)?.takeIf { it.isNotEmpty() } ?: fallback
      Text(text = message, modifier = GlanceModifier.semantics { contentDescription = message })
    } else {
      for (item in rows) {
        ItemRow(item, strings, locale)
        Spacer(modifier = GlanceModifier.size(4.dp))
      }
    }
  }
}

@Composable
private fun ItemRow(item: JSONObject, strings: JSONObject?, locale: Locale) {
  val title = item.optString("title")
  val whenParts = whenParts(item, strings, locale)
  // The whole row as one sentence, ending with what the row IS or what state it
  // is in — the same wording VoiceOver reads on iOS.
  val spoken = (listOf(title) + whenParts + listOf(kindWord(item, strings))).joinToString(", ")

  if (item.optBoolean("completable", false)) {
    // The row IS the checkbox: one element carrying content, role and state,
    // rather than a label with a control beside it.
    //
    // `checked` is always false — a completed task is not in the snapshot, and
    // one just ticked is filtered out by the pending-tap overlay. There is no
    // second copy of the truth here to fall out of step.
    //
    // A checkbox cannot draw a third state, so a task already underway says so
    // in its VISIBLE text. Spoken, both states are named; drawn, only the
    // exception is, because an unticked box already reads as "not started" to
    // anyone who can see it.
    val visible = (listOf(title) + whenParts + inProgressSuffix(item, strings))
      .filter { it.isNotEmpty() }
      .joinToString(" · ")
    CheckBox(
      checked = false,
      onCheckedChange = actionRunCallback<ToggleTaskAction>(
        actionParametersOf(
          ToggleTaskAction.itemId to item.optString("id"),
          ToggleTaskAction.containerId to item.optString("containerId"),
        ),
      ),
      text = visible,
      modifier = GlanceModifier.fillMaxWidth().semantics { contentDescription = spoken },
    )
  } else {
    Row(
      verticalAlignment = Alignment.CenterVertically,
      modifier = GlanceModifier.fillMaxWidth().semantics { contentDescription = spoken },
    ) {
      Image(
        provider = ImageProvider(iconFor(item)),
        // Described by the row, not by itself — the sentence above already says
        // in words what this glyph says in a picture.
        contentDescription = null,
        modifier = GlanceModifier.size(12.dp),
      )
      Spacer(modifier = GlanceModifier.width(6.dp))
      // No explicit colour: the widget's default text colour already follows
      // the launcher's light/dark rendering, and overriding it would mean
      // pulling in the separate glance-material3 artifact to name one.
      Text(text = (listOf(title) + whenParts).joinToString(" · "))
    }
  }
}

/** Only for a task already underway; open is left to the unticked box. */
private fun inProgressSuffix(item: JSONObject, strings: JSONObject?): List<String> =
  if (item.optString("status") == "in_progress") {
    listOfNotNull(strings?.optString("statusInProgress")?.takeIf { it.isNotEmpty() })
  } else {
    emptyList()
  }

/** "When", in the order it reads — the twin of the Swift `whenParts`. */
private fun whenParts(item: JSONObject, strings: JSONObject?, locale: Locale): List<String> {
  val at = WidgetStore.parseInstant(item.optString("at", null)) ?: return emptyList()
  val untimed = item.optBoolean("untimed", false)
  val isEvent = item.optString("kind") == "event"
  val parts = mutableListOf<String>()
  val day = WidgetStore.dayText(at, locale)
  if (day != null) {
    parts.add(day)
  } else if (untimed && !isEvent) {
    strings?.optString("today")?.takeIf { it.isNotEmpty() }?.let { parts.add(it) }
  }
  if (!untimed) {
    parts.add(WidgetStore.timeText(at, locale))
  } else if (isEvent) {
    strings?.optString("allDay")?.takeIf { it.isNotEmpty() }?.let { parts.add(it) }
  }
  return parts.filter { it.isNotEmpty() }
}

/** What an event IS, or what state a task is in — a state implies the kind, and
 *  the kind never implied the state. */
private fun kindWord(item: JSONObject, strings: JSONObject?): String {
  if (strings == null) return ""
  if (item.optString("kind") == "event") return strings.optString("kindEvent")
  return when (item.optString("status")) {
    "in_progress" -> strings.optString("statusInProgress")
    "open" -> strings.optString("statusOpen")
    else -> strings.optString("kindTask")
  }
}

private fun iconFor(item: JSONObject): Int =
  if (item.optString("kind") == "event") {
    R.drawable.aperio_widget_event
  } else {
    // A read-only projection — a future occurrence of a recurring task — gets
    // the recurrence marker. It is also why it is not a checkbox: a box that
    // cannot be ticked is a control that lies.
    R.drawable.aperio_widget_recurring
  }

private fun visibleItems(snapshot: JSONObject, ticked: Set<String>, now: Date): List<JSONObject> {
  val items = snapshot.optJSONArray("items") ?: return emptyList()
  val out = mutableListOf<JSONObject>()
  for (i in 0 until items.length()) {
    val item = items.optJSONObject(i) ?: continue
    val expiry = WidgetStore.expiresAt(item) ?: continue
    if (!expiry.after(now)) continue
    if (ticked.contains(item.optString("id"))) continue
    out.add(item)
    if (out.size == MAX_ROWS) break
  }
  return out
}

private fun isExhausted(snapshot: JSONObject, now: Date): Boolean {
  val horizon = WidgetStore.parseInstant(snapshot.optString("horizonEnd", null)) ?: return true
  return !now.before(horizon)
}

/**
 * A tap on a row's checkbox.
 *
 * Queues the request and stops. Completing a task cascades to parents and
 * children, self-assigns on shared lists, advances a recurring series, appends
 * to the event log and queues a sync push — the app's rules over the Rust core,
 * and a widget callback has none of them. The app drains this on its next run,
 * through the same check-off path a tap in the app takes.
 */
class ToggleTaskAction : ActionCallback {
  override suspend fun onAction(
    context: Context,
    glanceId: GlanceId,
    parameters: ActionParameters,
  ) {
    val id = parameters[itemId] ?: return
    WidgetStore.enqueue(context, id, parameters[containerId] ?: "")
    // Redraw now so the row disappears under the finger, rather than whenever
    // the app next happens to run.
    AperioWidget().update(context, glanceId)
  }

  companion object {
    val itemId = ActionParameters.Key<String>("itemId")
    val containerId = ActionParameters.Key<String>("containerId")
  }
}

class AperioWidgetReceiver : GlanceAppWidgetReceiver() {
  override val glanceAppWidget: GlanceAppWidget = AperioWidget()
}
