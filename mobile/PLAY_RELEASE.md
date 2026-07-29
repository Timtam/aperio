# Aperio auf Google Play — vollständiger Weg zum ersten Test-Build

Stand: 29.07.2026. Gegenstück zum iOS-Abschnitt in `README.md`.

Alle Angaben hier sind gegen Googles und Expos aktuelle Dokumentation geprüft.
Wo die Quellen sich widersprechen, steht das ausdrücklich dabei — dann gilt:
ausprobieren und dem Fehler folgen, nicht der Doku.

Kommandos laufen alle aus `C:\scripts\aperio\mobile`, sofern nichts anderes
dabeisteht. Das ist wichtig: `serviceAccountKeyPath` in `eas.json` wird gegen
das **Arbeitsverzeichnis des Prozesses** aufgelöst, nicht gegen den Ort der
`eas.json`.

---

## 0. Überblick

Fünf Sätze. Du baust mit EAS ein `.aab`, bringst es einmal auf die Spur
"Interner Test", trägst dort deine eigene Google-Adresse als Tester ein und
installierst es auf dem Gerät. Der interne Test ist der einzige Weg, der ohne
das komplette Play-Console-Formularpaket auskommt — Datensicherheit entfällt
dort ausdrücklich. Erst wenn du in den geschlossenen Test gehst, wird der ganze
Papierkram fällig, und dort läuft auch die 14-Tage-Uhr für den
Produktionszugang. Der Service-Account und `eas submit` sind der zweite Schritt,
nicht der erste. Der GitHub-Workflow dafür existiert bereits.

Ehrliche Zeitschätzung: interner Test auf dem Gerät in 1–3 Tagen (die erste
Prüfung durch Google dauert), öffentliche Veröffentlichung frühestens in 4–5
Wochen, falls dein Konto der Testpflicht unterliegt.

---

## 1. Zwei Dinge zuerst klären — sie entscheiden über Wochen

### 1.1 Kontotyp und Anlagedatum

Persönliche Entwicklerkonten, die **nach dem 13.11.2023** angelegt wurden,
kommen nur über einen **geschlossenen Test mit 12 Testern über 14
zusammenhängende Tage** in die Produktion. Organisationskonten und ältere
persönliche Konten nicht.

Nachsehen: Play Console → `Entwicklerkonto` (Developer account) →
`Kontodetails` (Account details). Dort stehen Kontotyp und Anlagedatum.

Zwei wichtige Details, die oft falsch berichtet werden:

- Gezählt wird die **Opt-in-Zeit**, nicht die Installation. Googles Wortlaut ist
  "opted-in for at least the last 14 days continuously". Wer sich abmeldet,
  bricht die Kette; erneutes Anmelden startet die 14 Tage neu.
- Google bewertet zusätzlich die **Beteiligung** der Tester und lehnt Anträge
  ab, wenn die Tester "not engaged" wirken. Zwölf Karteileichen reichen nicht.

Der interne Test zählt **nicht** dafür. Er ist trotzdem der richtige erste
Schritt, weil er ohne die Formulare auskommt.

### 1.2 Google-OAuth-Verifizierung (eigenes Verfahren, nicht Play)

Aperios Google-Adapter nutzt Kalender-Scopes. Das sind bei Google "sensitive
scopes". Ohne abgeschlossene OAuth-Verifizierung im Google-Cloud-Projekt gilt
eine **harte Grenze von 100 Nutzern**, und Nutzer sehen den
"unverifizierte App"-Warnbildschirm.

Das ist ein von Play völlig unabhängiges Review mit eigener Laufzeit. Für einen
internen Test mit einer Handvoll Leuten irrelevant; vor der Produktion nicht.
Wenn du eine öffentliche Veröffentlichung planst, starte die
OAuth-Verifizierung **jetzt parallel**, nicht später.

---

## 2. Was im Repo vor dem ersten Build zu ändern ist

Vier Funde aus der Prüfung, zwei davon würden den Upload oder die App auf dem
Gerät kaputtmachen.

### 2.1 BLOCKER: JNA 5.14.0 verletzt die 16-KB-Page-Size-Regel

