//! Macros that emit the FFI boilerplate plugin authors would
//! otherwise have to hand-roll.
//!
//! ## What's here
//!
//! - [`declare_lifecycle!`] — emits the two required exports
//!   (`aperio_plugin_create` / `aperio_plugin_destroy`) plus a
//!   `static AperioPlugin` descriptor pointing at the user-
//!   supplied metadata + vtable + open/close hooks.
//!
//! ## What's NOT here
//!
//! A "full" `aperio_plugin_export!` that derives the vtable
//! straight from a struct's trait impls. That requires either a
//! proc-macro (separate crate + `syn`/`quote`/`proc-macro2`) or
//! a 400-line `macro_rules!` arm per trait — both add maintenance
//! burden out of proportion with the workspace size. The
//! existing bundled plugins hand-roll the vtable using
//! [`crate::response`] + [`crate::decode_args`]; if that
//! boilerplate ever gets painful we add a dedicated proc-macro
//! crate in its own phase.
//!
//! The vtable is parameter-typed: any of
//! [`plugin_core::CalendarVtable`], [`plugin_core::TasksVtable`],
//! [`plugin_core::ContactsVtable`], [`plugin_core::SyncVtable`],
//! or the outer [`plugin_core::CalendarAdapterVtable`] wrapper
//! works as long as the `&'static` reference outlives the
//! plugin (which is trivially true when it's a `static` in the
//! plugin crate).

/// Emit the two required FFI exports (`aperio_plugin_create`,
/// `aperio_plugin_destroy`) plus a `static AperioPlugin`
/// descriptor that the host's
/// [`plugin_core::manager::PluginManager`] reads at load time.
///
/// Arguments (named-call shape — order doesn't matter):
///   - `id` — UTF-8 + NUL-terminated reverse-DNS id. Must match
///     the `id` field of the sibling `plugin.json` manifest.
///   - `name` — display name; the Settings → Plugins UI shows
///     this verbatim.
///   - `version` — SemVer string matching `plugin.json`.
///   - `plugin_type` — the kebab-case tag from §20.2.
///   - `vtable` — path to a `static` of one of the vtable
///     structs from [`plugin_core::vtables`]. The macro casts
///     it to `*mut c_void` so the host can pull the typed
///     pointer back out via its `plugin_type` discriminator.
///   - `open_instance` — path to a
///     `unsafe extern "C" fn(*const c_char) -> OpenInstanceResult`
///     the host fires once per account / connection. Use the
///     literal token `none` for process-global plugins that
///     don't carry per-instance state — the host then dispatches
///     vtable methods with a NULL instance handle.
///   - `close_instance` — counterpart for the teardown side,
///     `unsafe extern "C" fn(*mut c_void)`. `none` to skip
///     (only valid when `open_instance` is also `none`).
///
/// ## Memory ownership
///
/// All strings cross the FFI boundary as `&'static CStr` whose
/// backing storage lives in the plugin's `static` data. The
/// host MUST NOT free them — the plugin's own `dlclose` (run by
/// the host's [`plugin_core::manager::LoadedPlugin::drop`])
/// reclaims everything when the library unloads.
///
/// ## Example
///
/// ```ignore
/// use plugin_sdk::declare_lifecycle;
///
/// static CALENDAR_VTABLE: plugin_core::CalendarVtable = /* … */;
///
/// declare_lifecycle! {
///     id: "com.aperio.cal-adapter-local",
///     name: "Aperio Local",
///     version: "0.1.0",
///     plugin_type: "calendar-adapter",
///     vtable: CALENDAR_VTABLE,
///     open_instance: plugin_open_instance,
///     close_instance: plugin_close_instance,
/// }
/// ```
#[macro_export]
macro_rules! declare_lifecycle {
    (
        id: $id:literal,
        name: $name:literal,
        version: $version:literal,
        plugin_type: $plugin_type:literal,
        vtable: $vtable:path,
        open_instance: $open:tt,
        close_instance: $close:tt $(,)?
    ) => {
        // C-string literals give us &'static CStr, which
        // `.as_ptr()` turns into a stable `*const c_char` that
        // satisfies the ABI's "lives until aperio_plugin_destroy
        // returns" contract — the literal's storage is part of
        // the binary's .rodata.
        const __APERIO_PLUGIN_ID: &::std::ffi::CStr =
            match ::std::ffi::CStr::from_bytes_with_nul(concat!($id, "\0").as_bytes()) {
                Ok(s) => s,
                Err(_) => panic!("plugin id must not contain interior NUL"),
            };
        const __APERIO_PLUGIN_NAME: &::std::ffi::CStr =
            match ::std::ffi::CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes()) {
                Ok(s) => s,
                Err(_) => panic!("plugin name must not contain interior NUL"),
            };
        const __APERIO_PLUGIN_VERSION: &::std::ffi::CStr =
            match ::std::ffi::CStr::from_bytes_with_nul(concat!($version, "\0").as_bytes()) {
                Ok(s) => s,
                Err(_) => panic!("plugin version must not contain interior NUL"),
            };
        const __APERIO_PLUGIN_TYPE: &::std::ffi::CStr =
            match ::std::ffi::CStr::from_bytes_with_nul(concat!($plugin_type, "\0").as_bytes()) {
                Ok(s) => s,
                Err(_) => panic!("plugin_type must not contain interior NUL"),
            };

        /// `aperio_plugin_create` per DESIGN.md §20.3.
        ///
        /// # Safety
        ///
        /// FFI export. Called once by the host immediately after
        /// `dlopen`. No precondition on host state.
        #[no_mangle]
        pub unsafe extern "C" fn aperio_plugin_create() -> *mut $crate::plugin_core::AperioPlugin {
            let descriptor = $crate::plugin_core::AperioPlugin {
                abi_version: $crate::plugin_core::ABI_VERSION,
                id: __APERIO_PLUGIN_ID.as_ptr(),
                name: __APERIO_PLUGIN_NAME.as_ptr(),
                version: __APERIO_PLUGIN_VERSION.as_ptr(),
                plugin_type: __APERIO_PLUGIN_TYPE.as_ptr(),
                open_instance: $crate::declare_lifecycle!(@open $open),
                close_instance: $crate::declare_lifecycle!(@close $close),
                vtable: &$vtable
                    as *const _
                    as *mut ::std::os::raw::c_void,
            };
            Box::into_raw(Box::new(descriptor))
        }

        /// `aperio_plugin_destroy` per DESIGN.md §20.3.
        ///
        /// # Safety
        ///
        /// `plugin` must be the exact pointer returned by the
        /// preceding `aperio_plugin_create` call, not yet freed.
        /// The host's [`plugin_core::manager::LoadedPlugin::drop`]
        /// guarantees this.
        #[no_mangle]
        pub unsafe extern "C" fn aperio_plugin_destroy(
            plugin: *mut $crate::plugin_core::AperioPlugin,
        ) {
            if plugin.is_null() {
                return;
            }
            let _ = Box::from_raw(plugin);
        }
    };

    // ── Internal helper arms ─────────────────────────────────
    //
    // `open_instance` / `close_instance` accept either a fn path
    // or the literal token `none`. We turn `none` into None and
    // a fn name into Some(fn_name). The differing signatures
    // (open takes config_json + returns OpenInstanceResult,
    // close takes the opaque handle) need two helper arms.
    (@open none) => { None };
    (@open $fn:path) => {
        Some(
            $fn as unsafe extern "C" fn(
                *const ::std::os::raw::c_char,
            ) -> $crate::plugin_core::OpenInstanceResult,
        )
    };
    (@close none) => { None };
    (@close $fn:path) => {
        Some($fn as unsafe extern "C" fn(*mut ::std::os::raw::c_void))
    };
}

