//! `FfiCalendarAdapter` — `cal_core::CalendarFeature` impl that
//! dispatches every method across the FFI boundary into a loaded
//! plugin's [`crate::vtables::CalendarVtable`].
//!
//! Canonical pattern for the other three shims (Tasks /
//! Contacts / Sync): hold the [`LoadedInstance`] Arc (which in
//! turn keeps the loaded plugin alive), cache the per-account
//! handle, and implement the trait by going through
//! `encode_args` → `call_method` → `decode_payload`.

use std::sync::Arc;

use async_trait::async_trait;
use cal_core::adapter::{Adapter, AuthToken, CalendarFeature, Capability, ChangeSet, Credentials};
use cal_core::color::ContainerColor;
use cal_core::error::{Error, Result};
use cal_core::types::{AttendeeStatus, Calendar, DateRange, Event, FreeBusy, NewEvent};
use serde::Serialize;
use tracing::warn;

use crate::ffi::*;
use crate::manager::{InFlightGuard, LoadedInstance};
use crate::vtables::CalendarVtable;

use super::call::{call_method, call_method_sync, decode_payload, encode_args, CallOutcome};

/// Holds a loaded calendar-adapter plugin instance + a snapshot
/// of the methods we expect on its vtable. The vtable pointer is
/// resolved once (in [`Self::new`]) so every method call avoids
/// the type-cast each time.
///
/// `Arc<LoadedInstance>` keeps both the per-account state in the
/// plugin AND the shared library alive across every
/// `spawn_blocking` we issue; once the shim itself drops + every
/// consumer Arc goes away, the instance's `close_instance`
/// fires and (when no more instances of the same plugin exist)
/// the manager can unload the library safely.
pub struct FfiCalendarAdapter {
    /// Keeps the loaded instance — and the loaded plugin Arc it
    /// transitively holds — alive. Don't dereference directly
    /// for vtable calls; use `instance_handle()` instead.
    _instance: Arc<LoadedInstance>,
    /// Cached opaque per-account handle. `*mut c_void` itself
    /// isn't `Send`/`Sync`, so we store the address as `usize`
    /// and cast at the call site (see `instance_handle`).
    handle_addr: usize,
    /// Cached copy of each method-pointer slot from the plugin's
    /// vtable. Copies (rather than holding a `&CalendarVtable`)
    /// because we hand individual fn pointers to `spawn_blocking`
    /// closures — those need `'static` so the borrow has to
    /// disappear before the closure runs.
    vtable: VtableSnapshot,
    /// Statically-cached capability list, derived once at
    /// construction time from the plugin's manifest. Used by the
    /// `Adapter::capabilities()` trait method, which is sync
    /// and can't go through the FFI itself.
    capabilities: Vec<Capability>,
    /// In-flight counter handle shared with the
    /// [`crate::manager::LoadedPlugin`]. Every FFI-dispatching
    /// trait method brackets its body with an [`InFlightGuard`]
    /// derived from this Arc so the host's unload path can
    /// observe a deterministic "is anything in flight" gate.
    in_flight: Arc<std::sync::atomic::AtomicUsize>,
}

/// Owned snapshot of every fn-pointer slot. We copy these out
/// of the [`CalendarVtable`] at construction time so each
/// `spawn_blocking` closure can move its own copy without having
/// to hold a reference back into the manager.
#[derive(Clone, Copy)]
struct VtableSnapshot {
    authenticate: Option<crate::vtables::VtableMethodFn>,
    list_calendars: Option<crate::vtables::VtableMethodFn>,
    get_events: Option<crate::vtables::VtableMethodFn>,
    create_event: Option<crate::vtables::VtableMethodFn>,
    update_event: Option<crate::vtables::VtableMethodFn>,
    delete_event: Option<crate::vtables::VtableMethodFn>,
    get_free_busy: Option<crate::vtables::VtableMethodFn>,
    calendar_color: Option<crate::vtables::VtableMethodFn>,
    add_event_exdate: Option<crate::vtables::VtableMethodFn>,
    rename_calendar: Option<crate::vtables::VtableMethodFn>,
    get_events_delta: Option<crate::vtables::VtableMethodFn>,
    current_user_email: Option<crate::vtables::VtableMethodFn>,
    respond_to_event: Option<crate::vtables::VtableMethodFn>,
}

