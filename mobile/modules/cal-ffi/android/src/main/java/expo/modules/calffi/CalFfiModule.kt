package expo.modules.calffi

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition
import java.io.File
import java.time.Instant
import uniffi.cal_ffi.LocalStore
import uniffi.cal_ffi.NewTaskDto
import uniffi.cal_ffi.TaskDto
import uniffi.cal_ffi.TaskListDto
import uniffi.cal_ffi.TaskPriority
import uniffi.cal_ffi.TaskStatus
import uniffi.cal_ffi.parseAttendee as uniffiParseAttendee

class CalFfiModule : Module() {
  // The on-device store, opened lazily at the app-private sandbox path
  // (`<filesDir>/aperio.sqlite`). `by lazy` is SYNCHRONIZED, so the concurrent
  // background threads `AsyncFunction` dispatches on share one handle safely;
  // the Rust store serialises its own SQLite access behind a mutex. Opening
  // also runs the shared `aperio-db` migrations (same schema as desktop).
  private val store: LocalStore by lazy {
    val filesDir = appContext.reactContext?.applicationContext?.filesDir
      ?: throw IllegalStateException(
        "CalFfi: no Android application context to resolve the data directory",
      )
    LocalStore.open(File(filesDir, "aperio.sqlite").absolutePath)
  }

  override fun definition() = ModuleDefinition {
    Name("CalFfi")

    // Calls the Rust `cal_ffi::parse_attendee` through the UniFFI-generated
    // Kotlin bindings (uniffi/cal_ffi/cal_ffi.kt), backed by libcal_ffi.so.
    // This is the engine-reuse boundary: cal-core's parser runs in-process.
    Function("parseAttendee") { entry: String ->
      val parsed = uniffiParseAttendee(entry)
      mapOf(
        "name" to parsed.name,
        "email" to parsed.email,
      )
    }

    // ─── Task lists ───────────────────────────────────────────────────────
    // All store operations are `AsyncFunction`s: Expo dispatches them off the
    // JS thread, and a thrown `StoreException` rejects the JS promise.

    AsyncFunction("taskLists") {
      store.taskLists().map { taskListToMap(it) }
    }

    AsyncFunction("createTaskList") { name: String ->
      taskListToMap(store.createTaskList(name))
    }

    AsyncFunction("renameTaskList") { id: String, name: String ->
      store.renameTaskList(id, name)
    }

    AsyncFunction("deleteTaskList") { id: String ->
      store.deleteTaskList(id)
    }

    // ─── Tasks ────────────────────────────────────────────────────────────

    AsyncFunction("tasks") { listId: String ->
      store.tasks(listId).map { taskToMap(it) }
    }

    AsyncFunction("createTask") { listId: String, title: String, description: String?, scheduledDate: String? ->
      val task = NewTaskDto(
        title = title,
        description = description,
        status = TaskStatus.OPEN,
        priority = TaskPriority.MEDIUM,
        scheduledDate = scheduledDate,
        scheduledTime = null,
        deadlineDate = null,
        deadlineTime = null,
        recurrence = null,
        parentId = null,
        sectionId = null,
        colorLabel = null,
        reminders = emptyList(),
        sound = null,
      )
      taskToMap(store.createTask(listId, task))
    }

    // Complete / reopen via read-modify-write: fetch the row, flip the status
    // (stamping `completed_at` on completion), write it back. The generated
    // DTO is a data class, so `.copy()` preserves every other field — keeping
    // the round-trip lossless without surfacing the full task to JS.
    AsyncFunction("setTaskDone") { taskId: String, done: Boolean ->
      val current = store.task(taskId)
      val updated = current.copy(
        status = if (done) TaskStatus.COMPLETED else TaskStatus.OPEN,
        completedAt = if (done) Instant.now().toString() else null,
      )
      taskToMap(store.updateTask(updated))
    }

    AsyncFunction("renameTask") { taskId: String, title: String ->
      val current = store.task(taskId)
      taskToMap(store.updateTask(current.copy(title = title)))
    }

    AsyncFunction("rescheduleTask") { taskId: String, scheduledDate: String? ->
      val current = store.task(taskId)
      taskToMap(store.updateTask(current.copy(scheduledDate = scheduledDate)))
    }

    AsyncFunction("deleteTask") { taskId: String ->
      store.deleteTask(taskId)
    }
  }
}

private fun taskListToMap(list: TaskListDto): Map<String, Any?> = mapOf(
  "id" to list.id,
  "name" to list.name,
  "parentId" to list.parentId,
  "readOnly" to list.readOnly,
)

// The reduced view the UI consumes. The full lossless `TaskDto` stays
// Rust-side / Kotlin-side; JS only needs these fields to render and act.
private fun taskToMap(task: TaskDto): Map<String, Any?> = mapOf(
  "id" to task.id,
  "listId" to task.listId,
  "title" to task.title,
  "description" to task.description,
  "done" to (task.status == TaskStatus.COMPLETED),
  "status" to task.status.name.lowercase(),
  "scheduledDate" to task.scheduledDate,
  "deadlineDate" to task.deadlineDate,
  "hasRecurrence" to (task.recurrence != null),
  "completedAt" to task.completedAt,
)
