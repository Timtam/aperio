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

- Expo SDK 58 (React Native 0.88, React 19.3, TypeScript 6), `blank-typescript`,
  with the UIKit scene life cycle (Xcode 27 / iOS 27 SDK) since 2026-11.
- **EAS Build + EAS Submit** for cloud iOS builds, code-signing and
  TestFlight upload. No Mac required — EAS builds and signs on Expo's cloud
  macOS workers.

`AGENTS.md` is the Expo template's reminder that the SDK changes APIs; check
the versioned docs at <https://docs.expo.dev/versions/v58.0.0/> before adding
Expo packages.

### What the move to SDK 58 brought (2026-11)

Apple takes only builds made with the iOS 27 SDK (Xcode 27) from April 2027,
and an app built with it starts on iOS 27 only under the UIKit scene life
cycle. Expo SDK 58 generates a `SceneDelegate` and starts React Native in
`scene(_:willConnectTo:)`. What that and the version jump needed here:

- **Background launches.** When iOS relaunches a terminated app for a
  background task it connects no scene, so under SDK 58 no JavaScript ran: no
  sync, reminder or widget round after the app had been ended (expo/expo#52878,
  open). `modules/background-launch` starts the React host without a window
  when no scene has connected two seconds after launch (release builds only).
  Remove it once Expo fixes the issue.
