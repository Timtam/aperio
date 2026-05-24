/*
 * aperio_plugin.h — Aperio plugin ABI (DESIGN.md §20.3).
 *
 * This header is the canonical contract between Aperio and any
 * shared library that wants to act as a plugin. Plugins MAY be
 * written in any language that can produce a C-ABI compatible
 * dynamic library (Rust, C, C++, Zig, Go, Swift, …); the Rust
 * `plugin-sdk` crate is a convenience wrapper, not a requirement.
 *
 * Stability rules
 * ───────────────
 * - This header is versioned by `APERIO_PLUGIN_ABI_VERSION`. Aperio
 *   refuses to load plugins whose `abi_version` field doesn't equal
 *   the host's. Bumps to the constant are breaking changes and ship
 *   with release notes describing the migration path.
 * - All struct layouts here MUST stay binary-compatible within one
 *   ABI version. Adding new fields requires a new version bump.
 * - Type-specific vtables (CalendarVtable, SyncVtable, …) are NOT
 *   defined in this P0 header — they land in the next plugin-core
 *   phase. P0 lays down only the lifecycle and metadata surface so
 *   community plugin authors can already declare their `plugin.json`
 *   manifest and the C entry points, and we can wire up the host
 *   manager around them.
 *
 * Plugins MUST be safe to load and call concurrently from multiple
 * Aperio threads. Each plugin instance is a singleton inside the
 * host process; lifecycle is `aperio_plugin_create` → optional
 * `init` → many feature calls → `destroy` → `aperio_plugin_destroy`.
 */

#ifndef APERIO_PLUGIN_H
#define APERIO_PLUGIN_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/*
 * ABI version supported by this header.
 *
 * Plugins MUST emit this same value in their `AperioPlugin.abi_version`
 * field. A mismatch is a load-time fatal error — Aperio surfaces it
 * to the user as "Plugin XY needs a newer/older Aperio" so they know
 * to update one side or the other.
 *
 * v1 = initial release.
 */
#define APERIO_PLUGIN_ABI_VERSION 1u

/*
 * Lifecycle return codes.
 *
 * Returned by `init`. Non-zero is a load-time fatal error and the
 * plugin will be torn back down via `destroy` (if non-NULL) and
 * unloaded. The host surfaces the code + the optional
 * `last_error_message` from the plugin's vtable to the user.
 */
#define APERIO_PLUGIN_OK                  0
#define APERIO_PLUGIN_ERR_INIT            1   /* generic init failure */
#define APERIO_PLUGIN_ERR_INVALID_CONFIG  2   /* config_json was malformed */
#define APERIO_PLUGIN_ERR_INTERNAL        3   /* unrecoverable internal error */

/*
 * Plugin-type tag. Mirrors DESIGN.md §20.2.
 *
 * The `plugin_type` field on AperioPlugin carries the lowercase
 * string form ("calendar-adapter", "sync-adapter", …) — these
 * enum values exist purely as a convenience for C consumers
 * doing strcmp-free dispatch.
 */
typedef enum AperioPluginType {
    APERIO_PLUGIN_TYPE_UNKNOWN              = 0,
    APERIO_PLUGIN_TYPE_CALENDAR_ADAPTER     = 1,
    APERIO_PLUGIN_TYPE_SYNC_ADAPTER         = 2,
    APERIO_PLUGIN_TYPE_VIDEOCONFERENCE_ADAPTER = 3,
    APERIO_PLUGIN_TYPE_NOTIFICATION         = 4
} AperioPluginType;

/*
 * Plugin descriptor — returned by `aperio_plugin_create`.
 *
 * Memory ownership: every pointer field is owned by the plugin and
 * remains valid until `aperio_plugin_destroy` returns. Strings are
 * NUL-terminated UTF-8. The host MUST NOT free any of them.
 *
 * Layout MUST stay binary-compatible across plugin-core 0.x patch
 * versions. Adding fields requires bumping APERIO_PLUGIN_ABI_VERSION.
 */
typedef struct AperioPlugin {
    /* ABI version emitted by the plugin (compare against
       APERIO_PLUGIN_ABI_VERSION; mismatch → refuse to load). */
    uint32_t abi_version;

    /* Stable id, e.g. "com.aperio.cal-adapter-local". MUST match
       the `id` field of plugin.json. */
    const char *id;

    /* Human-readable display name (already localised by the plugin
       if it cares). */
    const char *name;

    /* SemVer string. */
    const char *version;

    /* Plugin-type tag string ("calendar-adapter", "sync-adapter",
       …). The enum above mirrors the canonical set; future tags
       can be added without bumping ABI as long as the host
       gracefully degrades on unknown values. */
    const char *plugin_type;

    /*
     * Optional lifecycle hook. Called once before any feature-vtable
     * methods. `config_json` is a NUL-terminated UTF-8 string holding
     * the plugin's persisted user_prefs config (may be NULL or empty
     * on the first run). Returns one of the APERIO_PLUGIN_OK / _ERR_*
     * codes above.
     *
     * MAY be NULL — pure feature-vtable plugins that don't need
     * deferred init can skip it.
     */
    int32_t (*init)(const char *config_json);

    /*
     * Optional teardown hook. Called once after the last feature-
     * vtable method returns, just before `aperio_plugin_destroy`.
     * MUST release any resources the plugin acquired during `init`
     * or its feature work.
     *
     * MAY be NULL.
     */
    void (*destroy)(void);

    /*
     * Type-specific vtable. The host casts it to the right struct
     * pointer based on `plugin_type`. The concrete vtable layouts
     * are defined in a separate header that ships with the next
     * plugin-core phase; P0 leaves this slot opaque so we can lock
     * down the lifecycle surface first without committing to the
     * per-feature method shape.
     */
    void *vtable;
} AperioPlugin;

/*
 * Required exports from every plugin shared library.
 *
 * `aperio_plugin_create` returns a pointer to a singleton AperioPlugin
 * the host will use for the lifetime of the load. Returning NULL is
 * a load-time fatal error.
 *
 * `aperio_plugin_destroy` releases the descriptor and any tail-end
 * resources. The host calls it exactly once, after `AperioPlugin.destroy`
 * (if present) has already run.
 */
AperioPlugin *aperio_plugin_create(void);
void          aperio_plugin_destroy(AperioPlugin *plugin);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* APERIO_PLUGIN_H */