Gemessen an den echten Bibliotheken auf dieser Maschine: `libjnidispatch.so`
aus JNA 5.14.0 hat auf `x86_64`, `armeabi-v7a` und `x86` LOAD-Segmente mit
`align 2**12` (4 KB). Google verlangt seit dem 01.11.2025 für neue Apps und
seit dem 01.05.2026 für Updates 16 KB — die Frist ist also längst aktiv, und
die Prüfung passiert beim Upload.

Alle anderen rund 20 Bibliotheken sind sauber. Auch dein eigenes
`libcal_ffi.so` ist bereits 16-KB-ausgerichtet (`align 2**14`) — das Risiko lag
also nicht dort, wo man es vermutet.

Behebung: JNA in `mobile/modules/cal-ffi/android/build.gradle` auf **5.17.0
oder höher** heben. Der Fix stammt aus JNA-Issue #1647.

### 2.2 BLOCKER: zwei ABI-Splits ohne `libcal_ffi.so`

`mobile/android/gradle.properties` paketiert vier Architekturen
(`armeabi-v7a,arm64-v8a,x86,x86_64`), der CI-Workflow baut den Rust-Kern aber
nur für zwei:

    cargo ndk -t arm64-v8a -t x86_64 ...

Play erzeugt aus dem AAB pro Architektur einen eigenen Split. Ein 32-Bit-Gerät
bekommt also einen Split ohne `libcal_ffi.so` und stirbt beim ersten Zugriff
mit `UnsatisfiedLinkError`.

Behebung, empfohlene Variante: die ABI-Liste auf `arm64-v8a` beschränken
(optional `x86_64` für den Emulator). Das löst zugleich 2.1, weil die
32-Bit-JNA-Bibliotheken dann gar nicht mehr mitgehen, und halbiert das Bundle.
Alternative: `-t armeabi-v7a -t x86` in den Workflow aufnehmen und die
passenden Rust-Targets installieren.

### 2.3 Erinnerungen laufen auf Android 12+ ungenau

`expo-notifications` 56.0.18 deklariert weder `SCHEDULE_EXACT_ALARM` noch
`USE_EXACT_ALARM` — geprüft in dessen `AndroidManifest.xml`, dort stehen nur
`RECEIVE_BOOT_COMPLETED` und `POST_NOTIFICATIONS`. Der Code
(`ExpoSchedulingDelegate.kt`) fragt `canScheduleExactAlarms()` ab und fällt
sonst auf `setAndAllowWhileIdle` zurück. Ergebnis: jede Erinnerung kann durch
Doze um Minuten bis Stunden verspätet feuern. Das trifft die Tagesstart-
Erinnerungen direkt.

Aperio erfüllt Googles Bedingung für `USE_EXACT_ALARM` wörtlich: erlaubt ist
sie unter anderem für "a calendar app that shows event notifications".
`USE_EXACT_ALARM` ist eine *normale* Berechtigung — automatisch gewährt, keine
Laufzeitabfrage, kein Nutzerschalter. Danach nimmt expo-notifications ohne
Codeänderung den exakten Zweig.

Eintragen in `mobile/app.json` unter `expo.android.permissions`:
`"android.permission.USE_EXACT_ALARM"`.

Restrisiko: ob ein Prüfer die Kalender-Einordnung akzeptiert, ist eine
Ermessensfrage. Fällt die Ablehnung, ist der Rückweg `SCHEDULE_EXACT_ALARM`
plus Anfrage-Fluss — deutlich mehr Arbeit.

### 2.4 Kleinkram, der beim ersten Upload auffällt

- **Vier überflüssige Berechtigungen.** `SYSTEM_ALERT_WINDOW`,
  `READ_EXTERNAL_STORAGE`, `WRITE_EXTERNAL_STORAGE` kommen aus Expos eigener
  Manifest-Vorlage (dort steht wörtlich "REMOVE WHATEVER YOU DO NOT NEED"),
  `CAMERA` aus `expo-image-picker`, obwohl `ContactEditorModal.tsx` nur
  `launchImageLibraryAsync` ruft. `SYSTEM_ALERT_WINDOW` erscheint Nutzern als
  "Über anderen Apps anzeigen" und wirkt bei einer Kalender-App befremdlich.
  Weg damit über `expo.android.blockedPermissions` in `app.json`.
  Wichtig zu wissen: `android.permissions` ist **additiv**, keine Positivliste —
  dort nur zwei Einträge zu haben unterdrückt nichts.