- **What only the screens did.** Such a round runs without the app's screens,
  so it cannot lean on what they set up. The round applies the stored app
  language itself before it words reminders, the widget or Live Activities
  (Android's headless runs needed that all along), and `BackgroundRefresh`
  keeps the refresh interval in `UserDefaults`, so its re-arm does not fall
  back to 15 minutes.
- **Links into a running app.** After such a launch, a window that connects
  with a link (a Live Activity tap, an .ics opened in Aperio) finds React
  Native running: React Native keeps its first initial URL and sends no event
  for the link. expo-linking's app-delegate subscriber records it, so on iOS
  the reminder tap and the file import read `Linking.getLinkingURL()` (and
  clear it once taken) instead of React Native's `getInitialURL()`. Android
  cannot use that record for the launch link: expo-linking writes it again
  whenever Android recreates the activity, which would open the tap again. It
  takes the link from the activity's launch intent once per launch
  (`CalFfi.takeLaunchLink`, the rules of the file import's launch intent).
- **VoiceOver interruptions.** React Native 0.88 posts a layout change to
  VoiceOver for every changed text, which cuts announcements. 0.89 skips text
  in hidden subtrees; `patches/react-native+0.88.1.patch` backports that
  guard. It only takes effect because `expo-build-properties` builds React
  Native from source on iOS (`buildReactNativeFromSource`), which makes iOS
  builds slower. Drop both with SDK 59 (React Native 0.89).
- **Android release builds stay unminified** (`enableMinifyInReleaseBuilds:
  false`): SDK 58 turns R8 on by default, and the UniFFI bindings reach Rust
  through JNA reflection, which R8 can break.
- **No push entitlement** comes from expo-notifications' own
  `enableRemoteNotifications: false`; the plugin that deleted the key is gone.
- **Versions Expo does not manage:** react-native-bottom-tabs and
  @bottom-tabs/react-navigation 1.5.0 (1.3 does not build on React Native
  0.88; 1.5 also fixes VoiceOver calling every visited tab "selected" on iOS
  27). @react-navigation stays on v7 (v8 is an alpha that breaks the bottom
  tabs navigator). @bacons/apple-targets 5.0.0 has no SDK 58 release; it is
  kept and checked by the EAS build.
- **Code:** refs are typed with React Native's instance types (`TextInstance`,
  `ViewInstance`, ...) under its strict TypeScript API; InteractionManager is
  gone (the startup gate waits for an idle callback); our modules import from
  `expo`, our config plugins from `expo/config-plugins`.

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

## Tasks UI (M1 — offline, Android-first)

`App.tsx` is now a minimal, accessibility-first **tasks screen** backed by the
shared Rust core: the `cal-ffi` module exposes the on-device `Host`
(SQLite migrated by the same `aperio-db` runner the desktop uses) and the
screen lists / creates / completes / renames / deletes tasks through it —
real offline persistence, not mock state. Each task carries custom
accessibility actions (complete/reopen, rename, delete).

Local Android dev builds (cargo-ndk `.so` + `expo-dev-client` + emulator) cost
**no EAS quota** — iterate freely here; reserve EAS/TestFlight for the iOS
VoiceOver pass.

**After any change to `crates/cal-ffi`, rebuild the native `.so`** (gitignored)
for the emulator's ABI (`x86_64`) and the device ABI (`arm64-v8a`) with
cargo-ndk, then rebuild the dev client. The Kotlin bindings take care of
themselves: they are generated, not committed (TODO 89a), into
`modules/cal-ffi/android/src/main/java/uniffi/` (gitignored), by
`npm run bindings:android`, by `npm run check:bindings` and therefore by
`npm run android`, which runs the check first. Each builds `crates/cal-ffi` for
the host and reads the interface out of it, so the bindings come from the same
source as a `.so` built from the same tree; JNA fails to resolve symbols at
call time when they do not. The Android workflow generates them next to the
release `.so` it ships.

iOS needs none of this by hand either. `mobile-ios-testflight.yml` builds the
XCFramework and generates the Swift bindings fresh on every dispatch, and
neither is committed: `modules/cal-ffi/ios/CalFfi.xcframework/` and
`modules/cal-ffi/ios/cal_ffi.swift` are gitignored. A committed copy could only
ever be stale — the last Swift one lagged the Rust by some forty symbols for
months, and nothing noticed, because no build ever read it; the committed Kotlin
one let the whole event-group API and the whole day-marker API sit unbuildable
on Android for weeks, because the vendoring step refreshed the `.so` and left
the bindings behind.

**`npm run check:bindings`** holds the Rust, the bindings and both bridges to
each other before anything expensive. Nothing else does: `tsc` and ESLint never
look at Kotlin or Swift. It generates the Kotlin bindings first (a `cargo build`
of cal-ffi; `--use-existing` reads the ones already in place, as the Android
workflow does).

It compares ARITY, not just names, in three directions — Rust ↔ generated
bindings, bindings ↔ Kotlin bridge, Rust ↔ Swift bridge. The name-only version
missed a change where three methods gained a parameter, the Kotlin bridge was
updated to pass it, and the regeneration silently did not run: every name still
matched. The Swift direction matters most, because nothing on this machine ever
compiles Swift — that mistake is only visible after an XCFramework build.

It also checks what JavaScript can call. Every function
`modules/cal-ffi/src/CalFfiModule.ts` declares must be registered in BOTH
`CalFfiModule.kt` and `CalFfiModule.swift`, with `AsyncFunction` when it returns
a Promise and `Function` otherwise, and with one closure parameter per declared
parameter (expo's trailing `Promise` parameter does not count). A commented-out
registration does not count, and a declaration or a registration the check
cannot read is named, not skipped. TypeScript trusts the declaration, so without this a missing
registration shows only on the phone, as "CalFfi.x is not a function", and the
wrong kind as a Promise where a value was expected. A function one platform
lacks on purpose (the iOS background wake-up, the Siri pickers) is listed in
`ONLY_ON` in the script, with its reason.

And it watches the Android module's size. The JVM caps one method's bytecode at
64 KB, and expo inlines every registration into the method it sits in, so
`CalFfiModule.kt` splits its registrations across `ModuleDefinitionBuilder`
extensions. The check fails when `definition()` or one extension holds more than
70 registrations, or when a registration sits where it cannot tell which method
holds it. Without it, the cap showed up as "Method too large" at
`:cal-ffi:compileReleaseKotlin`, minutes into an EAS build: at 140
registrations, and at 130 after the first split, because new ones kept landing
in `definition()`.

So a new free function in `crates/cal-ffi` takes three steps: export it,
declare it in `CalFfiModule.ts`, and register it in both native modules.