#[cfg(test)]
mod tests {
    //! The macro-expansion happens inside tests via a sub-module
    //! that uses the macro to declare a no-op plugin. We then
    //! call the generated entry points + assert the descriptor
    //! shape.
    //!
    //! Note: only one `declare_lifecycle!` invocation can exist
    //! per crate because it emits `#[no_mangle]` symbols. The
    //! test crate is the lib's `#[cfg(test)]` build, so the
    //! macros below don't collide with the lib's main code.

    use plugin_core::vtables::CalendarVtable;
    use std::ffi::CStr;

    static TEST_VTABLE: CalendarVtable = CalendarVtable::empty();

    // SAFETY: the macro's safety story is that the host runs
    // create / destroy in a known sequence. We mimic that here.
    crate::declare_lifecycle! {
        id: "com.aperio.test-plugin",
        name: "Test Plugin",
        version: "0.1.0",
        plugin_type: "calendar-adapter",
        vtable: TEST_VTABLE,
        open_instance: none,
        close_instance: none,
    }

    #[test]
    fn descriptor_carries_expected_metadata() {
        // SAFETY: the create + destroy contract — call once,
        // free at the end of the test.
        let plugin_ptr = unsafe { aperio_plugin_create() };
        assert!(!plugin_ptr.is_null());
        let descriptor = unsafe { &*plugin_ptr };
        assert_eq!(
            descriptor.abi_version,
            plugin_core::ABI_VERSION
        );
        let id = unsafe { CStr::from_ptr(descriptor.id) }
            .to_str()
            .expect("utf8");
        assert_eq!(id, "com.aperio.test-plugin");
        let name = unsafe { CStr::from_ptr(descriptor.name) }
            .to_str()
            .expect("utf8");
        assert_eq!(name, "Test Plugin");
        let version = unsafe { CStr::from_ptr(descriptor.version) }
            .to_str()
            .expect("utf8");
        assert_eq!(version, "0.1.0");
        let ptype = unsafe { CStr::from_ptr(descriptor.plugin_type) }
            .to_str()
            .expect("utf8");
        assert_eq!(ptype, "calendar-adapter");
        assert!(descriptor.open_instance.is_none());
        assert!(descriptor.close_instance.is_none());
        assert!(!descriptor.vtable.is_null());
        // SAFETY: vtable points at our TEST_VTABLE static
        // (CalendarVtable::empty()).
        let vtable: &CalendarVtable = unsafe {
            &*(descriptor.vtable as *const CalendarVtable)
        };
        assert!(vtable.list_calendars.is_none());

        unsafe { aperio_plugin_destroy(plugin_ptr) };
    }

    #[test]
    fn aperio_plugin_destroy_tolerates_null() {
        // The host's manager always passes a non-null ptr, but
        // defensive null-check belongs in the destructor anyway
        // — no UB if a buggy caller passes null.
        unsafe { aperio_plugin_destroy(std::ptr::null_mut()) };
    }
}