impl FfiCalendarAdapter {
    /// Wrap a loaded calendar-adapter plugin instance so it can
    /// be handed to the rest of the host as
    /// `Arc<dyn CalendarFeature>`. Returns `None` when the
    /// plugin doesn't actually provide the calendar capability
    /// (its [`crate::vtables::AdapterVtable::calendar`] slot is null) or
    /// fails the minimum-surface check.
    ///
    /// Multi-capability plugins (e.g. CalDAV providing
    /// calendar + tasks + contacts) wrap the same loaded
    /// instance once per capability via the three FfiAdapter
    /// shims — they all share the same per-account handle.
    pub fn new(instance: Arc<LoadedInstance>) -> Option<Self> {
        let plugin = instance.plugin().clone();
        // Null pointer, or a layout this host cannot read — either way the
        // plugin is not callable. `adapter_vtable` reads `vtable_version` before
        // it trusts anything else in the struct.
        let Some(outer) = super::adapter_vtable(&plugin) else {
            warn!(
                plugin_id = %plugin.manifest.id,
                host_abi = crate::ABI_VERSION,
                "calendar plugin has no vtable this host can read; refusing to wrap",
            );
            return None;
        };
        if outer.calendar.is_null() {
            // Plugin didn't declare Capability::Calendar — not an
            // error, just means the registry should skip the
            // calendar slot for this plugin.
            return None;
        }
        // SAFETY: outer.calendar is non-null and points at a
        // CalendarVtable static in the plugin library; the
        // LoadedPlugin Arc inside the instance keeps it alive.
        let vtable_ref: &CalendarVtable = unsafe { &*outer.calendar };
        if !vtable_ref.has_minimum_surface() {
            warn!(
                plugin_id = %plugin.manifest.id,
                "calendar plugin's vtable lacks list_calendars; refusing to wrap",
            );
            return None;
        }
        let snapshot = VtableSnapshot {
            authenticate: vtable_ref.authenticate,
            list_calendars: vtable_ref.list_calendars,
            get_events: vtable_ref.get_events,
            create_event: vtable_ref.create_event,
            update_event: vtable_ref.update_event,
            delete_event: vtable_ref.delete_event,
            get_free_busy: vtable_ref.get_free_busy,
            calendar_color: vtable_ref.calendar_color,
            add_event_exdate: vtable_ref.add_event_exdate,
            rename_calendar: vtable_ref.rename_calendar,
            get_events_delta: vtable_ref.get_events_delta,
            current_user_email: vtable_ref.current_user_email,
            respond_to_event: vtable_ref.respond_to_event,
        };

        let capabilities = super::manifest_capabilities(&plugin.manifest.capabilities);
        let handle_addr = instance.handle() as usize;
        let in_flight = Arc::clone(plugin.in_flight_handle());

        Some(Self {
            _instance: instance,
            handle_addr,
            vtable: snapshot,
            capabilities,
            in_flight,
        })
    }
}

/// Helper to turn a [`CallOutcome`] into either a typed value
/// (via `decode_payload`) or the matching `cal_core::Error`.
async fn call_then_decode<T, A>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> Result<T>
where
    T: serde::de::DeserializeOwned,
    A: Serialize,
{
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!("encode args: {e}")))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        decode_payload(&outcome.bytes)
            .map_err(|e| Error::Protocol(format!("decode plugin response: {e}")))
    } else {
        Err(status_to_cal_error(outcome))
    }
}

/// Same shape as [`call_then_decode`] but for trait methods that
/// return `()` — we only care about the status code. Empty
/// payload is fine.
async fn call_for_unit<A: Serialize>(
    method: Option<crate::vtables::VtableMethodFn>,
    instance_addr: usize,
    args: &A,
) -> Result<()> {
    let bytes = encode_args(args).map_err(|e| Error::Internal(format!("encode args: {e}")))?;
    let outcome = call_method(method, instance_addr, bytes).await;
    if outcome.is_ok() {
        Ok(())
    } else {
        Err(status_to_cal_error(outcome))
    }
}

