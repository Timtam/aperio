# builtin-oauth — where Aperio's own OAuth client credentials live

Some providers will not issue tokens to a client that cannot identify itself.
Cisco Webex requires a client id **and** a client secret, even under PKCE.
Google requires both, while saying in its own documentation that for an
installed app the secret is not treated as a secret. Microsoft Graph is
registered as a public client and needs neither.

So a build is in one of two postures, and both are supported:

- **Built-in** — the build carries Aperio's registered client, and connecting an
  account is one button.
- **Bring your own** — the build carries nothing, and the app asks the user to
  register their own integration and paste its client id and secret. This is
  what every local build does by default, and it is a first-class mode, not a
  degraded one.

## Supplying credentials to a LOCAL build

Create `oauth-clients.local.env` in the repository root. It is gitignored — the
rule is in `.gitignore` under "Built-in OAuth client credentials", and
`git check-ignore -v oauth-clients.local.env` will confirm it before you write
anything sensitive into it.

```
# Cisco Webex — https://developer.webex.com/my-apps
APERIO_OAUTH_WEBEX_CLIENT_ID=C1234567890abcdef…
APERIO_OAUTH_WEBEX_CLIENT_SECRET=abcdef1234567890…

# Google — optional, not wired yet
# APERIO_OAUTH_GOOGLE_CLIENT_ID=…
# APERIO_OAUTH_GOOGLE_CLIENT_SECRET=…

# Microsoft Graph — public client, id only
# APERIO_OAUTH_MICROSOFT_CLIENT_ID=…
```

Blank lines and `#` comments are ignored, a leading `export ` is tolerated so
the same file can be sourced by a shell, and surrounding quotes are stripped.

To keep the file outside the checkout entirely, put it anywhere and point at it:

```bash
export APERIO_OAUTH_CLIENTS_FILE=/secure/place/aperio-oauth.env
```

Plain environment variables work too and take precedence over the file, which is
how CI supplies them and how you can override a single value for one build:

```bash
APERIO_OAUTH_WEBEX_CLIENT_ID=… cargo build -p builtin-oauth
```

### After changing the file

`build.rs` declares `cargo:rerun-if-changed` on it, so a normal `cargo build`
picks up an edit. If you ever suspect a stale value, `cargo clean -p builtin-oauth`
is enough — the credentials are compiled into this crate and nothing else.

### Confirming what a build carries

`builtin_oauth::baked_count()` reports how many credentials are compiled in, and
`has_builtin_client(Provider::Webex)` answers for one provider. Neither ever
exposes a value. The build prints no credential to the log either — build output
lands in pasted issue reports.

## Supplying credentials to CI

Set the same variable names as repository secrets and export them in the
workflow step that runs the build. Do **not** commit a file. Mobile needs no EAS
secret: the values are compiled into the Rust core, and the mobile build
consumes that core as a prebuilt artefact.

An unset secret expands to the empty string, which this crate treats as absent —
so a workflow that forgets to export them produces a working bring-your-own
build rather than a broken built-in one.

## Rules this crate exists to enforce

**A secret never reaches `accounts.config_json`.** That column is documented as
non-secret and is appended to the sync event log unconditionally, so with
end-to-end encryption switched off it travels to the remote sync target in the
clear. Accounts store a *reference* to a credential, never its value.

**A rotated client is detected, not suffered.** Refresh tokens are bound to the
client that minted them, so replacing Aperio's registration invalidates every
account that used it. `ClientFingerprint` is stored beside the account so the
app can say "this account was connected with a different Aperio registration,
please reconnect" instead of failing with an opaque `invalid_grant` whenever the
access token happens to lapse.

**Scope is the mitigation, not obscurity.** A secret compiled into a shipped
binary is extractable. That is inherent to the posture, so the registration asks
for the narrowest scopes that work and the credential stays revocable.

## Adding a provider

1. Add the variable names to `VARS` in `build.rs`.
2. Add the enum variant, its string, its `requires_secret()` answer, and the
   `option_env!` pair in `src/lib.rs`.
3. Document the variables in the sample block above.