- **`allowBackup` steht auf `true`.** Android sichert dann
  `shared_prefs/aperio_secrets.xml` nach Drive, aber der geräte-gebundene
  KeyStore-Masterschlüssel wird nicht mitgesichert. Nach einer
  Wiederherstellung auf einem neuen Gerät sind alle Konten da und keins
  entschlüsselbar. Entweder `"allowBackup": false` setzen oder Auto-Backup
  gezielt konfigurieren.
- **`expo-notifications` fehlt in der `plugins`-Liste** von `app.json`. Expo
  wendet Config-Plugins nicht automatisch an. Ohne den Eintrag rechnet Android
  das farbige App-Icon zum Statusleisten-Symbol herunter — ein weißes Rechteck.
- **Store-Grafiken fehlen komplett.** Vorhanden sind nur `icon.png` (1024×1024)
  und die Adaptive-Icon-Teile. Play braucht: Icon **exakt 512×512** PNG
  (≤1024 KB), Feature-Grafik **1024×500** (JPEG oder 24-Bit-PNG, **ohne**
  Alphakanal), mindestens **2 Handy-Screenshots** (320–3840 px Kantenlänge,
  ohne Alpha). Die sieben Screenshots unter `mobile/.expo/` sind
  gitignorierte Entwicklungsartefakte.

Bereits in Ordnung, kein Handlungsbedarf: `targetSdk` ist über den
RN-0.85-Versionskatalog auf **36** — die Frist zum 31.08.2026 ist damit
erfüllt. `versionCode` regelt EAS server-seitig (`appVersionSource: "remote"` +
`autoIncrement`), also **kein** `android.versionCode` in `app.json` eintragen.
Das Produktionsprofil erzeugt korrekt ein `.aab`.

---

## 3. Erster Build

Der Keystore ist kein Problem mehr: seit eas-cli 18.2.0 (März 2026) erzeugt der
erste Android-Build den Upload-Keystore auch im nicht-interaktiven Modus. Du
hast 20.3.0.

```bash
eas build --platform android --profile production --non-interactive --wait
```

Erfolgssignal: das Kommando endet mit einer Build-URL und Status `finished`.
Zum Nachlesen ohne Pfeiltastenmenü:

```bash
eas build:list --platform android --build-profile production --status finished --limit 3 --json --non-interactive
```

Danach den Keystore sichern. `eas credentials` ist ausschließlich interaktiv
(einziger Schalter `-p/--platform`), also für NVDA lieber der Weg über die
Weboberfläche: expo.dev → Projekt `aperio` → `Credentials` → Android →
`com.aperio.mobile`.

Zur Einordnung, damit du weißt, was du da sicherst: **Google** hält den
eigentlichen App-Signaturschlüssel (Play App Signing ist für alle Apps seit
2021 verpflichtend, `com.aperio.mobile` ist automatisch dabei). Du hältst nur
den **Upload-Schlüssel**. Geht der verloren, ist das kein Totalverlust — man
erzeugt einen neuen und lässt ihn von Google registrieren.

Auf dem Gerät prüfen, bevor irgendetwas hochgeht — das `.aab` selbst ist nicht
installierbar, dafür ist das `preview`-Profil da:

```bash
eas build --platform android --profile preview --non-interactive --wait
```

```bash
eas build:run --platform android --latest
```

---

## 4. Erster Upload

Hier widersprechen sich die Quellen, und du solltest das wissen statt es zu
raten.

