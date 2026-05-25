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
 *
 * Library vs. instance lifecycle (v2)
 * ───────────────────────────────────
 * The descriptor returned by `aperio_plugin_create` is a process
 * singleton — one per loaded shared library. Per-account /
 * per-server adapter instances are opened on top of that via the
 * descriptor's `open_instance` + `close_instance` hooks, which a
 * single library may invoke arbitrarily often (see DESIGN.md §6.4
 * — multiple Google accounts, multiple CalDAV servers, …). Every
 * vtable method takes the opaque instance handle returned by
 * `open_instance` as its first argument, so the plugin can route
 * work to the right per-account state.
 *
 * Plugins MUST be safe to load and call concurrently from multiple
 * Aperio threads — the host doesn't serialise calls across
 * different instances (or across different methods on the same
 * instance).
 */

#ifndef APERIO_PLUGIN_H
#define APERIO_PLUGIN_H

#include <stddef.h>
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
 * History
 * - v1: initial release; one process-singleton instance per loaded
 *       library; descriptor carried `init(config_json)` and
 *       `destroy()` hooks.
 * - v2: instance handles. Descriptor lost `init`/`destroy`, gained
 *       `open_instance` / `close_instance`. Every vtable method now
 *       takes the opaque instance handle as its first argument.
 */
#define APERIO_PLUGIN_ABI_VERSION 2u

/*
 * Lifecycle return codes.
 *
 * Returned in the `status` field of `OpenInstanceResult`. Non-zero
 * is a load-time failure for that instance and the host surfaces
 * the code + the optional error message in the result to the user.
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
 * Plugin-owned byte buffer crossing the FFI boundary. Used both
 * for `OpenInstanceResult.error` and as the payload field on
 * every vtable call result.
 *
 * Memory ownership: `data` is allocated by the plugin's allocator.
 * The host MUST call `free(data, len)` after copying the bytes —
 * typically immediately after extracting the message / decoding
 * the payload. The double-pointer pattern (data + free function
 * pointer) avoids any assumption that the host and plugin share
 * an allocator.
 *
 * A struct with `data == NULL`, `len == 0`, `free == NULL` is the
 * "no payload" sentinel.
 */
typedef struct AperioPluginBytes {
    uint8_t *data;
    size_t   len;
    /* Releases `data` back to the plugin's allocator. MAY be NULL
       when `data == NULL` — a status-only response doesn't need a
       free function. */
    void   (*free)(uint8_t *data, size_t len);
} AperioPluginBytes;

/*
 * Return value of `AperioPlugin.open_instance`. Either carries a
 * non-NULL `instance` handle with `status == APERIO_PLUGIN_OK`,
 * or a NULL handle with a non-OK status + an optional UTF-8
 * error message in `error`.
 *
 * The plugin owns the bytes in `error` and the host releases
 * them via `error.free` after copying the message out.
 */
typedef struct OpenInstanceResult {
    /* Opaque per-instance handle. NULL on error. The host stores
       it and passes it back to every vtable method as the first
       argument; on shutdown the host calls
       `close_instance(handle)` to release it. */
    void              *instance;
    /* APERIO_PLUGIN_OK on success, or one of the APERIO_PLUGIN_ERR_*
       codes on failure. */
    int32_t            status;
    /* Optional plugin-owned UTF-8 error detail. Empty on success.
       Released by the host after extraction. */
    AperioPluginBytes  error;
} OpenInstanceResult;

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
     * Open a new instance of the adapter (per account / per server).
     * `config_json` is a NUL-terminated UTF-8 pointer (may be NULL
     * or empty for instance-less plugins). The host calls this
     * once per account it wants to wire up; a single loaded
     * library may have N live instances at the same time
     * (DESIGN.md §6.4).
     *
     * MAY be NULL for plugins that don't carry per-account state
     * — the host then dispatches vtable methods with a NULL
     * instance handle. Notification channels and other process-
     * global plugins are the typical case.
     */
    OpenInstanceResult (*open_instance)(const char *config_json);

    /*
     * Release an instance previously returned by `open_instance`.
     * Called by the host when the corresponding account is
     * removed or the app is shutting down.
     *
     * MAY be NULL iff `open_instance` is also NULL.
     */
    void (*close_instance)(void *instance);

    /*
     * Type-specific vtable. The host casts it to the right struct
     * pointer based on `plugin_type`. The concrete vtable layouts
     * are defined in `aperio_plugin_vtables.h` (one struct per
     * plugin type); every method on every vtable takes
     * `void *instance` as its first argument so the plugin can
     * route work to the right per-account state.
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
 * resources. The host calls it exactly once, after every instance
 * opened via `open_instance` has already been released via
 * `close_instance`.
 */
AperioPlugin *aperio_plugin_create(void);
void          aperio_plugin_destroy(AperioPlugin *plugin);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* APERIO_PLUGIN_H */
