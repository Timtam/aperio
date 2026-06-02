# Testing

## Unit tests (Rust)

Every workspace crate carries its own unit tests:

```sh
cargo test --workspace            # everything
cargo test -p cal-adapter-todoist # one crate
cargo test -p aperio cache::      # one module in the host
```

Pure logic — date math, recurrence handling, priority/colour mapping,
wire-shape mapping — is covered by ordinary `#[test]`s next to the code.

## Adapter integration tests (mock servers)

Provider adapters are tested against an **HTTP mock server** rather than a
live account, so the suite is deterministic and offline. The pattern (used
by the Google, Microsoft Graph, CalDAV, Vikunja, Todoist… adapters):

- [`mockito`](https://docs.rs/mockito) stands in for the provider's API.
- A test-only constructor points the adapter's HTTP client at the mock's
  URL (e.g. `with_base_url_for_tests`).
- Each test registers the exact request it expects and the canned JSON/XML
  response, then asserts the mapped `cal-core` value.

This keeps the *mapping* honest (wire shape → domain type and back) without
network flakiness. The assertions encode our understanding of the wire
contract — when a real provider surprises us, the fix is a new mock case.

## The host & sync tests

The `aperio` (host) crate tests cover the database migrations, the snapshot
cache range/overlap logic, the override layer, and the **event-log
applier** — including conflict detection and convergence (applying the same
envelopes in any order reaches the same state). The local adapter's tests
build an in-memory SQLite database via `cal_adapter_local::test_support`,
which replays the migration SQL so the schema matches a real run.

## Frontend tests

```sh
npm run test    # Vitest — hooks, state reducers, intl/date helpers, a11y
```

These cover pure helpers (date keys, recurrence expansion via `rrule`,
colour resolution) and accessibility-sensitive components (the live-region
announcer, keyboard grid navigation).

## Accessibility testing

Automated checks can't prove screen-reader usability, so accessibility is
**also verified manually** against the screen readers the project targets:

| Platform | Screen reader(s) |
|---|---|
| Windows | NVDA, JAWS, Narrator |
| macOS | VoiceOver |

When you change an interactive surface, verify with at least one screen
reader that focus order, roles/states, and live announcements behave, and
note what you tested in the PR. See the user book's
[accessibility page](/aperio/user/barrierefreiheit.html) for the intended
behaviour per reader.

## CI

CI runs on every push/PR and is the authority. It mirrors the local gate:

- `cargo fmt --all -- --check` (fails fast on formatting),
- `cargo clippy --workspace --all-targets -- -D warnings`,
- `cargo test --workspace`,
- `tsc`, ESLint, Vitest, and the production `vite build`,

across the supported desktop platforms. Make the local gate green before
pushing and CI will follow.
