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
#define APERIO_PLUGIN_ABI_VERSION 3u

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
 * string form ("adapter", "notification") — these enum values
 * exist purely as a convenience for C consumers doing
 * strcmp-free dispatch.
 */
typedef enum AperioPluginType {
    APERIO_PLUGIN_TYPE_UNKNOWN      = 0,
    /* Any provider surface. WHICH surfaces is the manifest's
     * `capabilities` array — "calendar", "tasks", "contacts",
     * "sync", "videoconference" — and the matching non-NULL
     * pointers in AperioAdapterVtable. */
    APERIO_PLUGIN_TYPE_ADAPTER      = 1,
    APERIO_PLUGIN_TYPE_NOTIFICATION = 2
} AperioPluginType;

/*
 * Plugin-call status codes. Mirrors the `PLUGIN_CALL_*`
 * constants in `crates/plugin-core/src/ffi.rs`. Returned by
 * vtable methods (per type-specific header) + by
 * `aperio_plugin_interactive_auth`.
 */
#define APERIO_PLUGIN_CALL_OK             0
#define APERIO_PLUGIN_CALL_ERR_UNSUPPORTED 1
#define APERIO_PLUGIN_CALL_ERR_INVALID    2
#define APERIO_PLUGIN_CALL_ERR_AUTH       3
#define APERIO_PLUGIN_CALL_ERR_NETWORK    4
#define APERIO_PLUGIN_CALL_ERR_NOT_FOUND  5
#define APERIO_PLUGIN_CALL_ERR_PROTOCOL   6
#define APERIO_PLUGIN_CALL_ERR_IO         7
#define APERIO_PLUGIN_CALL_ERR_CONFLICT   8
#define APERIO_PLUGIN_CALL_ERR_FORBIDDEN  9
#define APERIO_PLUGIN_CALL_ERR_INTERNAL   10

/*
 * Log-severity levels handed to the `aperio_plugin_set_log` sink.
 * Mirror `tracing::Level` (1 = most severe); the Rust side spells
 * these `LOG_LEVEL_*` in `crates/plugin-core/src/abi.rs`.
 */
#define APERIO_PLUGIN_LOG_LEVEL_ERROR  1
#define APERIO_PLUGIN_LOG_LEVEL_WARN   2
#define APERIO_PLUGIN_LOG_LEVEL_INFO   3
#define APERIO_PLUGIN_LOG_LEVEL_DEBUG  4
#define APERIO_PLUGIN_LOG_LEVEL_TRACE  5

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
/*
 * Standard return type for every plugin call (vtable methods +
 * `aperio_plugin_interactive_auth`).
 *
 * `status` is one of the `APERIO_PLUGIN_CALL_*` codes above.
 * On `APERIO_PLUGIN_CALL_OK` the payload is the JSON-encoded
 * response (empty for void-returning methods). On any non-zero
 * status the payload is a UTF-8 error message.
 */
typedef struct PluginCallResult {
    int32_t            status;
    AperioPluginBytes  payload;
} PluginCallResult;

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

    /* Plugin-type tag string ("adapter", "notification"). The
       enum above mirrors the canonical set.

       Adding a future tag does not require an ABI bump — but note
       what a host that predates the tag actually does today: it
       PARSES the unknown value fine and then REFUSES the load with
       a manifest error, which the Settings UI reports as "this
       plugin's manifest is invalid". That is a misleading
       diagnosis for what is really "this Aperio is too old". Until
       that path grows its own reason code, a plugin introducing a
       new type should set `min_app_version` in plugin.json to the
       release that understands it, so older hosts fail with the
       accurate "update Aperio" message instead.

       Note also that the host does NOT dispatch on this string —
       it selects the vtable cast from the account's adapter kind.
       The tag is what the loader validates and what the Settings
       UI groups by. */
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

/*
 * Host-supplied log sink (see `aperio_plugin_set_log`).
 *
 * `level` is one of the APERIO_PLUGIN_LOG_LEVEL_* constants. `target`
 * and `message` are NUL-terminated UTF-8 valid only for the duration
 * of the call — copy them if you need to retain anything. The
 * callback MUST NOT unwind (throw / panic) across the FFI boundary.
 */
typedef void (*AperioLogFn)(uint8_t level, const char *target, const char *message);

