# Exchange (EWS)

**Crate:** `cal-adapter-ews` · **Capabilities:** calendars, tasks, contacts

Exchange Web Services is the older SOAP/XML API for on-premises Exchange and
older Microsoft 365 tenants — used where Graph isn't available.

## Protocol

SOAP over HTTPS. Requests are XML envelopes (`soap.rs` builds them,
`mapping.rs` parses the responses):

- **Autodiscover:** the endpoint can be discovered from an email address.
- **Sync:** `SyncFolderItems` returns changes for a folder with a sync
  state token. The adapter first does an **id-only probe** to learn the
  change counts cheaply, then fetches item bodies with `GetItem`.

## Authentication

Basic auth (username/password) over TLS, or NTLM depending on the server.
The endpoint is discovered or user-supplied.

## Quirks

- **Folder-complete events.** EWS keeps a per-folder in-memory view of
  *every* item it has seen, so its event read is **folder-complete**: it
  emits the full set with `ChangeSet.complete = true`, and the host stores
  an unbounded cache window. This is what fixed a class of "event in a new
  month doesn't appear" bugs — the sync cookie is folder-wide, so a
  range-filtered emit would miss unchanged items in newly-viewed ranges.
- **Recurring masters always pass** the folder filter; recurrence shapes
  are enriched via `GetItem`.
- **ChangeKey churn.** An edited item keeps its item id but rotates the
  `ChangeKey` embedded in the composite id, so the cache purges the whole
  native group before re-inserting (avoids stale duplicates).

## Testing

`mockito` (or fixture XML) for the SOAP envelopes. Tests cover the
id-only folder-sync probe/drain, the count parsing, and the
folder-complete emit. Live testing needs an Exchange/365 mailbox that still
exposes EWS.