/// Plugin status → `cal_core::Error` variant. Same shape as the
/// status-code constants documented in `crate::ffi`.
fn status_to_cal_error(outcome: CallOutcome) -> Error {
    let msg = outcome.message();
    match outcome.status {
        PLUGIN_CALL_ERR_UNSUPPORTED => Error::Unsupported(msg),
        PLUGIN_CALL_ERR_INVALID => Error::InvalidInput(msg),
        PLUGIN_CALL_ERR_AUTH => Error::Authentication(msg),
        PLUGIN_CALL_ERR_NETWORK => Error::Network(msg),
        PLUGIN_CALL_ERR_NOT_FOUND => Error::NotFound(msg),
        PLUGIN_CALL_ERR_PROTOCOL => Error::Protocol(msg),
        PLUGIN_CALL_ERR_CONFLICT => Error::Conflict(msg),
        PLUGIN_CALL_ERR_FORBIDDEN => Error::Forbidden(msg),
        PLUGIN_CALL_ERR_IO => Error::Internal(format!("plugin IO: {msg}")),
        PLUGIN_CALL_ERR_INTERNAL => Error::Internal(msg),
        other => Error::Internal(format!("plugin status {other}: {msg}")),
    }
}

#[async_trait]
impl Adapter for FfiCalendarAdapter {
    async fn authenticate(&self, credentials: Credentials) -> Result<AuthToken> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.authenticate, self.handle_addr, &credentials).await
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

// JSON-shape helpers — one struct per method that carries multiple
// arguments. Single-arg methods reuse the typed arg directly.

#[derive(Serialize)]
struct GetEventsArgs<'a> {
    calendar_id: &'a str,
    range: DateRange,
}

#[derive(Serialize)]
struct GetEventsDeltaArgs<'a> {
    calendar_id: &'a str,
    range: DateRange,
    since_token: Option<&'a str>,
}

#[derive(Serialize)]
struct CreateEventArgs<'a> {
    calendar_id: &'a str,
    event: NewEvent,
}

#[derive(Serialize)]
struct DeleteEventArgs<'a> {
    event_id: &'a str,
    send_cancellations: bool,
}

#[derive(Serialize)]
struct GetFreeBusyArgs<'a> {
    emails: Vec<&'a str>,
    range: DateRange,
}

#[derive(Serialize)]
struct AddExdateArgs<'a> {
    event_id: &'a str,
    occurrence: chrono::DateTime<chrono::Utc>,
    send_cancellations: bool,
}

#[derive(Serialize)]
struct RenameCalendarArgs<'a> {
    calendar_id: &'a str,
    new_name: &'a str,
}

#[derive(Serialize)]
struct RespondToEventArgs<'a> {
    event_id: &'a str,
    status: AttendeeStatus,
    send_response: bool,
}