/*
 * Optional: log-sink installer.
 *
 * The host calls this exactly once, right after `aperio_plugin_create`,
 * to hand the plugin a host-side log sink. A plugin that exports it
 * should forward each of its own log events by calling `log` with the
 * severity (an APERIO_PLUGIN_LOG_LEVEL_* value), the event target, and
 * the rendered message, so its diagnostics reach the host log
 * (`data/logs/aperio.log`). The host re-emits forwarded events under
 * the tracing target `aperio::plugin`, preserving the plugin's own
 * target in a `source` field.
 *
 * Each plugin shared library links its own logging stack, so without
 * this its diagnostics never reach the host. MAY be left unexported:
 * plugins built before this ABI addition simply don't forward, and
 * the host treats a missing symbol as "no log forwarding" (a
 * best-effort lookup, never a load error). Under static linking (the
 * mobile build) the plugin already shares the host's logging global,
 * so the host never calls this.
 */
void aperio_plugin_set_log(AperioLogFn log);

/*
 * Status codes returned by an AperioHostChannelFn call.
 *
 * A plugin MAY act on these — retry, give up, back off — but is not
 * obliged to. APERIO_HOST_CHANNEL_ACCEPTED is the only one that
 * promises anything: the host has taken durable responsibility for
 * what was reported.
 */
#define APERIO_HOST_CHANNEL_ACCEPTED      0
#define APERIO_HOST_CHANNEL_UNKNOWN_KIND  1  /* older host, newer plugin  */
#define APERIO_HOST_CHANNEL_UNKNOWN_SCOPE 2  /* no such live account      */
#define APERIO_HOST_CHANNEL_REFUSED       3  /* understood, declined      */
#define APERIO_HOST_CHANNEL_FAILED        4  /* attempted, did not land   */
#define APERIO_HOST_CHANNEL_MALFORMED     5  /* envelope unreadable       */
#define APERIO_HOST_CHANNEL_THROTTLED     6  /* too much, too fast        */

/*
 * Host-supplied sink for reports the host did not ask for.
 *
 * Vtable calls run host→plugin and return data. Nothing in that shape
 * lets a plugin say "by the way, the credential I hold has changed" —
 * which is exactly what an OAuth provider that rotates tokens forces an
 * adapter to say. This is the only channel back other than the log sink.
 *
 * `json_ptr` / `json_len` describe a plugin-owned UTF-8 JSON object,
 * valid only for the duration of the call. Pointer-plus-length rather
 * than a NUL-terminated string, matching every other JSON-carrying
 * boundary in this ABI. MUST NOT unwind across the boundary.
 *
 * The envelope is:
 *
 *   { "v": 1, "scope": "<token>", "kind": "<string>", "payload": { … } }
 *
 * `scope` is the opaque capability token the host placed in this
 * instance's `open_instance` config under the reserved key
 * `__aperio_host_token`. It is how the host knows WHICH account is
 * speaking: every plugin in the process shares an address space, so an
 * account id a plugin merely asserts is one it could have invented. The
 * token is random and unguessable for that reason, and it never appears
 * in the persisted account row.
 *
 * `kind` is an open string rather than an enum, so a later signal costs
 * no ABI change; a host that does not know one answers
 * APERIO_HOST_CHANNEL_UNKNOWN_KIND and carries on. The kind defined
 * today is "credential.rotated", whose payload is
 * { "slot": "refresh_token" | "access_token" | "api_token",
 *   "value": "…", "expires_at": "<RFC 3339>" (optional) }.
 *
 * Envelopes larger than 64 KiB are refused before allocation.
 */
typedef int (*AperioHostChannelFn)(const uint8_t *json_ptr, size_t json_len);

/*
 * OPTIONAL export. The host calls it once, right after
 * `aperio_plugin_create`, to hand the plugin its channel. A plugin that
 * never reports anything simply does not export it; a missing symbol is
 * not an error and does not block loading.
 */
void aperio_plugin_set_host_channel(AperioHostChannelFn sink);