Expos Doku (Stand 21.07.2026) sagt, `eas submit` lege die erste Version einer
App problemlos an. Googles eigene Publishing-API-Doku (Stand 18.12.2025) sagt
weiterhin, man müsse mindestens ein Artefakt über die Console hochladen, bevor
die API nutzbar ist. Zwei offene eas-cli-Tickets (#3171, #3675) berichten
genau diesen Fehler bei wirklich leeren Apps.

Praktisch: **versuch es zuerst automatisch.** Wenn es scheitert, dann mit der
konkreten Fehlermeldung, nicht ins Blaue.

Der zuverlässige Weg für die allererste Einreichung ist `releaseStatus:
"draft"` im Submit-Profil — die Play-API weigert sich, auf einer App ohne jede
Veröffentlichung eine nicht-Entwurfs-Version anzulegen ("Only releases with
status draft may be created on draft app"). Den Rollout schließt du dann in der
Console ab.

Fehlermeldungen, die "lade einmal von Hand hoch" bedeuten:
`rolloutNotPermittedOnDraftApp`, "The app is missing the required metadata to
submit the app", "you will have to upload at least one APK through the Play
Console".

Von Hand hochladen geht so: Play Console → App öffnen → `Testen und
veröffentlichen` → `Testen` → `Interner Test` → Reiter `Releases` →
`Neuen Release erstellen` → `.aab` hochladen → Release-Name und
Versionshinweise → `Weiter` → `Rollout für Internen Test starten`.

Zwei Dinge, die beim ersten Mal irritieren und **keine Fehler** sind:

- Die App heißt in Play bis zu **48 Stunden** anders (Platzhaltername), bis das
  erste Review durch ist.
- Der Opt-in-Link braucht nach der ersten Veröffentlichung **einige Stunden**,
  bis er funktioniert.

Und einmal ausdrücklich: **auch der erste Rollout auf dem internen Test wird von
Google geprüft.** Googles Wortlaut: "If your app's first release roll-out is on
an Internal test track, the submission must be reviewed before it can be
published." Bei einem neuen Entwicklerkonto nennt Google bis zu sieben Tage,
in Ausnahmen länger. Jedes spätere interne Update geht dann in Minuten durch.

---

## 5. Tester eintragen

Alles auf der Spur `Interner Test`, Reiter `Tester`.

1. Unter der Überschrift `Tester` → `E-Mail-Liste erstellen`. Listenname
   eingeben, Adressen kommagetrennt einfügen, `Änderungen speichern` → `Erstellen`.
   Bei CSV: eine Adresse pro Zeile, **kein** UTF-8 mit BOM.
2. **Das Häkchen in der Zeile der Liste setzen und erneut `Änderungen
   speichern`.** Die Liste zu erstellen hängt sie nicht an die Spur an. Das ist
   die zweithäufigste Ursache für "App nicht verfügbar".
3. Feedback-Adresse eintragen (deine E-Mail genügt), speichern.
4. Ganz unten im Bereich `So nehmen Tester an deinem Test teil` unter
   `Im Web teilnehmen` den Link kopieren. Der Bereich erscheint erst, wenn die
   App den Status `Veröffentlicht` hat.
5. Falls unter `Übersicht über die Veröffentlichung` Punkte als "noch nicht zur
   Prüfung eingereicht" stehen: `Änderungen zur Prüfung senden`. Das gilt seit
   Anfang 2026 für alle Entwickler, nicht nur bei aktiviertem Managed
   Publishing.

Die beiden Linkformen sind **nicht** austauschbar:

- Interner Test: `play.google.com/apps/internaltest/<numerische Track-ID>`
- Geschlossen/Offen: `play.google.com/apps/testing/com.aperio.mobile`

Was der Tester tut: Link im Browser öffnen, in dem genau das eingetragene
Google-Konto angemeldet ist → `Tester werden` → dem Play-Store-Link folgen →
installieren. Suchen im Play Store funktioniert nicht, interne und geschlossene
Tests sind nicht auffindbar. Auf dem Gerät muss dasselbe Konto im Play Store
aktiv sein.

Grenzen und Regeln: bis zu 100 interne Tester, Länderbeschränkungen gelten für
den internen Test **nicht**. Tester brauchen ein Google- oder
Workspace-Konto; eine beliebige Fremdadresse trifft nie zu. Ein Konto, das im
internen Test angemeldet ist, bekommt **keine** geschlossenen Builds — benutze
für den späteren geschlossenen Test nicht dieselben Konten.

Von interner auf geschlossene Spur wechselt man ohne neuen Build: Release
auswählen → `Release hochstufen` → Zielspur.

---

## 6. Pflichtangaben in der Console

Für den **internen Test** kannst du fast alles auslassen — Google sagt
ausdrücklich, man könne einen internen Test starten, bevor die App fertig
eingerichtet ist, und Apps ausschließlich auf dieser Spur sind vom
Datensicherheits-Formular **befreit**.

Für **geschlossen, offen, Produktion** gilt das alles:

- **Datenschutzerklärung (URL).** Pflicht für jede App, unabhängig von
  Datenerhebung. Existiert bereits:
  `https://timtam.github.io/aperio/privacy/` (Quelle:
  `web/src/content/docs/privacy.md`, enthält schon einen Abschnitt zu den
  Mobil-Berechtigungen).
- **Datensicherheit.** Der entscheidende Begriff: "Erhebung" heißt
  *Übertragung vom Gerät herunter* — egal an wen. Aperios Provider-Sync fällt
  also darunter. Für "Weitergabe" greift Googles Ausnahme für
  nutzerinitiierte Übertragungen, weil der Nutzer das Zielkonto selbst
  verbindet: "Weitergabe = Nein" ist vertretbar. Beim E2E-verschlüsselten
  Sync-Ziel greift zusätzlich die Verschlüsselungs-Ausnahme.
  Zweckangabe: **nur** "App-Funktionalität" — "Kontoverwaltung" meint ein Konto
  *bei dir*, und das gibt es nicht.
- **Inhaltsbewertung** (IARC-Fragebogen). Ohne Bewertung keine
  Veröffentlichung.
- **Zielgruppe und Inhalte.** Altersbänder auswählen — für Aperio "18 und
  älter", keinesfalls etwas unter 13.
- **Werbe-ID:** Nein. **Anzeigen:** Nein.
- **Anmeldedaten** (früher "App-Zugriff"). Google verlangt funktionierende
  Testzugänge für alles, was hinter einem Login liegt, und macht keine
  Ausnahme für Anbieter, bei denen der Prüfer sich nicht selbst anmelden kann.
  Aperio ist lokal ohne jedes Konto benutzbar — das ist ein vertretbares
  "keine besonderen Zugangsdaten nötig". Sicherer wäre ein wegwerfbarer
  CalDAV-Testzugang ohne Zwei-Faktor. Deine Entscheidung.
- **Store-Eintrag.** App-Name 30 Zeichen, Kurzbeschreibung 80,
  Vollbeschreibung 4000. Plus die Grafiken aus 2.4.

Nicht nötig: der Kontolösch-Pfad. Der gilt nur für Apps, in denen man ein Konto
*anlegen* kann.

---

## 7. Automatisierung

Der GitHub-Workflow existiert bereits:
`.github/workflows/mobile-android-play.yml` (Commit `4666cf73`), auf `main` und
auf dem Branch. Er baut die Rust-`.so` mit `cargo-ndk`, erzeugt die
UniFFI-Kotlin-Bindings neu, macht einen kurzlebigen lokalen Commit, damit EAS
die Bibliotheken mitbekommt, und kann anschließend einreichen. Der Schalter
`submit` steuert das. Im Kopfkommentar steht der erste Lauf schon beschrieben:
`submit = false`.

Es fehlt genau eines: der Service-Account.

1. `console.cloud.google.com` → Projekt anlegen (beliebiger Name).
2. `IAM & Verwaltung` → `Dienstkonten` → `Dienstkonto erstellen`, z. B.
   `eas-play-publisher`. **Keine** Cloud-IAM-Rollen vergeben — die Rechte kommen
   aus der Play Console.
3. Beim Dienstkonto: `Schlüssel verwalten` → `Schlüssel hinzufügen` → `Neuen
   Schlüssel erstellen` → **JSON**. Die Datei wird heruntergeladen.
4. API aktivieren:
   `console.cloud.google.com/apis/library/androidpublisher.googleapis.com`.
5. Play Console → `Nutzer und Berechtigungen` → `Nutzer einladen` → die
   Dienstkonto-Adresse (`…@….iam.gserviceaccount.com`) → Rechte für die App:
   Releases auf Testspuren veröffentlichen, App-Informationen sehen.

Zur Verzögerung: die verbreitete Aussage "24 Stunden warten" steht so **nicht**
in Googles Doku. Googles aktuelle Seite sagt, die Berechtigung greife direkt
nach dem Einladen. Kommt trotzdem ein 401/403, kurz warten und erneut
versuchen — nicht das Dienstkonto neu bauen.

Dann entweder die Datei nach `mobile/google-service-account.json` legen
(bereits korrekt gitignoriert) oder als GitHub-Secret
`GOOGLE_SERVICE_ACCOUNT_JSON` hinterlegen, das der Workflow selbst schreibt.

Ab dann:

```bash
eas submit --platform android --profile production --latest --non-interactive
```

Oder in einem Rutsch:

```bash
eas build --platform android --profile production --non-interactive --wait --auto-submit
```

`--auto-submit` beim allerersten Mal noch nicht benutzen — du willst einen
Submit-Fehler getrennt vom Build sehen.

Für den späteren geschlossenen Test genügt es, in `eas.json` unter
`submit.production.android` `"track"` von `"internal"` auf `"alpha"` zu ändern.
Der Spurname ist zugleich der API-Bezeichner, exakt und mit Groß-/Kleinschreibung.

---

## 8. Prüfrezepte

Berechtigungen im tatsächlich ausgelieferten Manifest, textbasiert:

```bash
cd C:/scripts/aperio/mobile && npx expo prebuild -p android --clean
```

Danach in `mobile/android` `./gradlew :app:processReleaseMainManifest` laufen
lassen und
`android/app/build/intermediates/merged_manifest/release/processReleaseMainManifest/AndroidManifest.xml`
lesen. Das ist das gemergte Ergebnis inklusive aller Bibliotheken. Beachte: das
`android/`-Verzeichnis auf der Platte ist ein alter Prebuild vom 17.06.2026,
`app.json` wurde danach geändert — ohne `--clean` liest du Veraltetes.

16-KB-Ausrichtung aller `.so` (das Rezept, mit dem der JNA-Fund gemacht wurde):

```bash
OD="C:/Users/Toni/AppData/Local/Android/Sdk/ndk/27.1.12297006/toolchains/llvm/prebuilt/windows-x86_64/bin/llvm-objdump.exe"; for f in *.so; do printf "%-36s " "$(basename $f)"; "$OD" -p "$f" | grep -A1 '^    LOAD' | grep -o 'align 2\*\*[0-9]*' | sort -u | tr '\n' ' '; echo; done
```

Bestanden ist alles ab `2**14`. Jedes `2**12` ist ein Ablehnungsgrund.

Am fertigen AAB: `bundletool dump config --bundle=app.aab` muss
`PAGE_ALIGNMENT_16K` zeigen.

Größe: Plays Grenze sind 200 MB **komprimierter Download pro Gerät**, nicht die
AAB-Dateigröße. Die lokalen `.so` sind mit 370 MB riesig, weil sie ungestrippte
Entwicklungs-Builds sind; `[profile.release] strip = true` in der
Workspace-`Cargo.toml` sorgt dafür, dass CI etwas ganz anderes produziert.
Nachsehen nach dem ersten Upload unter `App-Bundle-Explorer` → `Downloads`.

---

## 9. Wenn ein Tester "Diese App ist nicht verfügbar" sieht

In dieser Reihenfolge prüfen, dauert zwei Minuten:

1. Welchen Link hast du geschickt — `/apps/internaltest/` oder
   `/apps/testing/`? Passt er zur Spur, auf die du veröffentlicht hast?
2. Reiter `Tester`: ist das Häkchen der Liste gesetzt **und gespeichert**?
3. Reiter `Releases`: gibt es einen echten Release mit Versionscode und
   Live-Status — nicht `Entwurf`, nicht `Angehalten`, nicht "in Prüfung"?
4. `Übersicht über die Veröffentlichung`: hängen Änderungen ungesendet?
5. Steht die Adresse zeichengenau in der Liste?
6. Nur bei geschlossen/offen: ist das Land des Testers freigegeben?
7. Ist derselbe Tester zufällig im internen Test angemeldet? Dann bekommt er
   keine geschlossenen Builds.

Zusatzsymptom: Link funktioniert im Browser, der Play Store zeigt trotzdem
nichts → Play Store beenden und Cache leeren, dann einige Stunden Geduld. Die
Opt-in-Zähler in der Console laufen 24–48 Stunden hinterher.

---

## 10. Was ich nicht prüfen konnte

- **Kontotyp und Anlagedatum** deines Play-Kontos. Davon hängt ab, ob die
  Produktion drei Tage oder fünf Wochen entfernt ist.
- **Ob `eas submit` die allererste Version anlegen darf.** Quellen
  widersprechen sich; der Fehlertext beim Versuch entscheidet.
- **Die tatsächliche Größe der CI-gebauten `.so`.** Lokal liegen nur
  ungestrippte Debug-Artefakte.
- **Ob Play beim 16-KB-Test nur `arm64-v8a` oder alle ABIs bewertet.** Googles
  Doku nennt beide, also als blockierend behandeln.
- **Ob ein Prüfer `USE_EXACT_ALARM` für Aperio akzeptiert.** Ermessensfrage.