#[async_trait]
impl CalendarFeature for FfiCalendarAdapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.list_calendars, self.handle_addr, &()).await
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = GetEventsArgs { calendar_id, range };
        call_then_decode(self.vtable.get_events, self.handle_addr, &args).await
    }

    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> Result<Event> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = CreateEventArgs { calendar_id, event };
        call_then_decode(self.vtable.create_event, self.handle_addr, &args).await
    }

    async fn update_event(&self, event: Event) -> Result<Event> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        call_then_decode(self.vtable.update_event, self.handle_addr, &event).await
    }

    async fn delete_event(&self, event_id: &str, send_cancellations: bool) -> Result<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = DeleteEventArgs {
            event_id,
            send_cancellations,
        };
        call_for_unit(self.vtable.delete_event, self.handle_addr, &args).await
    }

    async fn get_free_busy(&self, emails: &[&str], range: DateRange) -> Result<Vec<FreeBusy>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = GetFreeBusyArgs {
            emails: emails.to_vec(),
            range,
        };
        call_then_decode(self.vtable.get_free_busy, self.handle_addr, &args).await
    }

    /// Synchronous slot in the trait. We dispatch to the plugin
    /// directly (no `spawn_blocking`) because we're already off
    /// the runtime here — the trait method is non-async and the
    /// plugin's implementation is expected to answer from
    /// in-memory state without IO.
    fn calendar_color(&self, calendar_id: &str) -> Option<ContainerColor> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let method = self.vtable.calendar_color?;
        let args = match encode_args(&calendar_id) {
            Ok(b) => b,
            Err(err) => {
                warn!(?err, "calendar_color encode args");
                return None;
            }
        };
        let outcome = call_method_sync(Some(method), self.handle_addr, args);
        if !outcome.is_ok() {
            warn!(
                status = outcome.status,
                msg = %outcome.message(),
                "calendar_color plugin returned non-OK status",
            );
            return None;
        }
        decode_payload::<Option<ContainerColor>>(&outcome.bytes)
            .map_err(|err| {
                warn!(?err, "calendar_color decode failed");
            })
            .ok()
            .flatten()
    }

    async fn add_event_exdate(
        &self,
        event_id: &str,
        occurrence: chrono::DateTime<chrono::Utc>,
        send_cancellations: bool,
    ) -> Result<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = AddExdateArgs {
            event_id,
            occurrence,
            send_cancellations,
        };
        call_for_unit(self.vtable.add_event_exdate, self.handle_addr, &args).await
    }

    async fn rename_calendar(&self, calendar_id: &str, new_name: &str) -> Result<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = RenameCalendarArgs {
            calendar_id,
            new_name,
        };
        call_for_unit(self.vtable.rename_calendar, self.handle_addr, &args).await
    }

    async fn get_events_delta(
        &self,
        calendar_id: &str,
        range: DateRange,
        since_token: Option<&str>,
    ) -> Result<ChangeSet<Event>> {
        // Null slot → Unsupported via `call_method`; the host falls
        // back to a full `get_events`.
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = GetEventsDeltaArgs {
            calendar_id,
            range,
            since_token,
        };
        call_then_decode(self.vtable.get_events_delta, self.handle_addr, &args).await
    }

    async fn current_user_email(&self) -> Result<Option<String>> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        // A null slot (read-only adapters that have no identity) maps to
        // `Unsupported`; for *this* method that means "identity unknown",
        // which is `Ok(None)`, not an error — the host just hides RSVP.
        match call_then_decode::<Option<String>, _>(
            self.vtable.current_user_email,
            self.handle_addr,
            &(),
        )
        .await
        {
            Ok(v) => Ok(v),
            Err(Error::Unsupported(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn respond_to_event(
        &self,
        event_id: &str,
        status: AttendeeStatus,
        send_response: bool,
    ) -> Result<()> {
        let _guard = InFlightGuard::enter(Arc::clone(&self.in_flight));
        let args = RespondToEventArgs {
            event_id,
            status,
            send_response,
        };
        call_for_unit(self.vtable.respond_to_event, self.handle_addr, &args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::PluginManifest;
    use crate::vtables::CalendarVtable;
    use std::os::raw::c_void;
    use std::sync::Mutex;

    // For `list_calendars` we'll return a fixed 2-cal JSON. The
    // test reads them back through the shim + asserts.
    static LIST_CALENDARS_CALLED: Mutex<usize> = Mutex::new(0);

    extern "C" fn fake_list_calendars(
        _instance: *mut c_void,
        _args_ptr: *const u8,
        _args_len: usize,
    ) -> PluginCallResult {
        *LIST_CALENDARS_CALLED.lock().unwrap() += 1;
        let cals = vec![
            Calendar {
                color_label: None,
                supports_scheduling: false,
                supports_event_color: false,
                id: "cal-1".into(),
                name: "Calendar One".into(),
                color: None,
                read_only: false,
                default_sound: None,
            },
            Calendar {
                color_label: None,
                supports_scheduling: false,
                supports_event_color: false,
                id: "cal-2".into(),
                name: "Calendar Two".into(),
                color: None,
                read_only: true,
                default_sound: None,
            },
        ];
        let json = serde_json::to_vec(&cals).expect("serialise");
        // We deliberately leak the boxed buffer so the lifetime
        // story matches what a real plugin would do (the bytes
        // outlive the call until the host frees them).
        let mut boxed = json.into_boxed_slice();
        let data = boxed.as_mut_ptr();
        let len = boxed.len();
        std::mem::forget(boxed);
        unsafe extern "C" fn free_boxed(data: *mut u8, len: usize) {
            if !data.is_null() {
                let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len));
            }
        }
        PluginCallResult {
            status: PLUGIN_CALL_OK,
            payload: PluginBytes {
                data,
                len,
                free: Some(free_boxed),
            },
        }
    }

    // For an error-path test: returns auth-error with a custom
    // message that the shim translates into Error::Authentication.
    extern "C" fn fake_authenticate(
        _instance: *mut c_void,
        _args_ptr: *const u8,
        _args_len: usize,
    ) -> PluginCallResult {
        let msg = b"creds rejected".to_vec();
        let mut boxed = msg.into_boxed_slice();
        let data = boxed.as_mut_ptr();
        let len = boxed.len();
        std::mem::forget(boxed);
        unsafe extern "C" fn free_boxed(data: *mut u8, len: usize) {
            if !data.is_null() {
                let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(data, len));
            }
        }
        PluginCallResult {
            status: PLUGIN_CALL_ERR_AUTH,
            payload: PluginBytes {
                data,
                len,
                free: Some(free_boxed),
            },
        }
    }

    // Synthetic LoadedInstance + FfiCalendarAdapter for tests.
    fn make_fake_adapter(
        list_calendars: Option<crate::vtables::VtableMethodFn>,
        authenticate: Option<crate::vtables::VtableMethodFn>,
    ) -> FfiCalendarAdapter {
        let cal_vtable = Box::new(CalendarVtable {
            list_calendars,
            authenticate,
            ..CalendarVtable::empty()
        });
        let cal_ptr: *const CalendarVtable = Box::into_raw(cal_vtable);
        let outer = Box::new(crate::vtables::AdapterVtable {
            calendar: cal_ptr,
            ..crate::vtables::AdapterVtable::empty()
        });
        let vtable_ptr = Box::into_raw(outer) as *mut c_void;

        let id_cstr = std::ffi::CString::new("test.calendar").unwrap();
        let name_cstr = std::ffi::CString::new("Test Calendar").unwrap();
        let version_cstr = std::ffi::CString::new("0.1.0").unwrap();
        let type_cstr = std::ffi::CString::new("adapter").unwrap();
        let descriptor = Box::new(crate::abi::AperioPlugin {
            abi_version: crate::ABI_VERSION,
            id: id_cstr.into_raw(),
            name: name_cstr.into_raw(),
            version: version_cstr.into_raw(),
            plugin_type: type_cstr.into_raw(),
            open_instance: None,
            close_instance: None,
            vtable: vtable_ptr,
        });
        let descriptor_ptr = Box::into_raw(descriptor);

        unsafe extern "C" fn noop_destroy(_: *mut crate::abi::AperioPlugin) {}
        let loaded = crate::manager::test_support::loaded_plugin_for_tests(
            PluginManifest {
                id: "test.calendar".to_string(),
                name: "Test".to_string(),
                version: "0.1.0".to_string(),
                plugin_type: crate::PluginType::Adapter,
                capabilities: vec![crate::Capability::Calendar],
                abi_version: crate::ABI_VERSION,
                min_app_version: "0.1.0".to_string(),
                author: None,
                description: None,
                signed: false,
                recurrence: Default::default(),
                tasks: Default::default(),
                account: None,
                adapter_kind: None,
                adopts_adapter_kinds: Vec::new(),
                strings: Default::default(),
            },
            descriptor_ptr,
            noop_destroy,
        );
        let plugin = Arc::new(loaded);
        let instance =
            crate::manager::test_support::loaded_instance_for_tests(plugin, std::ptr::null_mut());
        FfiCalendarAdapter::new(instance).expect("vtable has minimum surface")
    }

    #[tokio::test]
    async fn list_calendars_round_trips_through_ffi() {
        *LIST_CALENDARS_CALLED.lock().unwrap() = 0;
        let adapter = make_fake_adapter(Some(fake_list_calendars), None);
        let cals = adapter.list_calendars().await.expect("ok");
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].id, "cal-1");
        assert_eq!(cals[1].id, "cal-2");
        assert_eq!(*LIST_CALENDARS_CALLED.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn auth_error_maps_to_cal_core_authentication() {
        let adapter = make_fake_adapter(Some(fake_list_calendars), Some(fake_authenticate));
        let err = adapter
            .authenticate(Credentials::default())
            .await
            .unwrap_err();
        match err {
            Error::Authentication(msg) => assert_eq!(msg, "creds rejected"),
            other => panic!("expected Authentication, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_method_yields_unsupported() {
        let adapter = make_fake_adapter(Some(fake_list_calendars), None);
        let err = adapter.delete_event("ignored", false).await.unwrap_err();
        assert!(matches!(err, Error::Unsupported(_)));
    }

    #[test]
    fn capabilities_reflect_manifest() {
        let adapter = make_fake_adapter(Some(fake_list_calendars), None);
        assert_eq!(adapter.capabilities(), &[Capability::Calendar]);
    }
}