/*
 * Optional: interactive authentication entry point.
 *
 * Plugins that need a setup step the user has to drive through
 * (OAuth consent screen, SAML form, …) expose this symbol in
 * addition to the lifecycle exports. Plugins that don't —
 * CalDAV with Basic auth, an iCal feed, etc. — leave it
 * unexported and the host's PluginManager surfaces
 * `InteractiveAuthError::Unsupported` for any call against
 * them.
 *
 * `args_json` carries whatever setup data the host has at the
 * time it triggers the dance — for OAuth that's typically
 * `{"client_id": "...", "client_secret": "..."}`. The plugin
 * runs the dance to completion (opening a browser, listening
 * on a loopback port, exchanging the code, …) and returns
 * the resulting credential blob as the PluginCallResult's
 * payload. The host stores the blob opaquely in its keychain
 * and threads it back into `open_instance` later.
 *
 * Returning `APERIO_PLUGIN_CALL_ERR_AUTH` (or any other non-OK
 * status) surfaces the plugin's payload bytes verbatim to the
 * user as the error message, so OAuth-specific errors
 * (revoked consent, timeout, network) keep their actionable
 * text.
 *
 * The function blocks for the duration of the dance (up to
 * several minutes — the user might walk away from the consent
 * screen). The host invokes it inside its own
 * `tokio::task::spawn_blocking` so the reactor stays free.
 */
PluginCallResult aperio_plugin_interactive_auth(
    const uint8_t *args_ptr,
    size_t         args_len
);

/*
 * Optional: service-discovery entry point.
 *
 * Plugins that own a service-discovery protocol (EWS
 * Autodiscover today; CalDAV well-known URIs / Microsoft Graph
 * endpoint probing are candidates for later) expose this
 * symbol in addition to the lifecycle exports. Plugins that
 * don't — most — leave it unexported and the host's
 * PluginManager surfaces `DiscoverError::Unsupported` for any
 * call against them.
 *
 * `args_json` carries whatever inputs the discovery cascade
 * needs — for EWS that's typically `{"email": "...",
 * "password": "..."}`. The plugin runs the cascade to
 * completion and returns the resolved endpoint(s) as a JSON
 * document in the PluginCallResult's payload. The host parses
 * the JSON into its UI-facing shape.
 *
 * Returning `APERIO_PLUGIN_CALL_ERR_NOT_FOUND` (or any other
 * non-OK status) surfaces the plugin's payload bytes verbatim
 * as the error message, so discovery-specific errors keep their
 * actionable text ("no endpoint for hs-anhalt.de",
 * "Autodiscover HTTP 401", …).
 *
 * The function may block for several seconds while the cascade
 * runs (each probe is one HTTP request). The host invokes it
 * inside its own `tokio::task::spawn_blocking` so the reactor
 * stays free.
 */
PluginCallResult aperio_plugin_discover(
    const uint8_t *args_ptr,
    size_t         args_len
);

/*
 * Optional: TOFU-transport host-key probe entry point.
 *
 * Plugins wrapping a transport that pins server identity at
 * first use (SFTP today; potentially MQTT-over-TLS or similar
 * later) expose this so the host's trust dialog can read the
 * presented fingerprint *without* committing the pin or even
 * authenticating. Plugins that don't — most — leave the symbol
 * unexported and the host's PluginManager surfaces
 * `ProbeHostKeyError::Unsupported` for any call against them.
 *
 * `args_json` carries the connection inputs — for SFTP that's
 * typically `{"host": "...", "port": 22}`. The plugin opens a
 * single connection, captures the server's host-key
 * fingerprint, drops the connection without authenticating, +
 * returns the fingerprint as a JSON document
 * (`{"fingerprint": "SHA256:..."}`) in the PluginCallResult's
 * payload. The host compares the fingerprint against its own
 * pinned-key store (kept device-local in user_prefs) and
 * renders the "first use" / "key changed" / "unchanged" trust
 * dialog accordingly.
 *
 * Returning `APERIO_PLUGIN_CALL_ERR_NETWORK` (or any other
 * non-OK status) surfaces the plugin's payload bytes verbatim
 * as the error message ("connection refused", "host
 * unreachable", …).
 *
 * The function may block for several seconds waiting on TCP
 * connect / SSH handshake. The host invokes it inside its own
 * `tokio::task::spawn_blocking` so the reactor stays free.
 */
PluginCallResult aperio_plugin_probe_host_key(
    const uint8_t *args_ptr,
    size_t         args_len
);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* APERIO_PLUGIN_H */
