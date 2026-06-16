# Aperio Mobile (pilot)

A throwaway-but-promotable Expo / React Native pilot whose only job right
now is to **de-risk the whole Windows → iOS → TestFlight pipeline** and let
us validate VoiceOver/TalkBack behaviour on a real device *before* porting
any of Aperio's real UI.

Accessibility is the reason this app exists, so the pilot screen
(`App.tsx`) deliberately exercises the three things Aperio depends on:
semantic headings/labels, **custom accessibility actions** (the VoiceOver
"Actions" rotor — swipe up/down on a focused element), and **dynamic
announcements** (live region + `announceForAccessibility`).

> This folder is intentionally isolated from the Rust/Tauri workspace. The
> Cargo workspace lists its members explicitly, so `mobile/` is never part
> of `cargo build`. Engine reuse (sharing `cal-core` via a UniFFI Turbo
> Module) is a *later* phase — see "Roadmap".

## Stack

- Expo SDK 56 (React Native 0.85, React 19, TypeScript 6), `blank-typescript`.
- **EAS Build + EAS Submit** for cloud iOS builds, code-signing and
  TestFlight upload. No Mac required — EAS builds and signs on Expo's cloud
  macOS workers.

`AGENTS.md` is the Expo template's reminder that SDK 56 changed several
APIs; check the versioned docs at <https://docs.expo.dev/versions/v56.0.0/>
before adding Expo packages.

## Local development (Windows)

```powershell
npm install                 # once
npm run start               # Metro bundler; press 'a' for Android emulator / 'w' for web
```

Expo Go can run the JS-only pilot today. **The moment we add the native
Rust module we must switch to a Development Build** (`expo-dev-client`) —
Expo Go ships a fixed, immutable set of native libraries and cannot load
custom native code. `expo-dev-client` is already a dependency and the
`development` EAS profile is ready for that switch.

There is no local iOS Simulator on Windows; iOS is tested via TestFlight on
a physical iPhone (see below).

## One-time, account-bound setup (you must do these — they need *your*
## Apple/Expo accounts)

All steps below work from a browser + Windows. None needs a Mac.

1. **Pick the bundle identifier.** It is currently `com.aperio.mobile` in
   `app.json` (`ios.bundleIdentifier` + `android.package`). Change it *now*
   if you want a different reverse-DNS id — it becomes permanent once an App
   Store Connect record is tied to it.
2. **Expo account.** Create one at <https://expo.dev>, then `npx eas-cli
   login`.
3. **Link the project.** `npx eas-cli init` — creates the EAS project and
   writes `extra.eas.projectId` + `owner` into `app.json`.
4. **App Store Connect record (browser).** App Store Connect → Apps → "+"
   → New App. Set platform iOS, the app name, primary language, the **same
   bundle identifier** as step 1, and an SKU. Accept any pending agreements
   (Agreements, Tax, and Banking — even a free app needs the free agreement
   accepted). *(Alternatively the interactive `eas submit` can create this
   record for you via fastlane, but doing it by hand is clearer the first
   time.)*
5. **App Store Connect API key (browser, recommended).** App Store Connect →
   Users and Access → **Integrations** → App Store Connect API →
   generate a **Team** key with **Admin** access. Download the `.p8` **once**
   (it cannot be re-downloaded) and note the **Key ID** and **Issuer ID**.
   This key is what lets `eas submit` upload non-interactively. The legacy
   alternative is Apple ID + an app-specific password.
   - Why a *Team* key, not Individual: Apple's Individual keys cannot use the
     Provisioning endpoints EAS needs for signing automation.

## Build & ship to TestFlight (from Windows)

```powershell
# 1. Cloud build a store-distribution .ipa (EAS prompts to generate &
#    store the iOS distribution certificate + provisioning profile the
#    first time — just say yes; no Mac involved).
npx eas-cli build --platform ios --profile production

# 2. Upload that build to App Store Connect / TestFlight. Interactive the
#    first time; it will ask for / store your App Store Connect API key.
npx eas-cli submit --platform ios --profile production
```

- After upload, TestFlight processing usually takes ~10–15 min.
- Use **internal** TestFlight testing (up to 100 users): it needs **no Beta
  App Review**, so the build is installable on your iPhone as soon as it
  finishes processing. (External testing — up to 10 000 testers — *does*
  require Beta App Review.)
- Install the **TestFlight** app on the iPhone, sign in with the same Apple
  ID, and the build appears.
- `eas.json` sets `cli.appVersionSource: "remote"` and `production.autoIncrement: true`,
  so EAS owns the build number and bumps it automatically each upload — no
  manual `buildNumber` edits per TestFlight submission.

### Alternative iOS CI (same project, also no Mac)

If EAS's free tier ever gets in the way, the same project can be built and
submitted by **Xcode Cloud** (25 free compute hours/month are already
included with your Apple Developer Program membership) or **Codemagic**.
Both build on cloud macOS, so neither needs a local Mac.

## What still genuinely needs a Mac

Building, signing, TestFlight and the App Store are all solved without one.
What has **no Windows equivalent**:

- the local **iOS Simulator**,
- Xcode's **Accessibility Inspector** (deep semantic-tree inspection),
- **Instruments** (profiling),
- interactive native (lldb) debugging.

For an accessibility-first workflow the practical substitute is **TestFlight
+ on-device VoiceOver** on the physical iPhone, plus React Native DevTools
for the JS side. This is the honest gap: you can ship and hear the result,
but you cannot inspect the raw iOS accessibility tree on Windows.

## Roadmap

1. ✅ Pilot screen + EAS pipeline (this).
2. Switch to a **Development Build** (`expo-dev-client`) once native code is
   needed.
3. Expose `cal-core` as a **UniFFI Turbo Module** and call it from the RN
   layer (engine reuse — the Rust core is shared, the UI is rebuilt). The
   `uniffi-bindgen-react-native` path is real but flagged "early
   development / not production-ready" as of mid-2026; a hand-rolled Turbo
   Module over plain UniFFI is the fallback.
4. Port Aperio's real screens, accessibility-first, validating each against
   on-device VoiceOver/TalkBack.

> Android note: local Android builds need **JDK 17** (this machine currently
> has JDK 8). Not needed while iOS goes through EAS.
