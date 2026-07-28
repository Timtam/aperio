# Aperio – Design-Dokument

Eine barrierefreie, plattformübergreifende Kalender- und Aufgaben-App.

**Version:** 0.1 (Entwurf)
**Status:** In Bearbeitung – Design/Layout-Spezifikation ausstehend
**Letzte Aktualisierung:** Mai 2026

---

## Inhaltsverzeichnis

1. [Projektziel & Vision](#1-projektziel--vision)
2. [Technologie-Stack](#2-technologie-stack)
3. [Barrierefreiheit](#3-barrierefreiheit)
4. [Architektur-Übersicht](#4-architektur-übersicht)
5. [Ansichten](#5-ansichten)
6. [Adapter-Architektur](#6-adapter-architektur)
7. [Terminerstellung & -verwaltung](#7-terminerstellung---verwaltung)
8. [Farb-Labels](#8-farb-labels)
9. [Aufgaben-Management](#9-aufgaben-management)
10. [Kontakte & CardDAV-Integration](#10-kontakte--carddav-integration)
11. [Videokonferenz-Integration](#11-videokonferenz-integration)
12. [Feiertage (per iCal-Abonnement)](#12-feiertage-per-ical-abonnement)
13. [Suche](#13-suche)
14. [Erinnerungen & Benachrichtigungen](#14-erinnerungen--benachrichtigungen)
15. [Native Desktop-Erfahrung & Tastaturkürzel](#15-native-desktop-erfahrung--tastaturkürzel)
16. [Lokalisierung (i18n)](#16-lokalisierung-i18n)
17. [Systemintegration](#17-systemintegration)
18. [Offline-Fähigkeit & Datensynchronisation](#18-offline-fähigkeit--datensynchronisation)
19. [Geräteübergreifende Datenbanksynchronisation](#19-geräteübergreifende-datenbanksynchronisation)
20. [Plugin-System](#20-plugin-system)
21. [Self-Update-System](#21-self-update-system)
22. [Build- & Release-Workflow](#22-build---release-workflow)
23. [Dateistruktur (Projektlayout)](#23-dateistruktur-projektlayout)
24. [Dokumentation](#24-dokumentation)
25. [Offene Punkte & Ausstehend](#25-offene-punkte--ausstehend)

---

## 1. Projektziel & Vision

Entwicklung einer modernen, vollständig barrierefreien Desktop-App für **Kalender und Aufgaben**, die folgende Kernprinzipien erfüllt:

- **Barrierefreiheit als erste Priorität:** Vollständige Kompatibilität mit allen gängigen Screen Readern (NVDA, JAWS, Narrator, VoiceOver, Orca)
- **Keyboard-first:** Jede Funktion ohne Maus bedienbar; gleichzeitig exzellente Maus-Unterstützung für sehende Nutzer
- **Kalender und Aufgaben gleichrangig:** Beide sind Kern-Features mit eigenem Datenmodell, eigener Sync-Logik und eigener UI – Aufgabenlisten existieren auch unabhängig von Kalendern (z.B. Vikunja, Todoist)
- **Leichtgewichtig & portabel:** Keine feste Installation erforderlich, einzelne ausführbare Datei
- **Plattformübergreifend:** Windows, macOS, Linux
- **Offene Erweiterbarkeit:** Plugin-System für Daten-Adapter (Kalender, Aufgaben, Kontakte), Sync-Adapter, Videokonferenz-Anbieter und zukünftige Erweiterungen

---

## 2. Technologie-Stack

### 2.1 Empfehlung: Tauri (v2)

Nach Abwägung der Alternativen wird **Tauri** als Technologiebasis empfohlen:

| Kriterium | Tauri | Electron | Reines Rust (egui/iced) |
|---|---|---|---|
| Barrierefreiheit (ARIA) | ✅ Sehr gut (Web-Frontend) | ✅ Sehr gut | ⚠️ Unreif |
| Gewicht / Portabilität | ✅ ~5–15 MB Binary | ❌ ~80–150 MB | ✅ Sehr klein |
| Kalender-Bibliotheken | ✅ Gut (JS-Ökosystem) | ✅ Gut | ⚠️ Lücken |
| Rust-Backend | ✅ Nativ | ❌ Node.js | ✅ Nativ |
| Screen-Reader-Reife | ✅ Hoch | ✅ Hoch | ❌ Niedrig |

**Begründung:** Tauri kombiniert ein performantes Rust-Backend mit einem Web-Frontend, das ARIA-Barrierefreiheit vollständig unterstützt. Die portable Einzelbinary-Anforderung wird durch Tauris Build-System erfüllt.

### 2.2 Stack-Komponenten

| Schicht | Technologie | Zweck |
|---|---|---|
| Backend (Core) | Rust + Tauri v2 | Datenhaltung, Sync, Geschäftslogik |
| Frontend (UI) | TypeScript + React | Kalender-, Aufgaben- und Kontakt-UI, Barrierefreiheit |
| Styling | CSS + Tailwind | Design-System |
| Lokale Datenbank | SQLite (via `rusqlite`) | Offline-Datenhaltung |
| Datenprotokolle | CalDAV / CardDAV / iCalendar / REST | Google, Outlook, iCloud, Vikunja, Todoist etc. |
| HTTP-Client | `reqwest` (Rust) | API-Kommunikation |
| i18n | `i18next` (Frontend) | Mehrsprachigkeit |

### 2.3 Wichtige Rust-Crates

```toml
[dependencies]
tauri = { version = "2", features = ["updater"] }
rusqlite = { version = "0.31", features = ["bundled"] }
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"         # Async-Traits für Adapter
icalendar = "0.16"          # iCal-Parsing (VEVENT, VTODO, VALARM)
oauth2 = "4"                # OAuth2 für Daten-, Sync- und Videokonferenz-Adapter
keyring = "2"               # Sichere Speicherung von Zugangsdaten
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.9"           # Zeitzonen
libloading = "0.8"          # Dynamisches Laden von Plugin-Bibliotheken
aes-gcm = "0.10"            # AES-256-GCM für E2E-Verschlüsselung
argon2 = "0.5"              # Schlüsselableitung aus Passwort
dirs = "5"                  # Plattformspezifische Systemverzeichnisse (Fallback-Datenpfad)
```

### 2.4 Wichtige JS/TS-Bibliotheken (Frontend)

```json
{
  "dependencies": {
    "react": "^18",
    "@tauri-apps/api": "^2",
    "i18next": "^23",
    "react-i18next": "^14",
    "date-fns": "^3",
    "date-fns-tz": "^3"
  }
}
```

### 2.5 Windows-Portabilität & WebView2

Tauri erzeugt unter Windows standardmäßig NSIS-Installer (`.exe`) und MSI-Pakete. Für die angestrebte portable Einzelbinary wird der Bundle-Schritt deaktiviert (`"bundle": {"active": false}` in `tauri.conf.json`), wodurch `tauri build` direkt eine standalone `.exe` ohne Installer-Wrapper erzeugt (siehe Abschnitt 22.2).

#### WebView2-Abhängigkeit

Tauri nutzt unter Windows **WebView2** (Microsoft Edge Chromium) als Rendering-Engine. WebView2 wird standardmäßig **nicht** in die Binary gebündelt, sondern als systemseitige Installation vorausgesetzt. Das hat folgende Implikationen:

| Szenario | Verhalten |
|---|---|
| Windows 11 | ✅ WebView2 immer vorinstalliert |
| Windows 10 (aktuell, ab 2021) | ✅ WebView2 via Windows Update vorinstalliert |
| Windows 10 (älter, ohne Updates) | ⚠️ WebView2 fehlt möglicherweise |
| Windows Server | ⚠️ WebView2 nicht immer vorhanden |

#### Gewählte Strategie: Evergreen mit Bootstrap-Fallback

Die App nutzt die systemseitige WebView2-Installation ("Evergreen"). Falls WebView2 beim Start fehlt, lädt die App automatisch einen schlanken Bootstrap-Installer herunter und führt ihn aus:

```json
// tauri.conf.json (Auszug – WebView2-relevanter Teil; vollständige Konfiguration siehe Abschnitt 22.2)
{
  "bundle": {
    "windows": {
      "webviewInstallMode": {
        "type": "downloadBootstrapper"
      }
    }
  }
}
```

Der Bootstrap-Prozess ist einmalig, erfordert keine Admin-Rechte (benutzerspezifische WebView2-Installation) und ist für den Nutzer transparent mit einem Fortschrittsdialog versehen.

#### Verworfene Alternative: Fixed WebView2

Eine vollständig portable Variante ohne jegliche Systemabhängigkeit wäre durch Mitbündeln der WebView2-Runtime als "Fixed Version Distribution" möglich. Dies wurde verworfen, da die Binary damit auf ~150–200 MB anwächst – unvereinbar mit dem Leichtgewichts-Anspruch der App. Der Bootstrap-Fallback deckt den Sonderfall älterer Systeme hinreichend ab.

---

## 3. Barrierefreiheit

Barrierefreiheit ist keine nachträgliche Ergänzung, sondern ein Kernprinzip, das alle Architektur- und UI-Entscheidungen beeinflusst.

### 3.1 Unterstützte Screen Reader

| Screen Reader | Plattform | Priorität |
|---|---|---|
| NVDA | Windows | Hoch |
| JAWS | Windows | Hoch |
| Narrator | Windows | Hoch |
| VoiceOver | macOS | Hoch |
| Orca | Linux | Mittel |

### 3.2 ARIA-Anforderungen

- Alle interaktiven Elemente haben semantisch korrekte ARIA-Rollen (`role="grid"`, `role="listbox"`, `role="dialog"`, etc.)
- Dynamische Inhalte werden über `aria-live`-Regionen angekündigt (z.B. beim Navigieren zum nächsten Tag)
- Modale Dialoge implementieren korrekte Fokus-Fallen (`aria-modal="true"`, Fokus-Rückgabe beim Schließen)
- Alle Bilder/Icons haben `aria-label` oder `aria-hidden="true"` falls dekorativ
- Formularfelder haben explizite `<label>`-Elemente, nicht nur `placeholder`

#### 3.2.1 `role="application"` – Dauerhafter Fokus-Modus

Die wichtigste einzelne ARIA-Entscheidung für das native Gefühl ist die Verwendung von `role="application"` auf dem Root-Element der App:

```html
<div id="app-root" role="application" aria-label="Aperio">
  <!-- Gesamte App-UI -->
</div>
```

**Warum das entscheidend ist:** NVDA (und andere Screen Reader) unterscheiden zwischen Browse-Modus und Fokus-Modus. Im Browse-Modus behandelt NVDA den Inhalt wie eine Webseite – Buchstaben werden als Schnellnavigationstasten interpretiert, Pfeiltasten lesen Text linear. Das fühlt sich für eine Desktop-Anwendung wie Aperio falsch an.

`role="application"` signalisiert dem Screen Reader: "Das ist eine Anwendung, keine Webseite." NVDA startet direkt im Fokus-Modus und bleibt dort – Tasten werden direkt an die App durchgereicht, genau wie bei einer nativen Desktop-Anwendung. Synology DSM 7 nutzt exakt dieses Muster.

**Wichtige Einschränkung:** `role="application"` deaktiviert die automatische Lesbarkeit von statischem Text. Deshalb müssen alle informativen Inhalte explizit per `aria-label`, `aria-describedby` oder `aria-live` zugänglich gemacht werden – es gibt keinen Fallback auf lineares Lesen mehr.

**Ausnahmen innerhalb der App:** Bereiche mit dokumentartigem Charakter (z.B. die Beschreibung eines Termins im Detail-Panel, falls sie längeren Fließtext enthält) erhalten lokal `role="document"`, damit NVDA dort temporär in den Browse-Modus wechseln kann:

```html
<div role="document" aria-label="Terminbeschreibung">
  Längerer Beschreibungstext des Termins...
</div>
```

Der Nutzer kann in solchen Bereichen bewusst mit `NVDA+Space` zwischen den Modi wechseln.

### 3.3 Navigationsmuster je Ansicht

#### Grundprinzip: Zwei-Ebenen-Navigation (Outlook-Modell)

Die Kalendernavigation folgt dem bewährten Muster aus Microsoft Outlook, das zwei konzeptuelle Ebenen sauber trennt:

| Ebene | Tasten | Zweck |
|---|---|---|
| **Raumnavigation** | Pfeiltasten | Zwischen Tagen / Wochen / Zeitslots wechseln |
| **Inhaltsnavigation** | `Tab` / `Shift+Tab` | Zwischen Terminen und Aufgaben am aktuell fokussierten Tag wechseln |

Das verhindert, dass man beim Durchblättern von Terminen unbeabsichtigt den Tag wechselt – ein häufiges Problem bei naiveren Implementierungen.

#### Wochenansicht (Grid-Navigation)

Die Wochenansicht zeigt 7 Tage. Die KW-Nummer wird im Header direkt neben dem Datumsbereich angezeigt.

```
KW 20    Mo        Di        Mi        Do        Fr        Sa        So
       [12. Mai] [13. Mai] [14. Mai] [15. Mai] [16. Mai] [17. Mai] [18. Mai]
```

| Taste | Aktion |
|---|---|
| `←` / `→` | Vorheriger / nächster Tag |
| `↑` / `↓` | Vorherige / nächste Woche |
| `Page Up` / `Page Down` | Vorherige / nächste Woche (alternative Navigation) |
| `Tab` | Nächster Eintrag (Termin oder Aufgabe) am fokussierten Tag |
| `Shift+Tab` | Vorheriger Eintrag am fokussierten Tag |
| `Enter` / `Space` | Fokussierten Eintrag öffnen |
| `Escape` | Zurück zur Tag-Navigation (Fokus vom Eintrag auf den Tag) |

ARIA-Umsetzung: `role="grid"` auf dem Kalender-Container, `role="gridcell"` pro Tag, `aria-selected="true"` auf dem fokussierten Tag. Beim Wechsel zu einem neuen Tag wird per `aria-live="polite"` angekündigt: "Mittwoch, 14. Mai 2025, 2 Termine, 1 Aufgabe."

Beim Wechsel auf einen Tag mit Einträgen per `Tab`: "Eintrag 1 von 3: Teammeeting, 10:00 bis 11:00 Uhr."

#### Tagesansicht (Zeitraster-Navigation)

```
08:00  ────────────────────────────
09:00  [Standup, 09:00–09:15]
10:00  [Teammeeting, 10:00–11:00]
11:00  ────────────────────────────
```

| Taste | Aktion |
|---|---|
| `↑` / `↓` | Vorheriger / nächster Zeitslot (15-Minuten-Schritte, konfigurierbar) |
| `Tab` | Nächster Termin im Tagesraster |
| `Shift+Tab` | Vorheriger Termin im Tagesraster |
| `←` / `→` | Vorheriger / nächster Tag (bei leerem Zeitslot) |
| `Enter` / `Space` | Fokussierten Termin öffnen |
| `N` | Neuen Termin am fokussierten Zeitslot erstellen |

ARIA-Umsetzung: `role="listbox"` auf dem Zeitraster, Zeitslots als `role="option"`, Termine als `role="option"` mit `aria-label="Teammeeting, 10:00 bis 11:00 Uhr, Kalender: Arbeit"`.

#### Agenda-Ansicht (Listen-Navigation)

| Taste | Aktion |
|---|---|
| `↑` / `↓` | Vorheriger / nächster Termin in der Liste |
| `Enter` / `Space` | Termin öffnen |
| `←` / `→` | Zum vorherigen / nächsten Tag springen (Datumsgruppe) |

ARIA-Umsetzung: `role="list"`, Datumsgruppen als `role="listitem"` mit `aria-label` (Datum + Anzahl Termine), Termine als verschachtelte `role="listitem"`.

#### Monatsansicht (Grid-Navigation)

Identisches Muster wie Wochenansicht, aber auf Monatsebene:

| Taste | Aktion |
|---|---|
| `←` / `→` | Vorheriger / nächster Tag |
| `↑` / `↓` | Vorherige / nächste Woche |
| `Tab` | Nächster Eintrag (Termin oder Aufgabe) am fokussierten Tag |
| `Shift+Tab` | Vorheriger Eintrag am fokussierten Tag |
| `Enter` / `Space` | Eintrag öffnen oder Tagesansicht für diesen Tag öffnen |
| `Page Up` / `Page Down` | Vorheriger / nächster Monat |

Ankündigung beim Tageswechsel: "Donnerstag, 15. Mai 2025, 1 Termin, 2 Aufgaben."

#### Jahresansicht

| Taste | Aktion |
|---|---|
| `←` / `→` | Vorheriger / nächster Monat |
| `↑` / `↓` | Selber Monat im Vorjahr / Folgejahr |
| `Enter` | Monat in Monatsansicht öffnen |

### 3.4 Tastaturnavigation

Alle Ansichten müssen vollständig per Tastatur navigierbar sein. Die vollständige Tastaturkürzel-Referenz befindet sich in Abschnitt 15.7. Kernprinzipien:

| Tastenkombination | Aktion |
|---|---|
| `Tab` / `Shift+Tab` | Fokus zwischen Hauptbereichen |
| `Pfeiltasten` | Navigation innerhalb einer Ansicht (Tage, Wochen, etc.) |
| `Enter` / `Space` | Eintrag öffnen / Aktion auslösen (Space: Aufgabe abhaken in der Aufgaben-Ansicht) |
| `Ctrl+N` | Termin schnell anlegen (Quick-Add) |
| `Ctrl+Shift+N` | Neuer Termin (vollständiges Formular) |
| `Alt+N` | Aufgabe schnell anlegen (Quick-Add) |
| `Alt+Shift+N` | Neue Aufgabe (vollständiges Formular) |
| `Escape` | Dialog schließen / Aktion abbrechen |
| `Ctrl+←` / `Ctrl+→` | Vorherige / nächste Periode |
| `Ctrl+1–6` | Ansicht wechseln (6 = Aufgaben-Ansicht) |
| `Ctrl+T` | Zur heutigen Ansicht springen |
| `F6` | Zwischen Haupt-Navigationsbereichen wechseln |
| `Ctrl+H` / `Ctrl+/` (macOS) | Tastaturkürzel-Overlay öffnen |

### 3.5 Farbe & Kontrast

- Mindest-Kontrastverhältnis: **4.5:1** (WCAG AA) für normalen Text, **3:1** für großen Text
- Niemals Farbe als einziges Unterscheidungsmerkmal verwenden (z.B. Kalender-Farben müssen zusätzlich durch Muster oder Labels unterscheidbar sein)
- Dark Mode und High-Contrast-Mode werden unterstützt

### 3.6 Ankündigungen & Live-Regionen

```html
<!-- Beispiel: Ankündigung beim Navigieren -->
<div aria-live="polite" aria-atomic="true" class="sr-only">
  Montag, 12. Mai 2025 – 3 Termine, 2 Aufgaben
</div>
```

---

## 4. Architektur-Übersicht

```
┌────────────────────────────────────────────────────────────────────┐
│                          Tauri App Shell                           │
│                                                                    │
│  ┌─────────────────────┐    ┌───────────────────────────────────┐  │
│  │   React Frontend    │    │         Rust Backend Core         │  │
│  │  ┌───────────────┐  │    │  ┌─────────────────────────────┐  │  │
│  │  │  Kalender-UI  │  │    │  │ SQLite (lokaler Cache)      │  │  │
│  │  │  (ARIA/i18n)  │◄─┼────┼─►│ Sync Queue / Puffer         │  │  │
│  │  └───────────────┘  │    │  └─────────────────────────────┘  │  │
│  │  ┌───────────────┐  │    │  ┌─────────────────────────────┐  │  │
│  │  │  Aufgaben-UI  │  │    │  │ Plugin-Manager              │  │  │
│  │  └───────────────┘  │    │  │ ┌────────┐ ┌────────────┐   │  │  │
│  │  ┌───────────────┐  │    │  │ │Kalend.-│ │   Sync-    │   │  │  │
│  │  │ Einstellungen │  │    │  │ │Adapter │ │  Adapter   │   │  │  │
│  │  └───────────────┘  │    │  │ └────────┘ └────────────┘   │  │  │
│  │                     │    │  │ ┌────────┐ ┌────────────┐   │  │  │
│  │                     │    │  │ │  VC-   │ │Notification│   │  │  │
│  │                     │    │  │ │Adapter │ │            │   │  │  │
│  │                     │    │  │ └────────┘ └────────────┘   │  │  │
│  │                     │    │  └─────────────────────────────┘  │  │
│  │                     │    │  ┌─────────────────────────────┐  │  │
│  │                     │    │  │ Event Log (Append-Only)     │  │  │
│  │                     │    │  │ Self-Update-Manager         │  │  │
│  │                     │    │  └─────────────────────────────┘  │  │
│  └─────────────────────┘    └───────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────┘
         │ Kalender-Adapter         │ Sync-Adapter       │ VC-Adapter
         ▼                          ▼                    ▼
 Google / MS / CalDAV    WebDAV / Dropbox / SFTP   Zoom / Teams /
 EWS / iCal / Lokal      Google Drive / Lokal      Meet / WebEx
 Vikunja / Todoist
```

### 4.1 Kommunikation Frontend ↔ Backend

Tauri-Commands werden typisiert über eine gemeinsame API-Schicht definiert:

```rust
// Rust Backend
#[tauri::command]
async fn get_events(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Vec<Event>, String> { ... }

#[tauri::command]
async fn create_event(event: CreateEventRequest) -> Result<Event, String> { ... }
```

```typescript
// TypeScript Frontend
import { invoke } from '@tauri-apps/api/core';
const events = await invoke<Event[]>('get_events', { start, end });
```

---

## 5. Ansichten

### 5.1 Übersicht aller Ansichten

| Ansicht | Beschreibung | Screen-Reader-Muster |
|---|---|---|
| **Tagesansicht** | Detaillierte Stundenraster-Ansicht eines Tages | Listen-basiert |
| **Wochenansicht** | 7 Tage (konfigurierbarer Wochenanfang) mit KW-Anzeige | Tabellen-basiert |
| **Monatsansicht** | Klassisches Kalender-Grid | Tabellen-basiert |
| **Jahresansicht** | 12 Monate als Mini-Grids | Tabellen-basiert |
| **Agenda-Ansicht** | Chronologische Terminliste | Listen-basiert |
| **Aufgaben-Ansicht** | Dedizierte Aufgaben-Verwaltung – siehe Abschnitt 9.8 | Listen-basiert |

### 5.2 Wochenstart & Kalenderwochen-Anzeige

- Wochenanfang ist frei konfigurierbar (Montag / Sonntag / beliebig)
- Kalenderwochen werden nach **ISO 8601** berechnet
- In der Wochenansicht wird die KW-Nummer direkt im Ansichts-Header angezeigt, zusammen mit dem Datumsbereich
- Anzeige-Beispiel: `KW 20 · 12.–18. Mai 2025`
- In der Monatsansicht kann optional die KW-Nummer am Zeilenanfang jeder Woche eingeblendet werden (konfigurierbar)

### 5.3 Heute-Markierung & Navigation

- Der aktuelle Tag ist in allen Ansichten visuell **und per ARIA** (`aria-current="date"`) markiert
- Schnellnavigation: `Ctrl+T` springt immer zur heutigen Ansicht
- "Mini-Kalender" als Sidebar-Widget für schnelle Datumsauswahl (optional ein-/ausblendbar)

---

## 6. Adapter-Architektur

### 6.1 Crate-Struktur & Traits

#### Cargo-Workspace

Jeder Adapter wird als **eigenständiges Crate** in einem Cargo-Workspace entwickelt. Das ermöglicht unabhängige Versionierung, separate Tests und spätere Auskopplung als öffentliche Crates auf crates.io oder in separate Repositories – ohne Änderung an der Aperio-Architektur.

```
Aperio/
├── Cargo.toml                        # Workspace-Root
├── crates/
│   ├── cal-core/                     # Gemeinsame Typen & Traits
│   ├── plugin-core/                  # Plugin-ABI (C-Header), Plugin-Manager
│   ├── plugin-sdk/                   # Rust-SDK für Plugin-Entwickler
│   ├── sync-core/                    # Sync-Adapter-Trait & Event-Log-Typen
│   ├── cal-adapter-google/           # Google Calendar + Tasks + Contacts (nativ gebundelt)
│   ├── cal-adapter-microsoft-graph/  # Outlook + MS To Do + Contacts (nativ gebundelt)
│   ├── cal-adapter-ews/              # Exchange on-premise (Kalender + Tasks + Contacts, nativ gebundelt)
│   ├── cal-adapter-caldav/           # CalDAV + CardDAV, inkl. iCloud (nativ gebundelt)
│   ├── cal-adapter-ical/             # .ics-Dateien (nativ gebundelt)
│   ├── cal-adapter-local/            # Lokaler Kalender (host-intern, kein Plugin — siehe §20.2)
│   ├── cal-adapter-vikunja/          # Vikunja REST API (nativ gebundelt)
│   ├── cal-adapter-todoist/          # Todoist REST API (nativ gebundelt)
│   ├── sync-adapter-webdav/          # WebDAV (nativ gebundelt)
│   ├── sync-adapter-ftp/             # FTPS (nativ gebundelt)
│   ├── sync-adapter-sftp/            # SFTP (nativ gebundelt)
│   ├── sync-adapter-dropbox/         # Dropbox API v2 (nativ gebundelt)
│   ├── sync-adapter-googledrive/     # Google Drive API v3 (nativ gebundelt)
│   ├── sync-adapter-local/           # Lokales Dateisystem / NAS (nativ gebundelt)
│   ├── vc-adapter-zoom/              # Zoom (nativ gebundelt)
│   ├── vc-adapter-teams/             # Microsoft Teams (nativ gebundelt)
│   ├── vc-adapter-meet/              # Google Meet (nativ gebundelt)
│   └── vc-adapter-webex/             # Cisco WebEx (nativ gebundelt)
└── src-tauri/                        # Tauri-App
```

> **Vollständige Projektstruktur** inkl. Frontend-Komponenten, Plugin-Erweiterungen und Dokumentation: siehe Abschnitt 23.

#### Bundling-Strategie und Auskopplungsfähigkeit

Alle Adapter, die in diesem Dokument spezifiziert sind, werden **mit der Haupt-App zusammen entwickelt und ausgeliefert** ("nativ gebundelt"). Das gilt für alle Kalender-, Aufgaben-, Kontakt-, Sync- und Videokonferenz-Adapter – einschließlich Vikunja, Todoist, EWS, FTP, SFTP, iCloud usw.

Damit gilt:

- **Einheitliche Wartung:** Adapter-Updates erfolgen mit App-Releases; kein Drift zwischen Aperio-Version und Adapter-Version
- **Mobile-tauglich:** Für eine spätere Mobile-Portierung (Abschnitt 25.1) werden alle gebundelten Adapter statisch in die App einkompiliert (Feature-Flag `static-plugins`, siehe Abschnitt 20.6) – dynamisches Plugin-Laden ist auf Mobile nicht möglich. Da alle relevanten Adapter gebundelt sind, fehlt auf Mobile keine Funktionalität
- **Crates.io-Wiederverwendbarkeit:** Jeder Adapter ist eine reine Rust-Crate und kann unabhängig auf crates.io veröffentlicht werden
- **Auskopplung ohne Architektur-Bruch:** Soll ein Adapter später aus dem Workspace in ein eigenes Repository wandern (z.B. weil die Wartung an einen externen Maintainer übergeht), genügt:
  1. Crate-Verzeichnis in das neue Repo verschieben
  2. Eintrag in `Cargo.toml` (Workspace) entfernen
  3. Im Haupt-Repo eine Git-Submodul- oder Pfad-Referenz hinterlassen, falls weiter gebundelt werden soll – andernfalls verschwindet der Adapter aus zukünftigen Aperio-Builds und wird zum Community-Plugin

Damit das ohne Reibung funktioniert, gelten zwei strikte Regeln:

- **Keine Cross-Adapter-Abhängigkeiten:** Adapter dürfen nur von `cal-core` (bzw. `sync-core` / `plugin-sdk`) abhängen, niemals voneinander
- **Stabile Trait-Schnittstelle:** Die in `cal-core` definierten Traits werden semantisch versioniert; Breaking Changes nur in Major-Releases

#### Plugin-System bleibt als Erweiterungspunkt erhalten

Der Plugin-Mechanismus (Abschnitt 20) ist **weiterhin** Bestandteil der Architektur – nicht weil Aperio-eigene Adapter ihn brauchen (sie sind alle gebundelt), sondern damit Drittentwickler ohne Patch der Aperio-Quellen eigene Adapter für proprietäre oder Nischen-Systeme bauen können. Konkrete Beispiele: ein Adapter für eine firmeninterne Kalenderlösung, ein neuer Aufgaben-Dienst, eine Sync-Backend-Anbindung an ein NAS mit proprietärem Protokoll.

#### Das `cal-core`-Crate

`cal-core` definiert ausschließlich die gemeinsamen Datentypen und die Adapter-Traits (`Adapter`, `CalendarFeature`, `TasksFeature`, `ContactsFeature`). Es hat **keine Abhängigkeit** zu einem konkreten Adapter – alle Adapter hängen von `cal-core` ab, nicht umgekehrt.

```rust
// crates/cal-core/src/lib.rs

/// Basis-Trait für alle Adapter. Jeder Adapter implementiert dieses Trait und
/// mindestens eines der drei Feature-Traits (CalendarFeature, TasksFeature,
/// ContactsFeature) entsprechend seiner deklarierten Capabilities.
#[async_trait]
pub trait Adapter: Send + Sync {
    async fn authenticate(&self, credentials: Credentials) -> Result<AuthToken>;
    fn capabilities(&self) -> &[Capability];
}

pub enum Capability { Calendar, Tasks, Contacts }

/// Implementiert von Adaptern mit `"capabilities": ["calendar"]`.
#[async_trait]
pub trait CalendarFeature: Adapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>>;
    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>>;
    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> Result<Event>;
    async fn update_event(&self, event: Event) -> Result<Event>;
    async fn delete_event(&self, event_id: &str) -> Result<()>;
    async fn get_free_busy(&self, emails: &[&str], range: DateRange) -> Result<FreeBusy>;
    fn calendar_color(&self, calendar_id: &str) -> Option<ContainerColor>;
}

/// Implementiert von Adaptern mit `"capabilities": ["tasks"]`.
#[async_trait]
pub trait TasksFeature: Adapter {
    async fn list_task_lists(&self) -> Result<Vec<TaskList>>;
    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>>;
    async fn create_task(&self, list_id: &str, task: NewTask) -> Result<Task>;
    async fn update_task(&self, task: Task) -> Result<Task>;
    async fn delete_task(&self, task_id: &str) -> Result<()>;
}

/// Implementiert von Adaptern mit `"capabilities": ["contacts"]`.
#[async_trait]
pub trait ContactsFeature: Adapter {
    async fn list_contacts(&self) -> Result<Vec<Contact>>;
    async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>>;
}

pub struct ContainerColor {
    pub hex: String,          // z.B. "#4285f4"
    pub source: ColorSource,  // Native (vom Anbieter) oder Custom (vom Nutzer überschrieben)
}

pub enum ColorSource { Native, Custom }
```

`ContainerColor` wird sowohl für Kalender (via `CalendarFeature::calendar_color()`) als auch für Aufgabenlisten (als Feld `TaskList.color`) verwendet – die Source-Information (Native vom Anbieter oder Custom vom Nutzer überschrieben) ist in beiden Fällen relevant.

> **Token-Verwaltung:** Das von `authenticate()` zurückgegebene `AuthToken` wird vom Adapter intern (z.B. in einem `Arc<RwLock<AuthToken>>`-Feld) gespeichert und für alle nachfolgenden API-Aufrufe verwendet. Token-Refresh und Re-Authentifizierung sind damit ein Implementierungsdetail des jeweiligen Adapters und Teil seiner internen Logik. Aufrufer der Feature-Traits müssen das Token nicht selbst durchreichen.

Ein Adapter implementiert immer das `Adapter`-Basis-Trait plus eine beliebige Kombination der drei Feature-Traits. Beispiele:

| Adapter | Implementierte Traits | Capabilities |
|---|---|---|
| `cal-adapter-caldav` | `Adapter`, `CalendarFeature`, `TasksFeature`, `ContactsFeature` | `["calendar", "tasks", "contacts"]` |
| `cal-adapter-google` | `Adapter`, `CalendarFeature`, `TasksFeature`, `ContactsFeature` | `["calendar", "tasks", "contacts"]` |
| `cal-adapter-microsoft-graph` | `Adapter`, `CalendarFeature`, `TasksFeature`, `ContactsFeature` | `["calendar", "tasks", "contacts"]` |
| `cal-adapter-ews` | `Adapter`, `CalendarFeature`, `TasksFeature`, `ContactsFeature` | `["calendar", "tasks", "contacts"]` |
| `cal-adapter-ical` | `Adapter`, `CalendarFeature` | `["calendar"]` |
| `cal-adapter-local` | `Adapter`, `CalendarFeature`, `TasksFeature` | `["calendar", "tasks"]` |
| `cal-adapter-vikunja` | `Adapter`, `TasksFeature` | `["tasks"]` |
| `cal-adapter-todoist` | `Adapter`, `TasksFeature` | `["tasks"]` |

#### Abhängigkeiten je Adapter-Crate

> **Hinweis:** Die Videokonferenz-Adapter (`vc-adapter-*`) sind eigenständige Plugin-Crates im Workspace. Sie sind hier der Vollständigkeit halber aufgeführt; inhaltlich gehören sie zu Abschnitt 11.

| Crate | Externe Abhängigkeiten |
|---|---|
| `cal-adapter-google` | `google-apis-rs`, `oauth2` |
| `cal-adapter-microsoft-graph` | `graph-rs-sdk`, `oauth2` |
| `cal-adapter-ews` | `reqwest`, `quick-xml` (keine externen SDKs) |
| `cal-adapter-caldav` | `reqwest`, `quick-xml`, `icalendar` (CalDAV + CardDAV) |
| `cal-adapter-ical` | `icalendar` |
| `cal-adapter-local` | Keine (nur SQLite via `rusqlite`) |
| `cal-adapter-vikunja` | `reqwest`, `oauth2` |
| `cal-adapter-todoist` | `reqwest`, `oauth2` |
| `vc-adapter-zoom` | `reqwest`, `oauth2` |
| `vc-adapter-teams` | `graph-rs-sdk`, `oauth2` |
| `vc-adapter-meet` | `google-apis-rs`, `oauth2` |
| `vc-adapter-webex` | `reqwest`, `oauth2` |

### 6.2 Geplante Adapter (v1.0)

| Adapter | Protokoll | Auth |
|---|---|---|
| **Google Calendar** | Google Calendar API v3 | OAuth2 |
| **Microsoft Outlook / Exchange Online** | Microsoft Graph API | OAuth2 (MSAL) |
| **Exchange on-premise (EWS)** | Exchange Web Services (SOAP) | Basic / NTLM / OAuth2 |
| **Apple iCloud** | CalDAV | Apple-spezifisches Auth |
| **Generisch iCal (.ics)** | Datei / URL | Keine / Basic |
| **CalDAV / CardDAV (generisch)** | CalDAV RFC 4791 + CardDAV RFC 6352 | Basic / OAuth2 |
| **Vikunja** (nur Aufgaben) | Vikunja REST API | API-Token / OAuth2 |
| **Todoist** (nur Aufgaben) | Todoist REST API v2 | OAuth2 / API-Token |
| **Lokal** | Kein externes Protokoll | Keine |

> **Lokaler Kalender:** Termine und Aufgaben werden ausschließlich in der lokalen SQLite-Datenbank gespeichert und haben kein API-Gegenstück. Geräteübergreifende Synchronisation erfolgt vollständig über das Event Log (Abschnitt 19) – `event.*`-, `task.*`- und `task_list.*`-Ereignisse werden wie bei externen Kalendern ins Log geschrieben, jedoch nie an eine externe API weitergeleitet. Lokale Kalender sind beliebig oft anlegbar (siehe Abschnitt 6.4) und immer aufgabenfähig. Sie eignen sich für private Termine und Aufgaben, die bewusst keinem Cloud-Dienst anvertraut werden sollen.

> **Hinweis EWS:** Exchange Web Services ist ein älteres SOAP-basiertes Protokoll für on-premise Exchange-Server (2010–2019). Da kein ausgereiftes Rust-Crate existiert, wird `cal-adapter-ews` als native Eigenimplementierung auf Basis von `reqwest` + `quick-xml` entwickelt. Auf eine externe Brücke (z.B. DavMail) wird bewusst verzichtet, um die portable Einzelbinary-Anforderung zu wahren.

### 6.3 Hinweis: Bereits abgedeckte Dienste

Viele bekannte Kalenderdienste sind über die v1.0-Adapter bereits vollständig unterstützt, ohne dass ein eigener Adapter nötig ist:

| Dienst | Abgedeckt durch |
|---|---|
| **Nextcloud Calendar** | `cal-adapter-caldav` (CalDAV) |
| **Fastmail** | `cal-adapter-caldav` (CalDAV) |
| **Radicale / Baikal** | `cal-adapter-caldav` (CalDAV) |
| **Exchange Online (Microsoft 365)** | `cal-adapter-microsoft-graph` (Graph API) |
| **Zoho Calendar** | `cal-adapter-caldav` (CalDAV, sofern Zoho CalDAV-Zugang aktiviert) |

Eigene Adapter für v2.0+ sind daher nur dann sinnvoll, wenn ein Dienst proprietäre Funktionen hat, die über die v1.0-Adapter (CalDAV/Graph/REST) nicht erreichbar sind.

### 6.4 Mehrere Konten & Aggregation

- Jeder Adapter kann **beliebig oft** mit unterschiedlichen Konten oder Servern instanziiert werden – mehrere Google-Accounts, mehrere iCloud-Accounts, mehrere EWS-Quellen, mehrere CalDAV-Server, mehrere Vikunja-Instanzen, mehrere Todoist-Konten etc. sind alle gleichzeitig verbindbar
- Jede Verbindung erscheint im Filter-Panel als eigenständiger Eintrag mit eigenem Namen und eigener Farbe (konfigurierbar)
- Alle Verbindungen werden in einer aggregierten Ansicht zusammengeführt
- Pro Container (Kalender oder Aufgabenliste) innerhalb einer Verbindung: einzeln ein-/ausblendbar
- Filter-Panel (zugänglich per Tastatur): schnelles Ein-/Ausblenden per `Space`

#### Lokale Kalender

Der lokale Kalender-Adapter (`cal-adapter-local`) ist die einzige Datenquelle ohne externes Backend. Für ihn gilt:

- **Unbegrenzte Anzahl:** Der Nutzer kann beliebig viele lokale Kalender anlegen, umbenennen, einfärben und löschen
- **Aufgabenfähig:** Jeder lokale Kalender enthält automatisch eine Aufgabenliste mit derselben ID – Termine und Aufgaben können in demselben lokalen Container abgelegt werden
- **Kein automatisches Anlegen:** Beim ersten App-Start wird kein lokaler Kalender erzeugt – die App startet komplett leer, bis der Nutzer ein Konto verbindet oder einen lokalen Kalender erstellt
- **Synchronisation nur über Event Log:** Lokale Kalender und ihre Aufgabenlisten werden ausschließlich über die geräteübergreifende Synchronisation (Abschnitt 19) zwischen den eigenen Geräten geteilt, nicht an externe Anbieter weitergegeben

### 6.5 Kalender- und Aufgabenlisten-Farbgebung

Termine werden in der Farbe des zugehörigen Kalenders, Aufgaben in der Farbe der zugehörigen Aufgabenliste dargestellt. Damit erkennen Nutzer auf einen Blick, aus welcher Quelle ein Eintrag stammt.

#### Farb-Quellen je Adapter (Kalender)

| Adapter | Native Farb-API | Verhalten |
|---|---|---|
| **Google Calendar** | ✅ Ja – `calendar.backgroundColor` (Hex) via API | Farbe wird direkt übernommen; in der App überschreibbar |
| **Microsoft Graph (Outlook)** | ✅ Ja – `calendar.color` (Enum: `auto`, `lightBlue`, `lightGreen`, etc.) | Enum wird auf Hex-Wert gemappt; in der App überschreibbar |
| **Apple iCloud (CalDAV)** | ✅ Ja – `calendar-color`-Property im CalDAV-Response (`#RRGGBBAA`) | Farbe wird direkt übernommen; in der App überschreibbar |
| **Exchange EWS** | ⚠️ Eingeschränkt – keine standardisierte Farb-API | Farbe wird manuell in der App vergeben |
| **CalDAV / CardDAV (generisch)** | ⚠️ Optional – `calendar-color` per RFC nicht standardisiert, aber verbreitet (z.B. Nextcloud) | Farbe wird gelesen wenn vorhanden, sonst manuell vergeben |
| **iCal (.ics)** | ❌ Nein | Farbe wird manuell in der App vergeben |
| **Lokal** | ❌ Keine API | Farbe wird manuell in der App vergeben; über Event Log synchronisiert |

#### Farb-Quellen je Adapter (Aufgabenlisten)

| Adapter | Native Farb-API | Verhalten |
|---|---|---|
| **Google Tasks** | ❌ Nein | Farbe wird manuell in der App vergeben |
| **Microsoft To Do** | ❌ Nein | Farbe wird manuell in der App vergeben |
| **CalDAV/VTODO** | Wie der zugehörige Kalender | Aufgabenliste teilt den Container mit dem Kalender |
| **EWS Tasks** | ❌ Nein | Farbe wird manuell in der App vergeben |
| **Vikunja** | ✅ Ja – `hex_color` pro Projekt | Farbe wird direkt übernommen; in der App überschreibbar |
| **Todoist** | ✅ Ja – `color`-Feld (Enum mit definierten Farbnamen) | Enum wird auf Hex-Wert gemappt; in der App überschreibbar |
| **Lokal** | ❌ Keine API | Farbe wird manuell in der App vergeben; über Event Log synchronisiert |

#### Farbänderungen zurückschreiben

Wo die API es erlaubt, wird eine in der App geänderte Container-Farbe auch zurück zum Anbieter synchronisiert:

- **Google Calendar:** `PATCH /calendars/{calendarId}` mit `backgroundColor`
- **Outlook (Graph):** `PATCH /me/calendars/{id}` mit `color`
- **iCloud (CalDAV):** `PROPPATCH` mit `apple:calendar-color`
- **Vikunja:** `POST /projects/{id}` mit `hex_color`
- **Todoist:** `POST /projects/{id}` mit `color`

#### Container-Farbe als Farb-Label-Bindung

Eine in der App gesetzte Container-Farbe ist **kein eingefrorener Hex-Wert**, sondern eine **Bindung an ein Farb-Label** (Abschnitt 8) — dasselbe vereinheitlichte Palette-System wie bei Terminen/Aufgaben. `Calendar`/`TaskList`/`ContactList` tragen dazu ein optionales `color_label: Option<ColorLabelId>`; der gerenderte Hex wird im Frontend live aus dem Label aufgelöst (zentral im `CalendarStore`), sodass das Umfärben eines Labels **alle gebundenen Container** mitfärbt.

Zwei Speicherorte, an der Auflösungsschicht vereinheitlicht:

- **Lokale Container:** die Bindung liegt auf der Zeile (`calendars`/`task_lists`/`contact_lists.color_label_id`, Migration 0022) und **synchronisiert** mit den übrigen Container-Feldern über das Event-Log.
- **Externe Container** (Google/CalDAV/…): der Provider kennt nur Hex; die Bindung ist ein **host-lokales Override** in `container_color_overrides` (gleiche Form wie `container_name_overrides`), das der Lese-Pfad oben drauf stempelt. Die Provider-Farbe bleibt Fallback, solange der Nutzer nichts bindet.

Gesetzt wird über den Command `set_container_color_label(container_id, kind, color_label_id)` (lokal → Zeile + Sync-Event; extern → Override) bzw. den Farb-Picker im „Neu"-Dialog. Ein gelöschtes Label räumt die Bindung per FK (`ON DELETE SET NULL` / `CASCADE`) ab; das Frontend fällt ohnehin sanft auf die Native-Farbe zurück. Das ältere Zurückschreiben zum Provider (oben) bleibt für reine Hex-Quellen bestehen, ist aber für die Label-Bindung nicht nötig.

#### Barrierefreiheit & Farbe

Da Farbe niemals das **einzige** Unterscheidungsmerkmal sein darf (WCAG 1.4.1), wird jeder Termin und jede Aufgabe zusätzlich zur Farbe mit dem Container-Namen als ARIA-Label versehen:

```html
<div
  class="event"
  style="--event-color: #4285f4;"
  aria-label="Termin: Teammeeting, 10:00–11:00 Uhr, Kalender: Arbeit (Google)"
>
  Teammeeting
</div>
```

Screen Reader lesen damit den Container-Namen vor, ohne dass Farbe für die Zuordnung nötig ist. In der Agenda- und Aufgaben-Ansicht wird der Container-Name zusätzlich als sichtbarer Text-Badge angezeigt.

### 6.6 Sichere Token-Speicherung

Alle Zugangsdaten der Adapter (OAuth2-Tokens, API-Tokens, Basic-Auth-Zugangsdaten) werden über die plattformeigene Keychain gespeichert:
- Windows: Windows Credential Manager
- macOS: Keychain
- Linux: libsecret / GNOME Keyring

Verwendetes Crate: `keyring`

---

## 7. Terminerstellung & -verwaltung

### 7.1 Termintypen

| Typ | Beschreibung |
|---|---|
| **Einmaliger Termin** | Standard-Termin mit Start- und Endzeit |
| **Ganztages-Termin** | Ohne Uhrzeit, erstreckt sich über einen oder mehrere Tage |
| **Wiederkehrender Termin** | Täglich, wöchentlich, monatlich, jährlich (RRULE nach RFC 5545) |

> **Hinweis:** Aufgaben sind eine **eigenständige Entität** mit eigenem Datenmodell, eigener Sync-Logik und eigener Ansicht – vollständig in Abschnitt 9 spezifiziert. Sie sind kein Termintyp und werden nicht über das Terminformular erstellt. Erinnerungen sind kein eigener Termintyp, sondern ein Feature von Terminen und Aufgaben – siehe Abschnitt 14.

### 7.2 Terminformular-Felder

- Titel (Pflichtfeld)
- Datum & Uhrzeit (Start / Ende)
- Ganztages-Option
- Ort (Freitext + optional Karten-Link)
- Beschreibung (Rich-Text oder Markdown)
- Kalender (Auswahl aus verbundenen Konten)
- Farb-Label (optional; siehe Abschnitt 8)
- Teilnehmer (E-Mail-Adressen)
- Wiederholung (Typ + Endbedingung)
- Erinnerungen (Anzahl, Zeitpunkt und Typ – siehe Abschnitt 14 für vollständige Spezifikation)
- Sound-Override (optional, siehe Abschnitt 14.4)
- Videokonferenz-Link (automatisch generierbar, siehe Abschnitt 11)
- Anhänge (verlinkt, keine lokale Kopie)

### 7.3 Teilnehmer-Verwaltung

**Organizer-seitiger Versand (implementiert).** Teilnehmer werden als flache
Liste (`"Name <email>"` oder reine E-Mail) gepflegt. Beim Anlegen/Ändern/Löschen
eines Termins kann Aperio den Anbieter **serverseitig** Einladungen / Updates /
Absagen verschicken lassen — **ohne eigenes SMTP**. Gesteuert über den Schalter
„Teilnehmer benachrichtigen" im Termin-Dialog (Standard: an), der nur erscheint,
wenn der Zielkalender `supports_scheduling` meldet und Teilnehmer vorhanden sind.

| Anbieter | Mechanismus | „nicht benachrichtigen" |
|---|---|---|
| **Exchange / EWS** | `SendMeetingInvitations*="SendToAllAndSaveCopy"` | `SendToNone`; Teilnehmer werden trotzdem gespeichert |
| **iCloud / CalDAV** | RFC 6638 Auto-Scheduling: `ORGANIZER`+`ATTENDEE` in der .ics (erkannt via `schedule-outbox-URL`; Organizer aus `calendar-user-address-set`) | `ORGANIZER`/`ATTENDEE` weglassen — kein Versand, Termin ohne Teilnehmer gespeichert |
| **Google** | `?sendUpdates=all` | `none`; Teilnehmer werden trotzdem gespeichert |
| **Microsoft Graph** | Teilnehmer im Body ⇒ Graph mailt automatisch | Teilnehmer weglassen — Graph kann sie nicht ohne Versand speichern |

`supports_scheduling` ist statisch true für EWS/Google/Graph und für CalDAV
**laufzeit-erkannt** (nur RFC-6638-fähige Server wie iCloud). Bei iCloud/Graph gilt
„Teilnehmer im Datensatz = es wird gemailt" (keine stille Speicherung).

**Free/Busy-Abfrage (implementiert).** Im Termin-Dialog prüft „Verfügbarkeit
prüfen" — sichtbar unter demselben Gate wie der Benachrichtigen-Schalter
(scheduling-fähiger Kalender + Teilnehmer vorhanden) — die Belegung aller
Teilnehmer im aktuell eingegebenen Zeitfenster. Pro Teilnehmer wird frei/belegt
angezeigt plus eine Zusammenfassung; das Ergebnis wird über die Live-Region
angekündigt. Best-Effort: Ein Anbieter, der nicht antworten darf (fehlende
Berechtigung, kein Outbox), liefert leere Slots ⇒ „frei/unbekannt" statt Fehler.
Die Abfrage läuft über `CalendarFeature::get_free_busy(emails, range)` und den
Host-Befehl `query_free_busy`.

| Anbieter | Mechanismus |
|---|---|
| **Exchange / EWS** | SOAP `GetUserAvailability` (`RequestedView=Detailed`, ein `MailboxData` je Adresse; Antworten in Anfrage-Reihenfolge) |
| **iCloud / CalDAV** | RFC 6638: iTIP `VFREEBUSY` (`METHOD:REQUEST`) per POST an die `schedule-outbox-URL`; Belegung aus dem `schedule-response` je Empfänger |
| **Google** | `POST /freeBusy` (`items[].id`); Belegung aus `calendars[email].busy[]` |
| **Microsoft Graph** | `POST /me/calendar/getSchedule` (`scheduleItems`, Status ≠ `free`) |

**RSVP (implementiert).** Beim Öffnen eines bestehenden Meetings zeigt der
Termin-Dialog eine RSVP-Leiste, sobald der Termin Antwortdaten trägt
(externe Anbieter): Ist der verbundene Account ein **Teilnehmer** (nicht der
Organisator), erscheinen **Zusagen / Vorläufig / Absagen** mit der aktuellen
Antwort hervorgehoben; ist er der **Organisator**, erscheinen schreibgeschützte
Status-Chips je Teilnehmer. Das Modell trägt `Event.organizer` +
`attendee_responses[{email,name,status}]` (read-only, `AttendeeStatus` =
NeedsAction/Accepted/Declined/Tentative), beim Lesen aus jedem Anbieter
befüllt. „Wer bin ich" liefert `CalendarFeature::current_user_email()`
(CalDAV: `calendar-user-address`; Graph: `/me`; Google: primärer Kalender =
E-Mail; EWS: Login). Antworten läuft über
`CalendarFeature::respond_to_event(event_id, status, send_response)` + den
Host-Befehl `respond_to_event`.

| Anbieter | Antwort-Mechanismus |
|---|---|
| **Exchange / EWS** | `AcceptItem` / `DeclineItem` / `TentativelyAcceptItem` (CreateItem, `SendAndSaveCopy` vs. `SaveOnly`) |
| **iCloud / CalDAV** | `ATTENDEE;PARTSTAT` im `.ics` per PUT setzen; RFC-6638-Server senden den iTIP-`REPLY` automatisch (`Schedule-Reply: F` unterdrückt) |
| **Google** | Self-`responseStatus` im `attendees[]` patchen, `sendUpdates=all/none` |
| **Microsoft Graph** | `POST /me/events/{id}/accept\|decline\|tentativelyAccept` (`sendResponse`) |

### 7.4 Schnellerstellungs-Dialog

Ein barrierefreier Quick-Add-Dialog (aufrufbar per `N` von jeder Ansicht aus) ermöglicht die schnelle Terminerstellung mit minimalem Formular (Titel, Datum/Zeit, Kalender). Erweitertes Formular per "Weitere Details"-Button.

### 7.5 Termine zwischen Kalendern verschieben oder kopieren

Termine können jederzeit in einen anderen Kalender – auch kalender- und kontoübergreifend – verschoben oder kopiert werden.

**Verschieben vs. Kopieren:**
- **Verschieben:** Termin wird im Ziel-Kalender erstellt und im Quell-Kalender gelöscht. Endergebnis: ein Termin im Ziel-Kalender.
- **Kopieren:** Termin wird im Ziel-Kalender erstellt; das Original bleibt im Quell-Kalender erhalten. Endergebnis: zwei unabhängige Termine.

#### Bedienung

| Methode | Beschreibung |
|---|---|
| **Terminformular** | Kalender-Dropdown im Bearbeitungsdialog ändern (verschiebt) |
| **Kontextmenü** | Rechtsklick / Kontextmenü-Taste → "In Kalender verschieben" oder "In Kalender kopieren" → Untermenü mit allen verfügbaren Kalendern |
| **Tastatur** | Im Termin-Fokus: `Shift+M` öffnet Kalender-Auswahl-Dialog mit Verschieben/Kopieren-Wahl; `Ctrl+D` dupliziert in denselben Kalender (Schnellkopie) |
| **Drag & Drop** | Termin-Chip auf einen Kalender in der Sidebar ziehen (verschiebt in diesen Kalender) oder auf einen anderen Tag in der Wochen-/Monatsansicht (verschiebt den Tag; Uhrzeit + Dauer bleiben). Serientermine fragen nach dem Umfang („Nur diesen Termin" / „Ganze Serie"). Reine Maus-Affordanz — Tastatur-/Screenreader-Weg bleiben Dialog + `Shift+M` |

#### Technischer Ablauf

Das Verschieben zwischen Kalendern – besonders über Kontogrenzen hinweg (z.B. von Google nach Outlook) – ist technisch kein einfaches "Umbenennen", sondern ein zweistufiger Vorgang:

```
Verschieben:
1. Neuen Termin im Ziel-Kalender anlegen (CREATE)
2. Alten Termin im Quell-Kalender löschen (DELETE)

Kopieren:
1. Neuen Termin im Ziel-Kalender anlegen (CREATE)
   – das Original bleibt unangetastet
```

Beide Operationen werden atomar in der lokalen Sync-Queue eingetragen, sodass bei einem Verbindungsabbruch kein Datenverlust entsteht.

#### Sonderfälle

| Fall | Verhalten |
|---|---|
| **Innerhalb desselben Kontos (Verschieben)** | Direktes Verschieben via API (z.B. Google `calendarId`-Patch), kein Neu-Anlegen nötig |
| **Zwischen verschiedenen Konten (Verschieben)** | Zweistufig: CREATE + DELETE (siehe oben) |
| **Kopieren** | Immer CREATE im Ziel; keine native API-Optimierung |
| **Wiederkehrende Terminserie** | Nutzer wird gefragt: "Nur diesen Termin" oder "Gesamte Serie" (gilt für Verschieben und Kopieren) |
| **Termin mit Teilnehmern** | Hinweis-Dialog: Teilnehmer erhalten ggf. eine neue Einladung vom Ziel-Konto |
| **Termin mit Videokonferenz-Link** | Hinweis-Dialog: Bestehender Link bleibt erhalten, ist aber ggf. an das alte Konto gebunden |

#### Barrierefreiheit

- Kontextmenü ist per `Kontextmenü-Taste` oder `Shift+F10` erreichbar
- Alle Untermenü-Einträge sind per Pfeiltasten navigierbar
- Nach erfolgreichem Verschieben: `aria-live`-Ankündigung ("Termin 'Teammeeting' wurde in Kalender 'Privat' verschoben")
- Nach erfolgreichem Kopieren: `aria-live`-Ankündigung ("Termin 'Teammeeting' wurde in Kalender 'Privat' kopiert")

---

## 8. Farb-Labels

Farb-Labels ermöglichen farbliche Kennzeichnung einzelner Termine und Aufgaben – unabhängig von der Container-Farbe (Kalender bzw. Aufgabenliste).

### 8.1 Label-Verwaltung

Unter `Einstellungen → Farb-Labels`:

```
┌─────────────────────────────────────────────────┐
│  Farb-Labels                                    │
│                                                 │
│  ● Arbeit        #E53935   [Bearbeiten]         │
│  ● Privat        #43A047   [Bearbeiten]         │
│  ● Dringend      #FB8C00   [Bearbeiten]         │
│  ● Familie       #8E24AA   [Bearbeiten]         │
│                                                 │
│  [+ Neues Label]                                │
└─────────────────────────────────────────────────┘
```

### 8.2 Hierarchie

| Ebene | Priorität |
|---|---|
| Globale Labels (app-weit) | Basis |
| Container-Labels (Kalender / Aufgabenliste, überschreiben Global) | Mittel |
| Termin-/Aufgaben-Label | Höchste Priorität |

Ein Termin oder eine Aufgabe ohne Label erbt die Farbe seines bzw. ihres Containers (Kalender oder Aufgabenliste). Mit Label: Label-Farbe. Container-Overrides überschreiben globale Labels lokal.

**Abschnittsfarbe (nur Aufgaben):** Aufgaben-Abschnitte (Sections) tragen ein optionales `color_label` und liegen in der Kette **zwischen** Aufgabe und Aufgabenliste: Eine Aufgabe **ohne** eigene Farbe erbt die Farbe ihres Abschnitts; hat der Abschnitt keine, gilt die Aufgabenlisten-Farbe. Auflösungszeit-Kaskade (kein eingefrorener Wert): Verschiebt man eine farblose Aufgabe in einen anderen Abschnitt, übernimmt sie automatisch dessen Farbe; das Umfärben des Abschnitts-Labels färbt alle seine farblosen Aufgaben mit. Die Abschnittsfarbe ist – wie der Abschnitt selbst – ein lokal-eigenes, event-log-synchronisiertes Feld (kein Provider-Roundtrip).

Effektive Kette einer Aufgabe: `Aufgaben-Label → Abschnitts-Label → Aufgabenlisten-Farbe → neutral`.

Die Abschnittsfarbe wird an zwei Stellen gesetzt — im Abschnitt-Anlegen/Bearbeiten-Dialog **und** über ein Kontextmenü am Abschnitts-Kopf in der Aufgaben-Ansicht (Rechtsklick bzw. die „⋮"-Schaltfläche; analog zu den Sidebar-Containern), inkl. „Andere Farbe…" für Ad-hoc-Farben.

**Lokal vs. extern (Farbquelle).** Die Abschnittsfarbe ist **immer ein lokales Konzept** — kein Anbieter (Todoist/Vikunja) hat ein Section-Farbfeld. Sie wird über `set_section_color` gesetzt, das host-seitig verzweigt, exakt wie `set_container_color_label` (Container-Farben, §6.5):

- **Lokale Abschnitte:** Bindung direkt am `sections.color_label_id` (Migration 0024) + `SectionUpdated`-Event (cross-device-synchronisiert).
- **Externe Abschnitte:** lokales Override in `section_color_overrides` (Migration 0025; gespiegelt von `container_color_overrides`, nur `section_id`, kein `kind`). `get_sections` stempelt das Override beim Lesen auf `Section.color_label`, sodass die Kaskade einheitlich auflöst. Kein Event-Log (host-lokal).

Ein Abschnitt ist sein Leben lang lokal **oder** extern (das Konto seiner Liste ändert sich nie), also kollidieren die beiden Farbquellen für eine `section_id` nie.

**Zwei Capability-Achsen.** Farbe (Override) ist immer lokal und wird überall angeboten, wo Abschnitte existieren (`sections`). Das **Anlegen/Umbenennen/Löschen** von Abschnitten am Anbieter wird separat über `manageable_sections` (Manifest) gegated — lokal, Todoist und Vikunja deklarieren es; flache Anbieter nicht. Die Mutations-Commands routen nach Konto: lokal → Store + `section.*`-Event; extern → Provider-Adapter (kein Event-Log). Endpunkte: Todoist `POST/DELETE /sections[/{id}]`; Vikunja `PUT/POST/DELETE /projects/{p}/views/{v}/buckets[/{id}]` (Kanban-View wie beim Verschieben).

**Abschnitts-Zuordnung (Schreiben):** Eine Aufgabe lässt sich zwischen Abschnitten verschieben oder aus einem Abschnitt herausnehmen (`section_id → null`), soweit der Adapter es zulässt:

- **Lokal:** direkt (`section_id`-Spalte).
- **Todoist:** über die Sync-API (`item_move` — REST v2 ignoriert Abschnittswechsel im Update; nur bei tatsächlicher Änderung gefeuert, sonst keine Umsortierung).
- **Vikunja (≥0.24):** Buckets hängen an einer per-Projekt-Kanban-*View*, nicht mehr am Task. **Lesen:** `?expand=buckets` liefert die per-View-Bucket-Mitgliedschaft, die wir auf die Kanban-View des Projekts matchen (`bucket_id` am Task ist seit 0.24 leer). **Schreiben:** der dedizierte Endpunkt `POST /projects/{p}/views/{v}/buckets/{bucket}/tasks`; vorher wird der aktuelle Bucket gelesen und **nur bei echter Änderung** verschoben (kein Reordering bei unbeteiligten Edits; kann der aktuelle Bucket nicht gelesen werden, wird gar nicht verschoben). „Kein Abschnitt" → `default_bucket_id` der View (sonst linkester Bucket), da Vikunja-Kanban keinen bucket-losen Zustand kennt. Alles best-effort: ältere Server ohne View-Endpunkte überspringen den Move, ohne den Edit zu verlieren.

### 8.3 Anwendung & Barrierefreiheit

Im Formular: Label-Dropdown mit Autocomplete ("Arb" → "Arbeit"). Da Farbe allein nicht WCAG-konform ist, wird der Label-Name immer im `aria-label` angegeben:

```html
aria-label="Teammeeting, 10:00–11:00, Label: Arbeit, Kalender: Google"
```

Labels werden über das Event Log zwischen Geräten synchronisiert – mit eigenen Ereignistypen `color_label.created`, `color_label.updated` und `color_label.deleted` (siehe Abschnitt 19.2).

### 8.4 Eigene Farben (ad-hoc)

Neben der benannten Palette kann an jeder Stelle, an der eine Farbe gesetzt wird (Termin-/Aufgaben-Dialog, Sidebar-Kontextmenü `Farbe → Andere…`), über den barrierefreien Farb-Picker (`ColorComposer`: Hex-Textfeld + Farbfeld) **eine beliebige Farbe spontan komponiert** werden. Das vermeidet den Umweg über die Einstellungen.

### 8.5 Termin-Farben (pro Termin)

Ein einzelner Termin lässt sich umfärben — per Rechtsklick auf den Termin-Chip (`Farbe`-Untermenü) oder über das Label-Feld im Termin-Dialog. Wohin die Bindung geht, hängt davon ab, ob der Anbieter eine **native Pro-Termin-Farbe** speichern kann. Das entscheidet ein Capability-Flag `Calendar.supports_event_color` (`#[serde(default)]`):

- **Lokal:** immer `true` (Farbe liegt auf der Termin-Zeile, `color_label`).
- **CalDAV:** `true`, **außer iCloud**. Gesetzt im `CalendarFeature`-Impl des CalDAV-Adapters (`server_url.contains("icloud.com")`). iCloud terminiert serverseitig (RFC 6638) — ein `COLOR`-tragendes PUT auf einen Termin mit Teilnehmern würde diese **anmailen**, deshalb bleibt iCloud beim Override.
- **Google / Microsoft Graph / EWS / iCal / Geburtstage:** `false` (keine in Aperios Farbmodell gemappte Pro-Termin-Farbe).

**Single Source of Truth pro Kalender.** Farbfähig → nativ am Provider (kein Override); nicht farbfähig → host-lokales Override (wie Stage 1).

**Nicht-farbfähig (Override, Stage 1).** Für externe Termine ohne native Farbe lebt die Label-Bindung host-lokal in `event_color_overrides` (Migration 0026; `event_id` = Serien-Master-Id, kein `kind`). `apply_color_to_events` stempelt das Override beim Lesen auf `Event.color_label`. Gesetzt über `set_event_color(event_id, calendar_id, color_label_id)`, das nach Konto verzweigt: lokal **und** farbfähig-extern → No-op (die Farbe reist über `update_event`); nur nicht-farbfähig-extern schreibt das Override. Kein Provider-PUT, also nichts, das iCloud & Co. ablehnen könnten.

**Farbfähig (nativ, Stage 2).** Round-trip über RFC 7986 `COLOR`. Transportfeld `Event`/`NewEvent.color_hex: Option<String>` (`#[serde(default, skip…)]`, ein `#RRGGBB`):

- **Schreiben:** `create_event` / `update_event` lösen für einen farbfähigen externen Zielkalender `event.color_label` → `color_hex` auf (`LocalAdapter::resolve_label_to_hex`), bevor der Adapter gerufen wird; `apply_common` schreibt dann `COLOR:<hex>`. Zur Sicherheit löscht der CalDAV-Adapter `color_hex` für iCloud nochmals am Schreibrand. Ein farbfähiges `update_event` räumt zusätzlich ein evtl. veraltetes Stage-1-Override desselben Termins ab (sonst würde es die native Farbe beim Lesen verdecken).
- **Lesen:** `map_event` parst `COLOR` → `color_hex` (akzeptiert `#RRGGBB` und bekannte CSS3-Namen, sonst `None`). `get_events` mappt `color_hex` → `color_label` (`match_hex_to_label`, nur gegen **bestehende** Labels, bevorzugt benannte) — **vor** dem Override-Stempel, damit ein Override (Override wird zuletzt angewandt) für den seltenen Fall „Termin trägt beides" gewinnt.

Capability-Lookup im Host: aus dem gecachten Kalender-Listing (`cache.read_calendars`); unbekannt → `false` (sichere Voreinstellung: Override). Das Frontend routet identisch (`account_id === 'local' || calendar.supports_event_color`).

**Ungemappte Native-Farben (read-only).** Trägt ein Termin ein `color_hex`, das der Host **keinem** bestehenden Label zuordnen kann (kein Ad-hoc-Label beim Lesen, s. o.), bleibt der Hex am Event und wird im Frontend **direkt** gerendert — die Auflösung in `resolveEventColor` ist `color_label` (benannt) → `color_hex` (roh, namenlos) → Kalenderfarbe. Das betrifft v. a. **abonnierte iCal-Feeds** (deren Adapter sich `cal_adapter_caldav::mapping` teilt und so `COLOR` mitliest) sowie **fremde** Farben auf farbfähigen CalDAV-Kalendern (ein anderer Client hat den Termin gefärbt). Reine Anzeige: ein Feed ist nicht schreibbar; eine fremde CalDAV-Farbe wird beim nächsten Aperio-Edit durch das aufgelöste Label ersetzt. Bewusst **kein** Ad-hoc-Label beim Lesen (sonst DB-Writes + `color_label.created`-Events bei jedem Refresh, geflutete Palette).

Eine Custom-Farbe wird als `ColorLabel` realisiert — dadurch greift die gesamte bestehende Mechanik (Binding über `color_label`, Auflösung im `CalendarStore`, Sync via `color_label.*`, Container-Override-Tabelle) unverändert. `ColorLabel` trägt dazu ein Flag `ad_hoc: bool`:

- **`ad_hoc = false`** — normales, benanntes Label (sichtbar in Palette + Pickern).
- **`ad_hoc = true`** — verstecktes Einmalfarben-Label (`name == hex`). **Nach Hex dedupliziert** (`get_or_create_ad_hoc_color_label`: gleiche Farbe ⇒ gleiches Label, nur bei Neuanlage ein `color_label.created`-Ereignis). Aus der Label-Verwaltung und den Auswahl-Dropdowns **ausgeblendet**, wird aber zur Auflösung normal geladen.

Im Picker lässt sich eine komponierte Farbe optional benennen und damit als reguläres (sichtbares) Label in die Palette übernehmen. Migration `0023` ergänzt die Spalte `ad_hoc`; das Sync-Feld ist `#[serde(default)]`, sodass ältere Peers es ignorieren (Farbe erscheint dort als normales Label).

---

## 9. Aufgaben-Management

Aufgaben sind ein Kern-Feature der App, gleichwertig mit Kalendern. Aufgabenlisten sind eigenständige Container mit eigener Synchronisations-Logik und eigener Ansicht; sie können – je nach Adapter – mit oder ohne zugehörigen Kalender existieren.

### 9.1 Datenmodell: Aufgaben und Aufgabenlisten

#### Aufgabenliste als eigenständiger Container

Aufgaben gehören nicht zu Kalendern, sondern zu **Aufgabenlisten**. Eine Aufgabenliste ist ein eigenständiger Container mit eigener ID, eigenem Namen und einer Quelle (Adapter). Das hat einen klaren Vorteil: Adapter, die nur Aufgaben anbieten (z.B. Todoist, ein reiner Vikunja-Server), passen ohne konzeptionelle Verrenkung ins Modell.

```rust
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    pub source: AdapterSource,

    /// Standard-Sound für Erinnerungen aller Aufgaben dieser Liste.
    /// Überschreibt den globalen Standard, kann pro Aufgabe weiter überschrieben werden.
    /// Siehe Abschnitt 14.4 für die vollständige Vererbungshierarchie.
    pub default_sound: Option<SoundConfig>,

    /// Wenn die Liste in einem aufgabenfähigen Kalender lebt (CalDAV/VTODO,
    /// lokaler Kalender), zeigt dieser Hinweis auf den Kalender. Bei
    /// eigenständigen Aufgabenlisten (Google Tasks, Microsoft To Do, Vikunja,
    /// Todoist) ist das Feld `None`.
    pub embedded_in_calendar: Option<String>,
}
```

Das Verhältnis Kalender ↔ Aufgabenliste ist je nach Anbieter unterschiedlich:

| Anbieter | Kalender und Aufgabenliste sind … |
|---|---|
| **Google** | Getrennte Entitäten – jede mit eigener ID |
| **Microsoft Graph** | Getrennte Entitäten (Outlook-Kalender vs. To-Do-Listen) |
| **CalDAV / iCloud** | Eine VTODO-Aufgabenliste lebt **in einem Kalender** (gleicher Container, geteilte URL) |
| **EWS** | Getrennte Entitäten |
| **Vikunja** | Kein Kalender vorhanden – nur Aufgabenlisten (= Vikunja-Projekte) |
| **Todoist** | Kein Kalender vorhanden – nur Aufgabenlisten (= Todoist-Projekte) |
| **Lokal** | Jeder lokale Kalender ist automatisch aufgabenfähig – Kalender und zugehörige Aufgabenliste teilen sich die ID |

#### Aufgaben-Datenmodell

```rust
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,

    // Zeitliche Zuordnung
    pub scheduled_date: Option<NaiveDate>,    // Manuell zugeordneter Tag
    pub deadline_type: Option<DeadlineType>,
    pub deadline_date: Option<NaiveDate>,
    pub deadline_time: Option<NaiveTime>,     // Nur bei DeadlineType::On

    // Wiederholung
    pub recurrence: Option<TaskRecurrence>,

    // Struktur
    pub parent_id: Option<String>,            // Unteraufgaben
    pub list_id: String,                      // Referenziert eine TaskList
    pub color_label: Option<ColorLabelId>,    // Abschnitt 8

    // Erinnerungen (analog Abschnitt 14)
    pub reminders: Vec<Reminder>,

    /// Sound-Override auf Aufgaben-Ebene. Überschreibt den Default der
    /// zugehörigen Aufgabenliste (siehe Abschnitt 14.4).
    pub sound: Option<SoundConfig>,

    // Metadaten
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

pub enum DeadlineType {
    /// Konkreter Tag + opt. Uhrzeit: erscheint nur am Deadline-Tag
    On,
    /// "Muss bis zum": erscheint als Fälligkeitsmarker am Deadline-Tag
    /// (Auto-Einplanung verschiebt überfällige Aufgaben dorthin)
    By,
}

pub enum TaskStatus { Open, InProgress, Completed, Cancelled }
pub enum TaskPriority { Low, Medium, High }
```

### 9.2 Deadline-Typen & Verhalten

| Zustand | Kalenderansicht |
|---|---|
| **Kein Datum (Backlog)** | Nur im Backlog sichtbar |
| **Geplanter Tag** (`scheduled_date`) | Erscheint am zugeordneten Tag |
| **Konkrete Deadline** (`On`) | Erscheint am Deadline-Tag; ohne geplanten Tag zusätzlich im Backlog |
| **"Muss bis zum"** (`By`) | Fälligkeitsmarker am Deadline-Tag; ohne geplanten Tag zusätzlich im Backlog |

**Automatische Einplanung:** Aufgaben mit `DeadlineType::By`, die bis zum Deadline-Tag nicht erledigt wurden, werden automatisch auf den Deadline-Tag gesetzt – letzter möglicher Tag.

**Konfigurierbar:** In welchen Ansichten `By`-Aufgaben erscheinen (Wochen-, Tages-, Monatsansicht etc.) ist unter `Einstellungen → Aufgaben → Ansichten` pro Ansichtstyp ein-/ausschaltbar.

**Auto-Datum beim Start:** Wird eine Aufgabe auf `in_progress` („in Bearbeitung") gesetzt und hat noch keinen geplanten Tag (`scheduled_date`), wird sie automatisch auf **heute** eingeplant – die Arbeit hat begonnen, also gehört die Aufgabe in den heutigen Tag (und der Carry-over-/Verpasste-Aufgaben-Ablauf findet sie später wieder). Ein in derselben Bearbeitung explizit gesetztes Datum hat Vorrang. Pro Liste unter `Einstellungen → Aufgaben` ein-/ausschaltbar (Standard: an). Die Regel gilt einheitlich für den Status-Wechsel über das Kontextmenü **und** über den Aufgaben-Dialog sowie für Eltern-Aufgaben, die durch die Status-Kopplung (§9.1) auf `in_progress` abgeleitet werden. Implementiert als `autoDateOnStart` in `taskCascade.ts`.

### 9.3 Backlog

Aufgaben ohne **geplanten Tag** (`scheduled_date`) sammeln sich im Backlog – der ungeordneten Warteschlange noch nicht auf einen Arbeitstag eingeplanter Aufgaben. Das schließt Aufgaben mit Deadline, aber ohne festen Bearbeitungstag mit ein: Sie erscheinen als Fälligkeitsmarker am Deadline-Tag **und** im Backlog, damit man sie von dort auf einen konkreten Arbeitstag ziehen kann. (Maßgeblich ist `scheduled_date`, nicht `deadline_date`.)

**Sichtbarkeit:**
- **Sidebar im Kalender** (ein-/ausblendbar, Standard: eingeblendet in Wochen- und Tagesansicht)
- **Separate Aufgaben-Ansicht** (Abschnitt 9.8)

**Einplanen aus dem Backlog:**

| Methode | Beschreibung |
|---|---|
| **Drag & Drop** | Aufgabe aus Backlog-Sidebar auf Tag in Kalenderansicht ziehen |
| **Kontextmenü** | "Für Tag einplanen" → Datumsauswahl |
| **Tastatur** | Aufgabe fokussieren → `Shift+D` → Datumsauswahl-Dialog |

Auf einen Tag eingeplante Aufgaben (`scheduled_date`) verschwinden aus dem Backlog. Wird der geplante Tag entfernt, wandern sie zurück – eine reine Deadline allein hält eine Aufgabe nicht aus dem Backlog fern.

### 9.4 Wochenplanung

Die Wochenansicht bietet eine dedizierte Planungsebene: Backlog als Sidebar, Aufgaben per Drag & Drop auf Wochentage verteilen. Eine Aufgabe mit Deadline erscheint als **Fälligkeitsmarker** („fällig bis …") an ihrem Deadline-Tag im Tagesraster – als Punkt, **nicht** als durchgehender Balken über alle Tage bis dahin (das wuchs bei weit entfernten Deadlines unbegrenzt und überfrachtete die Planung). Eine an einem anderen Tag eingeplante Aufgabe erscheint zusätzlich als normaler Chip an ihrem Plan-Tag; fallen Plan- und Deadline-Tag zusammen, bleibt es ein Chip. Die Tages-Ansicht folgt demselben Punkt-Modell.

### 9.5 Verpasste Aufgaben & Rückfrage

Aufgaben, deren Deadline verstrichen ist und die noch nicht erledigt wurden, lösen zum konfigurierbaren Zeitpunkt eine Rückfrage aus (Standard: nächster Morgen, z.B. 08:00 Uhr – konfigurierbar auf Morgen / Abend / App-Start):

```
┌─────────────────────────────────────────────────────┐
│  Nicht erledigte Aufgaben                           │
│                                                     │
│  Deadline war gestern:                              │
│                                                     │
│  • Steuererklärung einreichen                       │
│    [✓ Erledigt]  [↩ Zurück in Backlog]              │
│                                                     │
│  • Arzttermin absagen                               │
│    [✓ Erledigt]  [↩ Zurück in Backlog]              │
│                                                     │
│  [Alle erledigt]          [Später erinnern]         │
└─────────────────────────────────────────────────────┘
```

"Zurück in Backlog" entfernt das Datum und setzt Status auf `Open`.

### 9.6 Wiederkehrende Aufgaben (Vorlagen)

Wiederkehrende Aufgaben funktionieren als Vorlagen: Nach Abschluss einer Instanz wird automatisch die nächste erzeugt – nicht im Voraus, immer nachgelagert.

```rust
pub struct TaskRecurrence {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,                          // z.B. 3 = "alle 3 Wochen"
    pub day_of_week: Option<Vec<Weekday>>,      // z.B. [Sa, So]
    pub day_of_month: Option<u8>,
    pub end: Option<RecurrenceEnd>,
}
pub enum RecurrenceFrequency { Daily, Weekly, Monthly, Yearly }
```

Beispiel "Bettwäsche wechseln – jedes dritte Wochenende":
```
frequency: Weekly, interval: 3, day_of_week: [Saturday]
```

### 9.7 Kollaborative Aufgaben: Adapter

#### CalDAV/VTODO (Standard, im `cal-adapter-caldav` enthalten)

Aufgaben als VTODO über CalDAV – kompatibel mit Nextcloud Tasks, Radicale, Baikal. Capability: `tasks`.

#### Vikunja (`cal-adapter-vikunja`, nativ gebundelt)

Vikunja ist eine selbst-hostbare Open-Source-Aufgabenverwaltung (AGPLv3) mit REST-API, Multi-User-Support, CalDAV-Sync, wiederkehrenden Aufgaben, Unteraufgaben und Farb-Labels. Capability: `tasks`.

| Feature | CalDAV/VTODO | Vikunja |
|---|---|---|
| Standardprotokoll | ✅ RFC 5545 | ❌ REST |
| Kollaboration | ⚠️ Serverabhängig | ✅ Nativ |
| Unteraufgaben | ⚠️ Via RELATED-TO | ✅ Nativ |
| Selbst-hostbar | ✅ | ✅ |
| Lizenz | – | AGPLv3 |

> **Hinweis Lizenz:** Da Vikunja AGPLv3 ist, wird die Anbindung als reiner REST-Client implementiert – kein Vikunja-Code wird verlinkt oder mitausgeliefert. Die HTTP-Aufrufe bleiben in einem isolierten Crate (`cal-adapter-vikunja`), das nur Aperios eigene Lizenz benötigt. Diese Trennung erlaubt zudem, das Crate problemlos auszukoppeln, falls die enge Verzahnung mit dem Aperio-Release-Zyklus später unerwünscht wird.

#### Todoist (`cal-adapter-todoist`, nativ gebundelt)

Todoist ist ein kommerzieller, weit verbreiteter Aufgabendienst mit REST-API. Capability: `tasks`. Authentifizierung per OAuth2 oder API-Token.

#### API-Integration bestehender Adapter

| Adapter | Aufgaben-API | Unteraufgaben | Sync |
|---|---|---|---|
| **Google** | Google Tasks API | ❌ | Bidirektional |
| **Microsoft Graph** | Microsoft To Do API | ✅ (`checklistItems`) | Bidirektional |
| **CalDAV / iCloud** | VTODO RFC 5545 | ⚠️ RELATED-TO | Bidirektional |
| **EWS** | EWS Task-Items | ❌ | Bidirektional |
| **iCal (.ics)** | VTODO lesend | – | Nur lesend |
| **Vikunja** | Vikunja REST API | ✅ Nativ | Bidirektional |
| **Todoist** | Todoist REST API v2 | ✅ (`parent_id`) | Bidirektional |
| **Lokal** | Keine externe API | ✅ Via `parent_id` | – (Event Log) |

#### Datums-Felder-Mapping je Adapter

Aperio hält zwei unabhängige Datums-Slots pro Aufgabe: `scheduled_date` (+ optional `scheduled_time`) und `deadline_date` (+ optional `deadline_time`). Beide sind unabhängig (de-)setzbar. Wie die je nach Adapter auf die externe API abgebildet werden, ist nicht einheitlich — einige Dienste haben beide Felder nativ, andere nur eins.

| Adapter | Externe Felder | Mapping |
|---|---|---|
| **CalDAV / VTODO** | `DTSTART`, `DUE` | `DTSTART` ↔ `scheduled_*`, `DUE` ↔ `deadline_*`. Beide Richtungen mit optionalem UTC-DATE-TIME. Round-trip ist verlustfrei. |
| **Microsoft Graph** | `startDateTime`, `dueDateTime` | `startDateTime` ↔ `scheduled_*`, `dueDateTime` ↔ `deadline_*`. Round-trip ist verlustfrei. |
| **EWS Tasks** | `StartDate`, `DueDate` | Analog Microsoft Graph: `StartDate` ↔ `scheduled_*`, `DueDate` ↔ `deadline_*`. |
| **Google Tasks** | nur `due` | **One-Way-Drift möglich.** Schreiben: zuerst `scheduled_date` → `due`; ist nur `deadline_date` gesetzt, geht das stattdessen raus. Lesen: Google's `due` landet immer in `scheduled_date`. Folge: eine Aperio-Aufgabe mit `deadline_date` (ohne `scheduled_date`) landet in Google als `due` und kommt beim nächsten Sync als `scheduled_date` zurück — die "by"-Semantik geht beim Round-trip verloren. Akzeptiert als Trade-off, weil Google Tasks keine zwei Slots kennt. |
| **iCal (.ics)** | `DTSTART`, `DUE` (lesend) | Wie CalDAV, aber read-only. |
| **Vikunja / Todoist** | jeweiliges Schema | TBD beim Implementierungs-Commit; vermutlich analog Google (ein einziges Datums-Feld → `scheduled_date`). |
| **Lokal** | direkt aus / in DB | Spalten 1:1; keine Übersetzung nötig. |

#### Aufgaben-Zuweisung an andere Nutzer (Assignees)

Über die reine „für mich einplanen"-Sicht hinaus sollen Aufgaben **anderen Nutzern derselben Provider-Instanz zugewiesen** werden können (zuweisen, für andere einplanen). Was die Anbieter dabei können, ist sehr unterschiedlich:

| Provider | Zuweisen an andere | Feld / Endpoint | Pool (zuweisbar) | Mehrere? |
|---|---|---|---|---|
| **Vikunja** | ✅ Ja | `…/tasks/{id}/assignees` (PUT / `bulk` / DELETE / GET), Schlüssel `user_id` | `GET /projects/{id}/projectusers?s=` | Ja |
| **Todoist** | ✅ Begrenzt | `assignee_id` (nur geteilte Projekte) | `GET /projects/{id}/collaborators` | **Nein, nur 1** |
| **MS To Do** | ❌ Nein | — (Graph-`todoTask` hat kein Zuweisungsfeld) | — | — |
| **MS Planner** | ✅ Ja (eigener Adapter) | `assignments` (AAD-GUID), Consent `Group.ReadWrite.All` | `GET /groups/{id}/members` | Ja |

**Wichtig:** Microsoft **To Do** kann Zuweisung über die API nicht — das kollaborative Microsoft-Pendant ist **Planner** (eigener, schwergewichtiger Adapter; hier zunächst _out of scope_).

**Datenmodell.** Ein einheitlicher, mehrwertiger Typ:

- `TaskUser { id, name, email: Option }` — `id` ist die provider-native User-ID, `name` der Anzeigename. Wird für `assignees`, den Mitglieder-Pool **und** die Eigen-Identität verwendet.
- `assignees: Vec<TaskUser>` an `Task` **und** `NewTask`, mit `#[serde(default)]` → reist über die bestehende serde-Grenze der Plugin-FFI mit, ohne vtable-Änderung fürs Lesen/Schreiben.
- **Adapter clampt:** Mehrwert-Provider (Vikunja) setzen die ganze Liste; Einzelwert-Provider (Todoist) nehmen `assignees[0]` und **warnen** bei >1.

**Mitglieder-Pool.** Zuweisbar sind die Mitglieder der jeweiligen **Liste / des Projekts** (Vikunja `projectusers`, Todoist `collaborators`) — _nicht_ die globalen Kontakte. Dafür eine neue, optionale `TasksFeature`-Methode `list_task_list_members(list_id) -> Vec<TaskUser>` (default leer). Sie speist den Personen-Picker, auf Anfrage geladen und pro Liste gecacht.

**Eigen-Identität („ich").** Um „mir / anderen / niemandem zugewiesen" zu unterscheiden, liefert jeder Adapter `current_user() -> Option<TaskUser>` (Vikunja `GET /user`); die Identität wird beim Verbinden einmal geholt und am Account abgelegt.

**Capability.** Pro Adapter im Plugin-Manifest deklariert (`task_assignment: none | single | multiple`); das UI blendet den Picker aus, wo nicht unterstützt (lokal, MS To Do), und schaltet bei Todoist auf Einfach-Auswahl.

**UI.** Im Aufgaben-Dialog ein „Zugewiesen an"-Picker (Suche über die Listen-Mitglieder, Multi-Chips, capability-gated). „Für andere einplanen" ist damit Datum **+** Assignee im selben Dialog — beides sind Aufgaben-Felder. In der Aufgaben-Ansicht ein Assignee-Badge je Zeile plus Filter „mir / anderen / niemandem".

**Phasen.** (0) Fundament: Modell + Trait-Methoden + FFI-vtable für `list_task_list_members`/`current_user` + Account-Identität. ✅ (1) Vikunja end-to-end. ✅ (2) UI (Picker + Badge). ✅ (3) Todoist (single, geteilte Projekte). ✅ — `assignee_id` lesen/schreiben (REST v2), `assignees[0]` mit Warnung bei >1, Update sendet `assignee_id: null` zum Entfernen; `collaborators` als Mitglieder-Pool und zum Auflösen des Anzeigenamens (nur geholt, wenn überhaupt zugewiesen). `current_user` bleibt `None` (Todoist REST hat kein `/user` — „mir zugewiesen"-Hervorhebung wäre ein Sync-API-Nachzug). _Out of scope:_ MS To Do/Planner; Cross-Account-Zuweisung (Task in Konto A an Nutzer aus Konto B).

#### Mitglieder-/Freigabe-Verwaltung einer Liste

Ergänzend zur Zuweisung: Mitglieder einer Liste / eines Projekts direkt aus Aperio **auflisten, hinzufügen/einladen, entfernen** und (wo unterstützt) ihre **Rechte** setzen. Die Anbieter unterscheiden sich grundlegend:

| Operation | Vikunja | Todoist |
|---|---|---|
| Auflisten | `GET /projects/{id}/users` (direkte Shares **mit Recht**) — anders als `projectusers` (effektiver Pool ohne editierbare Rechte) | `GET /projects/{id}/collaborators` (REST, read-only) |
| Hinzufügen | `PUT /projects/{id}/users` `{user_id=Username, right}` — **sofort** | Sync-Command `share_project {email}` — **E-Mail-Einladung, Annahme nötig** (Status `invited`) |
| Entfernen | `DELETE /projects/{id}/users/{userID}` | Sync-Command `delete_collaborator {email}` |
| Recht setzen | `POST /projects/{id}/users/{userID}` `{right}` — **0/1/2 = Lesen/Schreiben/Admin** | ❌ keine per-Projekt-Rolle |
| Person finden | `GET /users?s=` (existierendes Konto; add per **Username**) | keine Suche — Einladung per **roher E-Mail** |
| Teams | ✅ `/projects/{id}/teams` (gleiche Verben) | — |

**Kernunterschied:** Vikunja = sofortige Freigabe an **bestehende Nutzer** mit **Rechtestufen** (+ Teams); Todoist = **E-Mail-Einladung mit Annahme-Flow**, ohne Rollen. (Vikunja-Eigenheit: **PUT = anlegen, POST = ändern**.) MS Planner/To Do bleiben _out of scope_.

**Datenmodell.**
- `MemberRight`: `Read | Write | Admin` (Vikunja 0/1/2).
- `TaskListShare { user: TaskUser, right: Option<MemberRight>, pending: bool }` — `right = None` bei Anbietern ohne Rollen (Todoist); `pending = true` für noch nicht angenommene Einladungen (Todoist).
- Adapter-Capabilities (Manifest / Capability): `manageable`, `roles` (Rechte editierbar?), `add_by: user_search | email`, `invitations` (Pending-Zustand?).

**Trait-Methoden** (`TasksFeature`, je FFI-vtable-Slot wie bei der Zuweisung):
- `list_task_list_shares(list_id) -> Vec<TaskListShare>` (default leer)
- `search_users(query) -> Vec<TaskUser>` (default leer) — Personensuche zum Hinzufügen (Vikunja)
- `add_task_list_member(list_id, member_ref, right) -> ()` (default Unsupported) — `member_ref` = Username (Vikunja) bzw. E-Mail (Todoist)
- `remove_task_list_member(list_id, member_ref) -> ()` (default Unsupported)
- `set_task_list_member_right(list_id, member_ref, right) -> ()` (default Unsupported)

**UI.** Ein „Mitglieder / Freigabe"-Dialog pro Liste (aus dem Sidebar-Kontextmenü), capability-gated:
- Vikunja: Mitgliederliste mit Rechte-Dropdown + Entfernen; Hinzufügen via **User-Suche**.
- Todoist: Kollaboratoren + **ausstehende Einladungen**; Hinzufügen via **E-Mail-Feld**; keine Rollen-Dropdowns.

Sichtbar nur, wenn der Adapter der Liste Mitgliederverwaltung kann (lokale Listen / nicht verwaltbare Backends blenden ihn aus).

**Capability.** Pro Adapter im Manifest (`tasks.manageable` + `tasks.member_add_by: search | email`). `manageable` gated das Sidebar-„Mitglieder"-Menü (Vikunja + Todoist = true; lokal/Google/MS To Do = false), `member_add_by` schaltet den Dialog zwischen User-Suche (Vikunja) und E-Mail-Einladung (Todoist). Rollen + Pending sind datengetrieben (`right = null` ⇒ kein Dropdown, `pending` ⇒ Badge).

**Phasen.** ✅ (1) Vikunja end-to-end: Modell + Trait/FFI + Adapter (Shares lesen, add/remove/set_right, User-Suche) + Mitglieder-Dialog mit Rechten. (1b) Teams. ✅ (2) Todoist: `collaborators` + `collaborator_states`, `share_project`/`delete_collaborator` (Sync-API v9), Pending-Zustand; UI auf E-Mail-Einladung ohne Rollen. Mitgliedschaft schlüsselt auf der **E-Mail** (was `delete_collaborator` braucht), `right = None`. _Out of scope:_ MS; instanzweite Nutzer-/Team-Administration (nur projekt-/listenbezogene Freigabe).

### 9.8 Separate Aufgaben-Ansicht (`Ctrl+6`)

- Gruppierung nach: Fälligkeitsdatum / Priorität / Status / Aufgabenliste
- Filterung nach Status, Priorität, Aufgabenliste, Zeitraum
- Backlog als eigener Bereich oben
- Unteraufgaben eingerückt, ein-/ausklappbar (`aria-expanded`)
- `Space` auf fokussierter Aufgabe: als erledigt markieren

### 9.9 Aufgaben erstellen & bearbeiten

Schnellerstellung per `Ctrl+Shift+N`. Formular:

- Titel (Pflichtfeld)
- Status & Priorität
- Deadline-Typ: Kein Datum / Geplanter Tag / Konkrete Deadline / "Muss bis zum"
- Datum + optionale Uhrzeit
- Beschreibung
- Aufgabenliste (Pflichtfeld – Auswahl aus verbundenen Aufgabenlisten)
- Farb-Label
- Unteraufgaben (inline)
- Wiederholung
- Erinnerungen (Abschnitt 14)
- Sound-Override (optional, siehe Abschnitt 14.4)

### 9.10 Aufgaben zwischen Aufgabenlisten verschieben oder kopieren

Aufgaben können jederzeit in eine andere Aufgabenliste – auch listen- und kontoübergreifend – verschoben oder kopiert werden. Das ist nützlich, um eine Aufgabe z.B. von einer privaten lokalen Liste in eine geteilte Vikunja-Projektliste zu übertragen.

**Verschieben vs. Kopieren:**
- **Verschieben:** Aufgabe wird in der Ziel-Liste erstellt und in der Quell-Liste gelöscht. Endergebnis: eine Aufgabe in der Ziel-Liste.
- **Kopieren:** Aufgabe wird in der Ziel-Liste erstellt; das Original bleibt in der Quell-Liste erhalten. Endergebnis: zwei unabhängige Aufgaben.

#### Bedienung

| Methode | Beschreibung |
|---|---|
| **Aufgabenformular** | Aufgabenlisten-Dropdown im Bearbeitungsdialog ändern (verschiebt) |
| **Kontextmenü** | Rechtsklick / Kontextmenü-Taste → "In Liste verschieben" oder "In Liste kopieren" → Untermenü mit allen verfügbaren Aufgabenlisten |
| **Tastatur** | Im Aufgaben-Fokus: `Shift+M` öffnet Listen-Auswahl-Dialog mit Verschieben/Kopieren-Wahl; `Ctrl+D` dupliziert in dieselbe Liste (Schnellkopie) |

#### Technischer Ablauf

Analog zum Verschieben/Kopieren von Terminen (Abschnitt 7.5): über Kontogrenzen hinweg (z.B. von Google Tasks nach Vikunja) ist beides ein CREATE im Ziel, beim Verschieben gefolgt von einem DELETE im Quell-Container.

```
Verschieben:
1. Neue Aufgabe in der Ziel-Liste anlegen (CREATE)
2. Alte Aufgabe in der Quell-Liste löschen (DELETE)

Kopieren:
1. Neue Aufgabe in der Ziel-Liste anlegen (CREATE)
   – das Original bleibt unangetastet
```

Beide Operationen werden atomar in der lokalen Sync-Queue eingetragen.

#### Sonderfälle

| Fall | Verhalten |
|---|---|
| **Innerhalb desselben Kontos (Verschieben)** | Direktes Verschieben via API, wo unterstützt (z.B. Google Tasks `move`-Endpunkt) |
| **Zwischen verschiedenen Konten (Verschieben)** | Zweistufig: CREATE + DELETE |
| **Kopieren** | Immer CREATE im Ziel; keine native API-Optimierung |
| **Aufgabe mit Unteraufgaben** | Nutzer wird gefragt: "Nur diese Aufgabe" oder "Mit allen Unteraufgaben" (gilt für Verschieben und Kopieren) |
| **Wiederkehrende Aufgabe (Vorlage)** | Nutzer wird gefragt: "Nur die aktuelle Instanz" oder "Mit Wiederholungsregel" |
| **Aufgabe mit Erinnerungen** | Erinnerungen werden mitkopiert/mitverschoben, sofern der Ziel-Adapter sie unterstützt; sonst Hinweis-Dialog |

#### Barrierefreiheit

- Kontextmenü ist per `Kontextmenü-Taste` oder `Shift+F10` erreichbar
- Alle Untermenü-Einträge sind per Pfeiltasten navigierbar
- Nach erfolgreichem Verschieben: `aria-live`-Ankündigung ("Aufgabe 'Bericht schreiben' wurde in Liste 'Projekte' verschoben")
- Nach erfolgreichem Kopieren: `aria-live`-Ankündigung ("Aufgabe 'Bericht schreiben' wurde in Liste 'Projekte' kopiert")

### 9.11 Tastaturkürzel & Barrierefreiheit

Die vollständige Kürzel-Referenz befindet sich in Abschnitt 15.7. Aufgaben-spezifische Kürzel im Überblick:

| Kürzel | Aktion |
|---|---|
| `Ctrl+6` | Aufgaben-Ansicht öffnen |
| `Alt+N` | Aufgabe schnell anlegen (Quick-Add) |
| `Alt+Shift+N` | Neue Aufgabe (vollständiges Formular) |
| `Space` | Fokussierte Aufgabe erledigen / rückgängig |
| `Shift+D` | Datum für fokussierte Aufgabe setzen |
| `Shift+M` | Aufgabe in andere Liste verschieben/kopieren |
| `Ctrl+D` | Aufgabe in dieselbe Liste duplizieren |

- Checkboxen: `role="checkbox"` mit `aria-checked`
- Unteraufgaben-Listen: `role="group"` mit `aria-label` der Eltern-Aufgabe
- Status-Änderung: `aria-live="polite"`-Ankündigung

---

### 9.12 Bedarfs-Wiederholung & Aperio-Extras

Manche wiederkehrenden Aufgaben haben **kein festes Intervall**, sondern kommen **bei Bedarf** zurück — und sollen im **Backlog** als Erinnerung auftauchen, statt auf einen Kalendertag gelegt zu werden. Beispiele:

- **Geschirrspüler einräumen** — sobald genug Geschirr da ist; nach Erledigung sofort wieder im Backlog.
- **Schuhe Sommer↔Winter tauschen** — soll ab dem 1. April bzw. 1. Oktober im Backlog auftauchen (wetterabhängig, kein konkreter Tag).

Erweitert das Vorlagen-Modell (§9.6) um zwei Achsen plus ein Transport-Konzept für geteilte Listen mit getrennten Sync-Quellen.

#### Datenmodell

```rust
pub struct TaskRecurrence {
    // … bestehend: frequency, interval, day_of_week, day_of_month, end …
    pub anchor: RecurrenceAnchor,           // ab wann zählt das Intervall?
    pub placement: RecurrencePlacement,     // wohin geht die nächste Instanz?
    pub fixed_dates: Option<Vec<MonthDay>>, // gesetzt ⇒ Trigger statt freq/interval
}
pub enum RecurrenceAnchor { FromDate, FromCompletion }  // Default FromDate (heutiges Verhalten)
pub enum RecurrencePlacement { Schedule, Backlog }      // Default Schedule (heutiges Verhalten)
pub struct MonthDay { pub month: u8, pub day: u8 }      // z.B. {4,1}, {10,1}

pub struct Task {
    // … bestehend …
    pub resurface_date: Option<NaiveDate>,  // im Backlog erst ab diesem Datum sichtbar; None = sofort
    pub series_id: Option<String>,          // identifiziert die Serie für idempotentes Spawnen
}
```

- `anchor=FromDate, placement=Schedule, fixed_dates=None` = bisheriges Verhalten → RRULE-Round-Trip (§9.7) unverändert.
- Geschirrspüler: `{ anchor: FromCompletion, interval: 0, placement: Backlog }`
- Schuhe: `{ fixed_dates: [{4,1},{10,1}], placement: Backlog }`

#### Spawner-Verhalten

Bei `open→completed` einer Instanz mit Wiederholung wird die nächste Instanz erzeugt (§9.6). Neu:

- **`placement=Backlog`** → nächste Instanz `scheduled_date=None`, `resurface_date` =
  - `FromCompletion`: Abschlussdatum + Intervall (sofort, wenn Intervall 0/leer)
  - `fixed_dates`: nächstes der Daten nach dem Abschluss
- Die Instanz erbt die `series_id` der Vorlage.
- **Läuft auch für externe Listen.** Bisher spawnt bei externen Aufgaben der Provider (über die RRULE); Bedarfs-/Backlog-Wiederholungen kennt kein Provider → **Aperio spawnt selbst**, für alle Listen.

##### Idempotenz bei geteilten Listen

Zwei Aperio-Clients gegen denselben Provider dürfen beim Abhaken **nicht beide** eine Folge-Instanz erzeugen:

- Jede Serie trägt eine stabile **`series_id`** (im Extras-Beutel transportiert).
- **Spawn-Regel:** neue Instanz nur, wenn **keine offene Instanz dieser `series_id`** im gesyncten Bestand existiert. Wer zuerst synct, erzeugt sie; der andere sieht sie schon.
- **Sicherheitsnetz:** Dedup-on-read — gibt es doch zwei offene Instanzen einer `series_id` (Race), wird die kanonische (älteste) behalten.

#### Aperio-Extras: Transport nicht-nativer Felder

Die obigen Felder kennt kein externer Provider nativ. Für **geteilte Listen** ist der einzige gemeinsame Kanal der Provider selbst → Aperio bettet sie als **generischen, versionierten Beutel** ein:

```jsonc
{ "v": 1, "extras": { "recurrence": { … }, "resurface_date": "2026-10-01", "series_id": "…" } }
```

**Kanal pro Adapter** (unsichtbar wo möglich):

| Adapter | Kanal |
|---|---|
| local | native DB-Spalten — **kein** Codec |
| Vikunja, Todoist, Google | **sichtbarer „managed block"** im Beschreibungsfeld |
| CalDAV | `X-APERIO-EXTRAS`-Property (unsichtbar) |
| EWS | Extended MAPI Property (unsichtbar) |
| Microsoft Graph | Open Extension (unsichtbar) |

**Sichtbarer Block** (nur Plaintext-Provider ohne Custom-Property-Kanal) — am Ende der Beschreibung, mit zweisprachiger Warnung und base64-Payload (HTML-sicher gegen Vikunjas Rich-Text-Editor):

```
<Beschreibung des Nutzers>

— ⚙ Aperio · bitte nicht bearbeiten / please don't edit —
aperio:1:<base64(json)>
```

- **Lesen:** Block/Property extrahieren → `Task.description` bleibt sauber, Felder landen in echten Task-Feldern. Fehlt/kaputt ⇒ als normale Aufgabe behandeln (sauberes Degradieren, nie Datenverlust).
- **Schreiben (defensiver Merge):** vor dem Schreiben die aktuelle Beschreibung **neu lesen**, **nur den Block ersetzen**, restlichen Text erhalten. Nur schreiben, wenn sich die Extras tatsächlich ändern.
- Codec liegt in `cal-core` (`extras::{embed, extract, …}`); jeder Adapter wählt seinen Kanal.

#### Frontend

- **„Zukünftig (N)"-Gruppe** in der Aufgaben-Ansicht — analog zur „Erledigt (N)"-Gruppe: navigierbare, einklappbare Baum-Zeile (`DEFERRED_GROUP_ID`) mit allen Aufgaben `resurface_date > heute`; zeigt je Aufgabe das Auftauch-Datum; Kontextmenü „ins Backlog holen" (löscht `resurface_date`). Standard eingeklappt, Zustand gemerkt.
- **Backlog-Filter** (§9.3) blendet `resurface_date > heute` aus dem aktiven Backlog aus.
- **Recurrence-Selector**: `anchor` (ab Datum / ab Abschluss), `placement` (einplanen / im Backlog auftauchen), Fixed-Dates-Eingabe — gegated über die `recurrence`-Capability (§9.7).
- `resurface_date` löst **nicht** den „Verpasste Aufgaben"-Flow (§9.5) aus — weicher Auftauch-Trigger, keine Deadline.

#### Phasen

1. `cal-core::extras` — Beutel + sichtbarer-Block-Codec (embed/extract/defensiver Merge) + Round-Trip-/Degradier-Tests.
2. Datenmodell — `anchor`/`placement`/`fixed_dates`/`resurface_date`/`series_id` durch alle Schichten (DB-Migration, Event-Log, TS).
3. Spawner — Backlog-Placement, resurface, Fixed-Dates, `series_id`-Idempotenz, Lauf für externe Listen.
4. Frontend — „Zukünftig"-Gruppe, Backlog-Filter, Recurrence-Selector-Knöpfe.
5. Kanäle — Vikunja (sichtbar) zuerst; dann CalDAV X-Props, EWS Extended Props, Graph Open Extension.

---

## 10. Kontakte & CardDAV-Integration

### 10.1 Designprinzip: Kontakte als Teil bestehender Adapter

Kontaktzugriff wird direkt in die bestehenden Kalender-Adapter integriert. Der `cal-adapter-caldav`-Adapter vereint CalDAV (Kalender und Aufgaben) und CardDAV (Kontakte) in einem Plugin – beide Protokolle sind WebDAV-Erweiterungen, laufen typischerweise auf demselben Server und teilen dieselbe Authentifizierung. CalDAV und CardDAV können im Plugin unabhängig voneinander aktiviert werden:

```
┌─────────────────────────────────────────────────┐
│  Server hinzufügen: Nextcloud                   │
│                                                 │
│  URL:      https://cloud.example.com            │
│  Benutzer: max@example.com                      │
│  Passwort: ••••••••                             │
│                                                 │
│  Protokolle:                                    │
│  [x] CalDAV  – Kalender und Aufgaben (VTODO)    │
│  [x] CardDAV – Kontakte & Geburtstage           │
│                                                 │
│  [Verbinden]                [Abbrechen]         │
└─────────────────────────────────────────────────┘
```

> **Hinweis:** Bei CalDAV werden Aufgaben (VTODO) immer zusammen mit Kalendern aktiviert – auf Protokollebene sind sie nicht trennbar, da VTODO-Aufgaben im selben Kalender-Container leben. Das CalDAV-Toggle steuert also gleichzeitig die `calendar`- und `tasks`-Capability des Adapters. CardDAV steuert die `contacts`-Capability.

Jede Verbindung wird **separat und unabhängig** konfiguriert. Derselbe Adapter kann beliebig oft mit unterschiedlichen Servern und unterschiedlichen Protokoll-Kombinationen instanziiert werden:

| Verbindung | CalDAV | CardDAV | Beispiel |
|---|---|---|---|
| Nextcloud Arbeit | ✅ | ✅ | Gleicher Server für beides |
| Nextcloud Privat | ✅ | ❌ | Nur Kalender, kein Kontaktbuch |
| Fastmail | ❌ | ✅ | Nur Kontakte, kein Kalender |
| iCloud Familie | ✅ | ✅ | Zweiter iCloud-Account |

Das gilt analog für alle anderen Adapter – mehrere Google-Accounts, mehrere iCloud-Accounts, mehrere EWS-Quellen, mehrere CalDAV-Server, mehrere Vikunja-Instanzen, mehrere Todoist-Konten etc. sind alle gleichzeitig verbindbar. Jede Verbindung erscheint im Filter-Panel als eigenständiger Eintrag mit eigenem Namen und eigener Farbe.

Für Google, Microsoft und iCloud gilt dasselbe Prinzip – Kalender, Aufgaben und Kontakte teilen das OAuth2-Token:

| Adapter | Kalender-API | Aufgaben-API | Kontakte-API | Authentifizierung |
|---|---|---|---|---|
| `cal-adapter-google` | Google Calendar API | Google Tasks API | Google People API | Selbes OAuth2-Token |
| `cal-adapter-microsoft-graph` | Microsoft Graph Calendar | Microsoft To Do API | Microsoft Graph Contacts | Selbes OAuth2-Token |
| `cal-adapter-caldav` | CalDAV RFC 4791 | VTODO via CalDAV | CardDAV RFC 6352 | Selbe Zugangsdaten |
| `cal-adapter-ews` | EWS Calendar | EWS Tasks | EWS Contacts | Selbe Zugangsdaten |

Auch bei diesen Adaptern sind Kalender-, Aufgaben- und Kontakt-Sync unabhängig voneinander aktivierbar – ein Nutzer kann z.B. Google Kalender synchronisieren, aber Google Contacts deaktivieren.

### 10.2 Erweiterung des Plugin-Manifests

Das Plugin-Manifest (Abschnitt 20.4) deklariert, welche Features ein Adapter unterstützt und welche davon der Nutzer aktivieren kann:

```json
{
  "id": "com.aperio.app.adapter.caldav",
  "plugin_type": "calendar-adapter",
  "capabilities": ["calendar", "tasks", "contacts"]
}
```

```json
{
  "id": "com.aperio.app.adapter.google",
  "plugin_type": "calendar-adapter",
  "capabilities": ["calendar", "tasks", "contacts"]
}
```

```json
{
  "id": "com.aperio.app.adapter.vikunja",
  "plugin_type": "calendar-adapter",
  "capabilities": ["tasks"]
}
```

Capabilities benennen **Features**, nicht Protokolle. Die möglichen Werte sind: `calendar`, `tasks`, `contacts` (und können in zukünftigen Versionen erweitert werden). Welches Protokoll der Adapter intern nutzt (CalDAV, CardDAV, Graph API, REST etc.) ist Implementierungsdetail des Adapters und in den Capabilities nicht sichtbar.

Ein Adapter kann jede beliebige Kombination der drei Capabilities deklarieren – auch nur eine einzelne. Reine Aufgabenlisten-Adapter wie Todoist oder Vikunja deklarieren `["tasks"]` und stellen keine Kalender oder Kontakte bereit. Der Plugin-Typ bleibt einheitlich `calendar-adapter`, da das Plugin dasselbe `Adapter`-Basistrait implementiert; welche Feature-Traits zusätzlich implementiert werden (`CalendarFeature`, `TasksFeature`, `ContactsFeature`), regeln die deklarierten `capabilities`.

Der Plugin-Manager zeigt im Einrichtungsdialog nur die Capabilities an, die der jeweilige Adapter unterstützt. Der Nutzer aktiviert nur, was er benötigt.

### 10.3 Geburtstags-Kalender

Geburtstage aus verbundenen Kontaktbüchern werden als **eigener, nicht-editierbarer Kalender-Layer** angezeigt – analog zu abonnierten Feiertags-iCals (Abschnitt 12).

- Pro verbundenem Kontaktbuch gibt es einen eigenen Geburtstags-Layer (z.B. "Geburtstage – Google", "Geburtstage – iCloud")
- Layer sind einzeln ein-/ausblendbar
- Geburtstage erscheinen als Ganztages-Termine mit dem Namen des Kontakts
- Nicht editierbar in der App – Änderungen am Geburtsdatum erfolgen im jeweiligen Kontaktbuch
- Erinnerungen für Geburtstage sind konfigurierbar (z.B. 1 Tag vorher), analog zu normalen Terminen
- ARIA-Label: "Geburtstag: Max Mustermann, Kalender: Google Kontakte"

#### Datenhaltung

Geburtstagsdaten werden lokal in SQLite gecacht und bei jedem Kontakt-Sync aktualisiert. Da Geburtstage rein aus den Kontaktdaten abgeleitet werden, gibt es kein eigenes API-Ereignis – sie werden beim Start und bei konfigurierbarem Intervall neu berechnet.

### 10.4 Teilnehmer-Auswahl

Bei der Terminerstellung und -bearbeitung können Teilnehmer aus verbundenen Kontaktbüchern ausgewählt werden.

#### Autocomplete-Suche

Das Teilnehmer-Eingabefeld bietet eine Live-Autocomplete-Suche über alle verbundenen Kontaktbücher:

```
Teilnehmer hinzufügen: [max_________]
                        ┌──────────────────────────────┐
                        │ Max Mustermann               │
                        │ max@example.com              │
                        │ ── Google Contacts ──        │
                        │                              │
                        │ Maximilian Muster            │
                        │ m.muster@firma.de            │
                        │ ── iCloud ──                 │
                        └──────────────────────────────┘
```

- Suche erfolgt lokal gegen den SQLite-Kontakt-Cache (keine API-Anfrage pro Tastendruck)
- Ergebnisse zeigen Name, primäre E-Mail-Adresse und Kontaktbuch-Quelle
- Mehrere E-Mail-Adressen eines Kontakts werden als separate Einträge angeboten
- Freie Eingabe einer E-Mail-Adresse bleibt möglich (für Kontakte, die nicht im Kontaktbuch sind)

#### Barrierefreiheit

- Eingabefeld hat `role="combobox"` mit `aria-autocomplete="list"`
- Vorschlagsliste hat `role="listbox"`, jeder Eintrag `role="option"` mit vollständigem `aria-label` (Name + E-Mail + Quelle)
- Auswahl per `Enter`, Navigation per Pfeiltasten, Schließen per `Escape`
- Ausgewählte Teilnehmer erscheinen als entfernbare Tags in einem `role="list"`-Container; jeder Tag hat `role="listitem"` mit einem Button (`aria-label="Max Mustermann entfernen"`) zum Entfernen

### 10.5 Kontakt-Sync

Kontakte werden bei folgenden Ereignissen synchronisiert:

| Auslöser | Verhalten |
|---|---|
| App-Start | Vollständiger Abgleich aller Kontaktbücher |
| Konfigurierbares Intervall | Standard: alle 60 Minuten (konfigurierbar) |
| Manuell | Per Button in `Einstellungen → Kontakte` |

Kontaktdaten werden **nicht** über das Event Log zwischen Geräten synchronisiert – sie kommen auf jedem Gerät direkt vom jeweiligen Anbieter, analog zur externen Kalender- und Aufgaben-Synchronisation (Abschnitt 18.2).

### 10.6 Datenschutz-Hinweis

Beim ersten Verbinden eines Kontaktbuchs wird der Nutzer darauf hingewiesen, dass Kontaktdaten (Namen, E-Mail-Adressen, Geburtstage) lokal in SQLite gecacht werden. Ein Link zu den Datenschutzrichtlinien des jeweiligen Anbieters wird angezeigt. Der Cache kann unter `Einstellungen → Kontakte → Cache leeren` jederzeit gelöscht werden.

---

## 11. Videokonferenz-Integration

### 11.1 Unterstützte Anbieter (v1.0)

| Anbieter | Funktion | Auth |
|---|---|---|
| **Zoom** | Link generieren + Raum buchen | Zoom OAuth2 |
| **Microsoft Teams** | Link generieren + Raum buchen | Microsoft Graph OAuth2 |
| **Google Meet** | Link generieren + Raum buchen | Google OAuth2 |
| **Cisco WebEx** | Link generieren + Raum buchen | WebEx OAuth2 |

### 11.2 Funktionsumfang

- **Link-Generierung:** Beim Erstellen eines Termins kann ein Videokonferenz-Link automatisch erstellt und in die Beschreibung eingefügt werden
- **Raumverwaltung:** Auswahl verfügbarer Konferenzräume nach Verfügbarkeit und Kapazität
- **Direkt beitreten:** In der Terminansicht ist ein "Meeting beitreten"-Button vorhanden (öffnet den Anbieter im Browser oder der nativen App)
- **Barrierefreiheit:** Der "Beitreten"-Button ist per Tastatur erreichbar und hat ein aussagekräftiges `aria-label`

### 11.3 Technische Umsetzung

Jeder Anbieter wird als eigenständiges `videoconference-adapter`-Plugin (Plugin-Typ siehe Abschnitt 20.2) implementiert:

| Crate | API-Basis | Besonderheit |
|---|---|---|
| `vc-adapter-zoom` | Zoom Meeting API v2 | Eigener OAuth2-Flow |
| `vc-adapter-teams` | Microsoft Graph API (Online Meetings) | Teilt OAuth2-Token mit `cal-adapter-microsoft-graph` |
| `vc-adapter-meet` | Google Calendar API (conferenceData) | Teilt OAuth2-Token mit `cal-adapter-google` |
| `vc-adapter-webex` | Cisco WebEx Meetings REST API | Eigener OAuth2-Flow |

Teams und Meet teilen das OAuth2-Token des jeweiligen Kalender-Adapters – es ist keine separate Anmeldung nötig, wenn der Nutzer bereits Google Calendar oder Outlook verbunden hat.

---

## 12. Feiertage (per iCal-Abonnement)

Eine eigene Feiertags-API ist nicht vorgesehen. Der `cal-adapter-ical` deckt den Use Case vollständig ab: die User abonnieren einen öffentlichen Feiertags-Feed ihrer Wahl als read-only Kalender und sehen Feiertage in allen Ansichten wie jeden anderen Kalender. Beispiele:

- `https://www.feiertage-deutschland.de/feiertage-de.ics` (alle deutschen Bundesländer)
- `https://www.officeholidays.com/ics/germany` (offizielle Feiertage Deutschland)
- Apple/Google bieten regionale Feiertags-iCals für ~100 Länder

Vorteile gegenüber einer eingebauten API:

- **Keine externe Abhängigkeit** in der Aperio-Binary — der iCal-Adapter ist sowieso vorhanden
- **Frei wählbare Quelle** — User entscheiden selbst, welchen Feed sie vertrauen
- **Mehrere Länder gleichzeitig** funktioniert genauso: ein iCal-Abo pro Land
- **Bundesland-spezifische Feiertage** werden von den meisten Feed-Anbietern bereits getrennt angeboten
- **Caching** geschieht über den existierenden 30-Sekunden-TTL des iCal-Adapters; lokale SQLite-Persistenz pro Abo

Aperio empfiehlt im Account-Dialog optional einen kuratierten Default-Feed pro User-Locale beim ersten Start, der dann nach Wunsch ergänzt oder ersetzt werden kann.

---

## 13. Suche

### 13.1 Suchumfang

Die Suche durchsucht alle lokal gecachten Termine und Aufgaben über alle Konten und Container hinweg. Durchsuchte Felder:

| Feld | Termine | Aufgaben |
|---|---|---|
| Titel | ✅ | ✅ |
| Beschreibung | ✅ | ✅ |
| Ort | ✅ | – |
| Teilnehmer (Name / E-Mail) | ✅ | – |
| Kalender-Name | ✅ | – |
| Aufgabenlisten-Name | – | ✅ |
| Anhänge (Dateiname) | ✅ | – |
| Farb-Label | ✅ | ✅ |

### 13.2 Technische Umsetzung

Die Volltextsuche wird direkt über SQLites eingebaute **FTS5-Engine** (Full-Text Search) realisiert – ohne externe Abhängigkeiten. Separate FTS5-Tabellen für Termine und Aufgaben ermöglichen typenspezifische Suche und Filterung. Felder, die in anderen Tabellen liegen (Kalender-Name, Aufgabenlisten-Name, Farb-Label), werden als denormalisierte Spalten in die FTS-Tabellen gespiegelt und bei Änderungen über Trigger oder Anwendungslogik synchronisiert:

```sql
CREATE VIRTUAL TABLE events_fts USING fts5(
    title,
    description,
    location,
    attendees,
    attachments,    -- Komma-getrennte Dateinamen
    calendar_name,  -- Denormalisiert aus calendars-Tabelle
    color_label,    -- Denormalisiert aus color_labels-Tabelle
    content='events',
    content_rowid='id'
);

CREATE VIRTUAL TABLE tasks_fts USING fts5(
    title,
    description,
    list_name,      -- Denormalisiert aus task_lists-Tabelle
    color_label,    -- Denormalisiert aus color_labels-Tabelle
    content='tasks',
    content_rowid='id'
);
```

Suchanfragen werden als Tauri-Command an das Rust-Backend übergeben und dort gegen beide FTS5-Indizes ausgeführt. Ergebnisse werden zusammengeführt, nach Relevanz (FTS5-Ranking) und Datum sortiert und mit einem Typ-Kennzeichen (Termin / Aufgabe) zurückgegeben.

**Externe Konten (Snapshot-Cache).** Termine und Aufgaben externer Anbieter liegen nicht in den lokalen `events`/`tasks`-Tabellen, sondern im Snapshot-Cache (`cache_events` / `cache_tasks`, Abschnitt CACHE-1). Damit der Suchumfang aus 13.1 („alle lokal gecachten … über alle Konten hinweg") tatsächlich gilt, führen Trigger-gepflegte FTS5-Spiegel (`cache_events_fts` / `cache_tasks_fts`) die Textfelder der Cache-Payloads nach (per `json_extract` beim Schreiben). Der Such-Command fragt beide Hälften mit demselben präparierten Präfix-Query ab und mergt die Treffer; Filter (Kalender/Liste, Zeitraum, Termin-Typ, Aufgaben-Status) wirken auf beiden Seiten identisch.

### 13.3 Bedienung

- Suche aufrufbar per `Ctrl+F` aus jeder Ansicht heraus
- Sucheingabe erscheint als fokussiertes Eingabefeld (kein Seitenwechsel)
- Ergebnisse erscheinen live während der Eingabe (Debounce: 200 ms)
- Ergebnisliste ist per Pfeiltasten navigierbar; `Enter` öffnet das fokussierte Ergebnis (Termin oder Aufgabe), `Escape` schließt die Suche

### 13.4 Filteroptionen

Zusätzlich zur Freitextsuche können Ergebnisse gefiltert werden:

| Filter | Optionen |
|---|---|
| **Zeitraum** | Vergangenheit / Zukunft / Benutzerdefiniert (Von–Bis) |
| **Kalender** | Einzelne oder mehrere Kalender auswählbar (nur bei Filter "Termine" oder "Beide") |
| **Aufgabenliste** | Einzelne oder mehrere Aufgabenlisten auswählbar (nur bei Filter "Aufgaben" oder "Beide") |
| **Typ** | Termine / Aufgaben / Beide |
| **Termintyp** | Einmalig / Wiederkehrend / Ganztags (nur bei Filter "Termine" oder "Beide") |
| **Aufgabenstatus** | Offen / In Bearbeitung / Erledigt (nur bei Filter "Aufgaben" oder "Beide") |

Filter sind per Tastatur erreichbar (`Tab` nach der Sucheingabe) und als `role="group"` mit `aria-label` beschriftet.

### 13.5 Barrierefreiheit

- Sucheingabefeld hat `aria-label="Termine und Aufgaben durchsuchen"` und `role="searchbox"`
- Ergebnisliste hat `role="listbox"`, jeder Treffer `role="option"` mit vollständigem `aria-label` (Typ, Titel, Datum, Container) – z.B. "Termin: Teammeeting, 14. Mai 2025, Kalender: Arbeit" oder "Aufgabe: Bericht schreiben, fällig 15. Mai 2025, Aufgabenliste: Projekte"
- Anzahl der Ergebnisse wird per `aria-live="polite"` angekündigt ("12 Ergebnisse gefunden")
- Keine Ergebnisse: Meldung "Keine Ergebnisse gefunden" wird per `aria-live` angekündigt

---

## 14. Erinnerungen & Benachrichtigungen

### 14.1 Erinnerungstypen

Erinnerungen können sowohl für **Termine** als auch für **Aufgaben** konfiguriert werden. Pro Termin/Aufgabe sind mehrere Erinnerungen möglich:

| Typ | Beschreibung |
|---|---|
| **Zeitbasiert (relativ, Termin)** | X Minuten / Stunden / Tage vor Terminbeginn |
| **Zeitbasiert (relativ, Aufgabe)** | X Minuten / Stunden / Tage vor der Deadline bzw. dem geplanten Tag |
| **Zeitbasiert (absolut)** | Fester Zeitpunkt (z.B. "12. Mai, 09:00 Uhr") |
| **Beim Öffnen der App** | Erinnerung beim nächsten App-Start nach dem Fälligkeitszeitpunkt |
| **E-Mail** | Erinnerung per E-Mail (wo vom jeweiligen Adapter unterstützt) |

Standardwerte für neue Termine und Aufgaben sind global und unabhängig voneinander konfigurierbar (z.B. "Termine: 15 Minuten vorher", "Aufgaben: am Vorabend um 18:00 Uhr").

> **Hinweis Aufgaben ohne Datum:** Aufgaben im Backlog ohne Deadline oder geplanten Tag erhalten keine zeitbasierten Erinnerungen. Sie erscheinen ausschließlich in der Aufgaben-Ansicht und im Backlog-Panel.

### 14.2 API-Integration: Native Erinnerungen

Wo die Kalender-API eigene Erinnerungsdaten mitliefert, werden diese direkt übernommen und bidirektional synchronisiert:

| Adapter | Native Erinnerungen | Synchronisation |
|---|---|---|
| **Google Calendar** | ✅ Ja – `reminders.overrides` (Popup, E-Mail) + `reminders.useDefault` | Bidirektional |
| **Microsoft Graph (Outlook)** | ✅ Ja – `reminderMinutesBeforeStart` | Bidirektional |
| **Apple iCloud (CalDAV)** | ✅ Ja – `VALARM`-Komponenten im iCal-Standard (RFC 5545) | Bidirektional |
| **Exchange EWS** | ✅ Ja – `ReminderMinutesBeforeStart` | Bidirektional |
| **CalDAV (generisch)** | ✅ Ja – `VALARM` per RFC 5545 | Bidirektional |
| **iCal (.ics)** | ✅ Ja – `VALARM` (lesend) | Nur lesend |

Analog dazu werden Aufgaben-Erinnerungen mit den jeweiligen Aufgaben-APIs synchronisiert, wo sie unterstützt werden (Google Tasks, Microsoft To Do, VTODO mit `VALARM`, Vikunja `reminders`, Todoist `due.datetime` – siehe Abschnitt 9.7 für Adapter-Details).

> **Hinweis:** API-seitige Erinnerungen steuern in der Regel den Versand von E-Mail-Benachrichtigungen durch den Anbieter (z.B. Google sendet eine E-Mail). Die lokalen Push-Benachrichtigungen (Abschnitt 14.3) werden davon unabhängig durch die App selbst ausgelöst.

### 14.3 Lokale Push-Benachrichtigungen

Erinnerungen werden als native Systembenachrichtigungen zugestellt – unabhängig davon, ob die App im Vordergrund ist. Hierfür wird `tauri-plugin-notification` verwendet.

#### Plattformverhalten

| Plattform | Benachrichtigungs-API | Besonderheiten |
|---|---|---|
| **Windows** | Windows Toast Notifications (WinRT) | Erscheinen im Aktionscenter; bleiben bei Nichtbeachtung erhalten |
| **macOS** | `UNUserNotificationCenter` | Erfordert Nutzer-Erlaubnis beim ersten Start |
| **Linux** | `libnotify` / D-Bus | Verhalten abhängig vom Desktop-Environment (GNOME, KDE, etc.) |

#### Benachrichtigungsinhalt (Beispiel Termin)

```
┌─────────────────────────────────────┐
│ 🔔  Aperio                          │
│ Teammeeting in 15 Minuten           │
│ 10:00–11:00 Uhr · Raum 3.12         │
│ [Öffnen]  [Snooze: 5 Min]           │
└─────────────────────────────────────┘
```

#### Benachrichtigungsinhalt (Beispiel Aufgabe)

```
┌─────────────────────────────────────┐
│ ✅  Aperio – Aufgabe                │
│ Bericht schreiben                   │
│ Fällig morgen, 17:00 Uhr            │
│ [Öffnen]  [Erledigt]  [Snooze]      │
└─────────────────────────────────────┘
```

- **Titel:** Termin- oder Aufgabenname
- **Untertitel:** Bei Terminen Zeitangabe + Ort; bei Aufgaben Fälligkeitsangabe (z.B. "Fällig morgen, 17:00 Uhr" oder "Geplant für heute")
- **Aktionen:** "Öffnen" (springt zum Termin/zur Aufgabe in der App), "Snooze" (konfigurierbare Snooze-Dauer); bei Aufgaben zusätzlich "Erledigt" (markiert die Aufgabe direkt als abgeschlossen)
- Screen Reader lesen den Benachrichtigungsinhalt vollständig vor (plattformseitig gewährleistet)

### 14.4 Sound-Konfiguration

Benachrichtigungssounds sind **hierarchisch konfigurierbar** – von der globalen Ebene bis zum einzelnen Termin bzw. zur einzelnen Aufgabe:

```
Globale Einstellung (App-Standard)
        │
        ▼
Container-Ebene: Kalender oder Aufgabenliste (überschreibt Global)
        │
        ▼
Termin- oder Aufgaben-Ebene (überschreibt Container)
        │
        ▼
Erinnerungs-Ebene: einzelner Alarm (überschreibt alle obigen)
```

> **Hinweis zu wiederkehrenden Terminen/Aufgaben:** Es gibt keine separate Serien-Ebene. Der Termin-/Aufgaben-Override ist auf die **Serien-ID** geschlüsselt (`sound.item.{seriesId}`) und gilt damit für alle Instanzen. Eine wiederkehrende Aufgabe ist eine **Task-Vorlage** (`Task` mit `recurrence`-Feld); ihr Override gilt für alle daraus erzeugten Instanzen.

> **Status:** Implementiert — Auflösung, Wiedergabe (inkl. eigener Dateien) und die komplette UI (global, Container, Termin/Aufgabe, Erinnerung) sind vorhanden.

#### Ebenen im Detail

| Ebene | Konfigurierbar? | Sync mit API | Speicherort |
|---|---|---|---|
| **Global (App-Standard)** | ✅ Ja | ❌ Nein (keine API) | `user_prefs` → `sound.global` → Event Log Sync |
| **Container (Kalender / Aufgabenliste)** | ✅ Ja | ❌ Nein (nicht in APIs vorgesehen) | `user_prefs` → `sound.calendar.{id}` / `sound.tasklist.{id}` → Event Log Sync |
| **Termin / Aufgabe** | ✅ Ja | ❌ Nein | `user_prefs` → `sound.item.{id}` (Serien-ID bei Serien) → Event Log Sync |
| **Erinnerung** (einzelner Alarm) | ✅ Ja | ❌ Nein | im `Reminder`-Objekt (`Reminder.sound`) → Event Log Sync |

Da keiner der Adapter (Kalender, Aufgaben, Kontakte) Sound-Konfigurationen in seinen APIs unterstützt, werden Sound-Einstellungen **nicht** mit externen Diensten synchronisiert. Sie werden jedoch über das **Event Log (Abschnitt 19)** zwischen den eigenen Geräten synchronisiert – inklusive der zugehörigen Audiodateien (Abschnitt 19.2.2). Jedes Gerät hat damit dieselbe Sound-Konfiguration, ohne dass externe Anbieter davon wissen.

#### Speicher-Modell (einheitlich)

Alle Override-Ebenen außer der Erinnerungs-Ebene liegen einheitlich in `user_prefs` unter dem Prefix `sound.` (steht in der Sync-Whitelist, Abschnitt 19.2.1). Das funktioniert identisch für **lokale und externe** Kalender/Items und überlebt einen Cache-Refresh. Die Erinnerungs-Ebene steckt im jeweiligen `Reminder`-Objekt. Die cal-core-Felder `Event.sound`/`Task.sound` und die Traits `Reminderable`/`Container` bleiben für lokale Adapter und Vorwärtskompatibilität erhalten, sind aber **nicht** die Auflösungsquelle.

#### Vererbungslogik

Der Reminder-Scheduler lädt einmal pro Scan einen `SoundPrefs`-Snapshot aller `sound.*`-Prefs (vor dem Sperren der DB-Verbindung — `std::sync::Mutex` ist nicht reentrant) und löst pro ausgelöster Erinnerung auf, spezifischste Ebene zuerst:

```
reminder.sound
  ?? prefs["sound.item.{itemId}"]
  ?? prefs["sound.{calendar|tasklist}.{containerId}"]
  ?? prefs["sound.global"]
  ?? System            // SoundConfig::default()
```

#### Sound-Optionen

- **Systemsound** (Standard): Plattformeigener Benachrichtigungssound (das OS spielt ihn)
- **Kein Sound:** Stille Benachrichtigung (nur visuell)
- **Benutzerdefiniert:** Eigene Audiodatei (`.mp3`, `.ogg`, `.wav`, `.m4a`, `.aac`, `.flac`, ≤ 5 MB), inhaltsadressiert per SHA-256

> **Lautstärke:** Eine app-eigene Lautstärkeregelung gibt es bewusst **nicht** — Windows und macOS bieten einen System-Mixer pro Anwendung. Das `volume`-Feld bleibt im Modell erhalten (Vorwärtskompatibilität), wird aber nicht angezeigt oder angewandt.

#### Wiedergabe

`tauri-plugin-notification` kann nur einen **benannten System-Ton** oder Stille auslösen, keine beliebige Datei. Daher:

| Quelle | Verhalten |
|---|---|
| Systemsound | OS-Standardton + visuell (`.show()`) |
| Kein Sound | stille Benachrichtigung, nur visuell (`.silent()`) |
| Benutzerdefiniert | stille Benachrichtigung **+** Datei wird selbst per `rodio` auf einem dauerhaften, dedizierten Audio-Thread abgespielt |

Fehlt eine referenzierte Custom-Datei lokal, fällt die Wiedergabe auf den Systemsound zurück (kein Absturz, keine stille Erinnerung). Custom-Dateien liegen unter `<data_dir>/assets/sounds/<hash>.<ext>` und werden über den Asset-Store (Abschnitt 19.2.2 / 19.11.7) zwischen Geräten synchronisiert; der Asset-Push erfasst dabei auch nur über `user_prefs` referenzierte Hashes.

#### Sonderfälle

- **Nicht stören / Fokus-Modus:** Die App respektiert die plattformseitigen "Nicht stören"-Einstellungen (Windows Fokus-Assistent, macOS Fokus-Modi)
- **Terminserie / wiederkehrende Aufgabe:** Bei Terminserien gilt die Sound-Einstellung für die gesamte Serie, kann aber für einzelne Instanzen überschrieben werden. Bei wiederkehrenden Aufgaben (Vorlagen-Modell, siehe Abschnitt 9.6) gilt die Sound-Einstellung der Vorlage für alle daraus erzeugten Instanzen
- **Mehrere Erinnerungen pro Termin oder Aufgabe:** Jede Erinnerung kann einen eigenen Sound haben

### 14.5 Snooze-Funktion

- Snooze-Optionen: 5 Min, 10 Min, 15 Min, 30 Min, 1 Std (konfigurierbar)
- Snooze ist ausschließlich lokal – es wird kein neuer API-seitiger Alarm gesetzt
- Maximale Snooze-Anzahl pro Erinnerung konfigurierbar (Standard: unbegrenzt)

### 14.6 Barrierefreiheit

- Alle Benachrichtigungen werden von Screen Readern plattformseitig vorgelesen
- Die Snooze- und Öffnen-Aktionen sind per Tastatur erreichbar (plattformseitig)
- In der App selbst gibt es eine **Erinnerungs-Übersicht** (zugänglich per `Ctrl+Shift+R`): eine chronologische Liste aller anstehenden und vergangenen Erinnerungen, vollständig tastaturnavigierbar

---

## 15. Native Desktop-Erfahrung & Tastaturkürzel

### 15.1 Designprinzip: "Native First"

Das Ziel ist, dass sich die App trotz WebView-Basis **nicht wie eine Webseite anfühlt** – sondern wie eine native Desktop-Anwendung. Synology DSM 7 ist hierfür ein treffendes Referenzbeispiel: konsequente Fenster-Metaphern, sofortige Reaktionen, kein sichtbares Browser-Verhalten, und – besonders wichtig für Screen-Reader-Nutzer – dauerhafter Fokus-Modus ohne unerwünschte Moduswechsel.

Die beiden Kernmaßnahmen für das native Gefühl sind:

1. **`role="application"` auf dem Root-Element** (Abschnitt 3.2.1): NVDA und andere Screen Reader bleiben dauerhaft im Fokus-Modus – kein Wechsel in den Browse-Modus, kein Buchstaben-als-Schnellnavigation-Verhalten
2. **Konsequente Unterdrückung von Browser-Verhalten** (Abschnitt 15.2): Kein Kontextmenü des Browsers, keine Textauswahl auf nicht-editierbaren Elementen, keine Überscroll-Effekte

Die folgenden Maßnahmen werden systematisch umgesetzt:

### 15.2 Unterdrückung von Browser-Verhalten

Alle typischen "das ist eine Webseite"-Signale werden aktiv deaktiviert:

| Verhalten | Maßnahme |
|---|---|
| Textauswahl per Maus | Deaktiviert via `user-select: none` auf allen nicht-editierbaren Elementen |
| Kontextmenü des Browsers | Deaktiviert (`contextmenu`-Event abgefangen); stattdessen eigenes natives Kontextmenü |
| Drag-to-scroll (Browser-nativ) | Deaktiviert; eigenes Drag-Verhalten für Termine und Aufgaben implementiert |
| `Ctrl+R` / `F5` (Seitenreload) | Browser-Reload abgefangen; `Ctrl+R` wird stattdessen für die App-eigene Daten-Synchronisation verwendet (siehe Abschnitt 15.7), `F5` ist ohne Funktion |
| `Ctrl+U` (Seitenquelltext) | Abgefangen und deaktiviert |
| Eingabe-Caret in nicht-editierbaren Bereichen | Unterdrückt via `cursor: default` |
| Überscroll-Effekte (Bounce/Glow) | Deaktiviert via `overscroll-behavior: none` |
| Linkfarben / Unterstreichungen | Nur wo semantisch korrekt, kein Browser-Standard-Styling |

### 15.3 Natives Fenstermanagement

Tauri ermöglicht vollständige Kontrolle über die Titelleiste und das Fensterverhalten:

- **Benutzerdefinierte Titelleiste:** Native Fensterdekorationen werden durch eine eigene, plattformkonsistente Titelleiste ersetzt (Tauri `decorations: false` + eigene Titelleisten-Komponente)
- **Fenster verschieben:** Titelleiste ist per `data-tauri-drag-region` als Drag-Bereich registriert
- **Minimieren / Maximieren / Schließen:** Eigene Schaltflächen, die plattformspezifisch positioniert sind (rechts unter Windows/Linux, links unter macOS)
- **Fensterstatus merken:** Größe und Position werden beim Schließen gespeichert und beim nächsten Start wiederhergestellt (lokal in `app_config.json`)

### 15.4 Interaktionsqualität

Diese Maßnahmen sorgen für unmittelbares, "schnappiges" Feedback:

| Aspekt | Umsetzung |
|---|---|
| **Animationen** | Kurz und zweckorientiert (max. 150 ms); keine langen Fade-Ins; `prefers-reduced-motion` wird respektiert (Animationen dann deaktiviert) |
| **Hover-Zustände** | Sofortig (kein Delay), konsistent auf allen interaktiven Elementen |
| **Fokus-Indikatoren** | Gut sichtbarer Fokusring (kein Browser-Standard, sondern eigenes Design), nie deaktiviert |
| **Ladezeiten** | UI-Elemente werden sofort aus dem lokalen Cache gerendert; kein Warten auf Netzwerkantworten beim Start |
| **Scrollverhalten** | `scroll-behavior: auto` (kein "smooth scroll" außer bei expliziter Nutzeraktion) |
| **Cursor** | Kontextsensitiv: `default`, `pointer`, `text`, `grab`, `grabbing`, `not-allowed` je nach Element und Zustand |

### 15.5 Natives Kontextmenü

Rechtsklick (oder `Shift+F10` / `Kontextmenü-Taste`) öffnet ein kontextsensitives Menü, das über Tauris `tauri-plugin-menu` als echtes natives Betriebssystem-Menü gerendert wird – nicht als HTML-Dropdown:

| Kontext | Menüeinträge |
|---|---|
| **Klick auf Termin** | Öffnen, Bearbeiten, Duplizieren, In Kalender verschieben, In Kalender kopieren, Löschen |
| **Klick auf Aufgabe** | Öffnen, Bearbeiten, Als erledigt markieren, Datum ändern, In Liste verschieben, In Liste kopieren, In Backlog zurück, Löschen |
| **Klick auf leeren Zeitslot** | Neuer Termin um diese Zeit, Neue Aufgabe für diesen Tag |
| **Klick auf Kalender (Sidebar)** | Kalender bearbeiten, Farbe ändern, Ein-/Ausblenden, Abonnement aktualisieren |
| **Klick auf Aufgabenliste (Sidebar)** | Liste bearbeiten, Farbe ändern, Ein-/Ausblenden, Sync aktualisieren |
| **Klick auf Titelleiste** | Minimieren, Maximieren, Schließen |

### 15.6 Drag & Drop

Termine und Aufgaben können per Maus verschoben und in ihren Eigenschaften angepasst werden:

#### Termine

| Aktion | Verhalten |
|---|---|
| **Termin ziehen** | Termin auf neuen Zeitslot oder neuen Tag ziehen; Zielzeit wird als Tooltip angezeigt |
| **Unterkante ziehen** | Endzeitpunkt des Termins anpassen |
| **Auf anderen Kalender ziehen** (Sidebar) | Termin in anderen Kalender verschieben (löst Bestätigungs-Dialog aus); mit gedrückter `Strg`/`Cmd`-Taste: kopieren |
| **Abbrechen** | `Escape` während des Ziehens bricht die Aktion ab |

#### Aufgaben

| Aktion | Verhalten |
|---|---|
| **Aufgabe auf Tag ziehen** | Setzt das geplante Datum der Aufgabe (aus dem Backlog auf einen Kalendertag) |
| **Aufgabe auf andere Aufgabenliste ziehen** (Sidebar) | Aufgabe in andere Liste verschieben; mit gedrückter `Strg`/`Cmd`-Taste: kopieren |
| **Aufgabe in der Aufgaben-Ansicht ziehen** | Neu-Sortieren innerhalb einer Liste (manuelle Reihenfolge) |
| **Abbrechen** | `Escape` während des Ziehens bricht die Aktion ab |

Für Tastatur- und Screen-Reader-Nutzer stehen Verschieben, Kopieren und Sortieren alternativ über Kontextmenü und Bearbeitungsdialog zur Verfügung (Drag & Drop ist keine Pflicht-Interaktion).

### 15.7 Vollständige Tastaturkürzel-Referenz

Alle Kürzel sind individuell umbelegbar (siehe Abschnitt 15.10), um Konflikte mit Screen-Reader-Belegungen zu vermeiden. Die folgende Tabelle zeigt die Standardbelegung:

#### Navigation

| Kürzel | Aktion |
|---|---|
| `Ctrl+T` | Zur heutigen Ansicht springen |
| `Ctrl+←` | Vorherige Periode (Tag / Woche / Monat je nach Ansicht) |
| `Ctrl+→` | Nächste Periode |
| `Ctrl+1` | Tagesansicht |
| `Ctrl+2` | Wochenansicht |
| `Ctrl+3` | Monatsansicht |
| `Ctrl+4` | Jahresansicht |
| `Ctrl+5` | Agenda-Ansicht |
| `Ctrl+6` | Aufgaben-Ansicht |
| `F6` | Zwischen Hauptbereichen wechseln (Sidebar ↔ Kalender ↔ Toolbar) |

#### Termine

| Kürzel | Aktion |
|---|---|
| `Ctrl+N` | Termin schnell anlegen (Quick-Add-Dialog) |
| `Ctrl+Shift+N` | Neuer Termin (vollständiges Formular) |
| `Enter` / `Space` | Fokussierten Termin öffnen |
| `Ctrl+E` | Fokussierten Termin bearbeiten |
| `Ctrl+D` | Fokussierten Termin duplizieren |
| `Delete` / `Backspace` | Fokussierten Termin löschen (mit Bestätigungs-Dialog) |
| `Shift+M` | Fokussierten Termin in anderen Kalender verschieben/kopieren |
| `Escape` | Dialog schließen / Aktion abbrechen |

#### App-Funktionen

| Kürzel | Aktion |
|---|---|
| `Ctrl+F` | Suche öffnen |
| `Ctrl+R` | Daten manuell synchronisieren (Kalender, Aufgaben, Kontakte) |
| `Ctrl+Shift+R` | Erinnerungs-Übersicht öffnen |
| `Ctrl+,` | Einstellungen öffnen |
| `Ctrl+H` | Tastaturkürzel-Übersicht anzeigen (`Ctrl+/` auf macOS, da `Cmd+H` Fenster versteckt) |
| `Ctrl+Q` | App beenden |

#### Aufgaben

| Kürzel | Aktion |
|---|---|
| `Alt+N` | Aufgabe schnell anlegen (Quick-Add-Dialog) |
| `Alt+Shift+N` | Neue Aufgabe (vollständiges Formular) |
| `Space` | Fokussierte Aufgabe als erledigt markieren / rückgängig |
| `Shift+D` | Datum für fokussierte Aufgabe setzen |
| `Shift+M` | Aufgabe in andere Aufgabenliste verschieben/kopieren |
| `Ctrl+D` | Aufgabe in dieselbe Liste duplizieren |

#### Plattform-Anpassungen

Unter macOS wird `Ctrl` durch `Cmd` ersetzt, entsprechend der macOS-Konvention (`Cmd+N`, `Cmd+F` etc.). Dies wird automatisch anhand der Plattform zur Laufzeit gesetzt.

### 15.8 Tastaturkürzel-Overlay

Per `Ctrl+H` (bzw. `Ctrl+/` auf macOS – `Cmd+H` ist auf macOS systemseitig für "Fenster verstecken" reserviert und kann nicht überschrieben werden) öffnet sich ein barrierefreies Overlay mit der vollständigen Kürzel-Referenz, durchsuchbar und gruppiert nach Kategorie – analog zu Overlays in Apps wie VS Code oder Figma. Das Overlay zeigt die aktuell wirksamen Belegungen, also inklusive aller individuellen Anpassungen aus Abschnitt 15.10. Eine Schaltfläche "Kürzel anpassen" öffnet direkt den Einstellungs-Dialog.

### 15.9 Plattformkonsistenz

Die App passt sich an plattformspezifische Konventionen an:

| Aspekt | Windows / Linux | macOS |
|---|---|---|
| Modifier-Taste | `Ctrl` | `Cmd` |
| Fenster-Schaltflächen | Rechts (Min / Max / Close) | Links (Close / Min / Max) |
| Titelleisten-Stil | Flach, Windows-typisch | Integriert in macOS-Stil |
| Schriftglättung | ClearType / keine | Subpixel-Antialiasing |
| Systemschrift als Basis | Segoe UI (Windows), System-UI (Linux) | SF Pro (macOS) |

### 15.10 Tastaturkürzel anpassen

Alle Tastaturkürzel sind individuell umbelegbar – essentiell für Nutzer, deren Screen Reader oder Hilfstechnologie bestimmte Tastenkombinationen abfängt, oder die schlicht eigene Belegungen bevorzugen.

#### Dialog

Erreichbar über `Einstellungen → Tastaturkürzel` oder per `Ctrl+,` → Reiter "Tastaturkürzel":

```
┌─────────────────────────────────────────────────────────────┐
│  Tastaturkürzel                                             │
│                                                             │
│  Suche: [______________________________]                    │
│                                                             │
│  ▼ Navigation                                               │
│    Tagesansicht                          [Ctrl+1]     [⟳]   │
│    Wochenansicht                         [Ctrl+2]     [⟳]   │
│    Heutige Ansicht                       [Ctrl+T]     [⟳]   │
│    ...                                                      │
│                                                             │
│  ▼ Termine                                                  │
│    Termin schnell anlegen                [Ctrl+N]     [⟳]   │
│    Neuer Termin                          [Ctrl+Shift+N][⟳]  │
│    ...                                                      │
│                                                             │
│  ▼ Aufgaben                                                 │
│    Aufgabe schnell anlegen               [Alt+N]      [⟳]   │
│    Neue Aufgabe                          [Alt+Shift+N][⟳]   │
│    ...                                                      │
│                                                             │
│  [Alle zurücksetzen]   [Schließen]                          │
└─────────────────────────────────────────────────────────────┘
```

Klick (oder `Enter`/`Space` bei Tastaturfokus) auf eine Kürzel-Belegung öffnet einen Aufnahme-Dialog:

```
┌──────────────────────────────────────────────────┐
│  Neue Tastenkombination für "Neuer Termin"       │
│                                                  │
│  Drücke die gewünschte Tastenkombination …       │
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │  Ctrl + Alt + N                            │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│  [Übernehmen]   [Auf Standard zurücksetzen]      │
│  [Abbrechen]                                     │
└──────────────────────────────────────────────────┘
```

Das `[⟳]`-Symbol in der Liste setzt eine einzelne Belegung auf den Standardwert zurück.

#### Konflikt-Erkennung

Wenn die neu gedrückte Kombination bereits einer anderen Aktion zugewiesen ist, erscheint sofort ein Hinweis:

```
┌──────────────────────────────────────────────────┐
│  Konflikt                                        │
│                                                  │
│  Ctrl + F ist bereits belegt mit:                │
│  "Suche öffnen"                                  │
│                                                  │
│  Möchtest du die bisherige Belegung ersetzen?    │
│  Die alte Aktion hat dann kein Kürzel mehr.      │
│                                                  │
│  [Ersetzen]   [Anderes Kürzel wählen]            │
│  [Abbrechen]                                     │
└──────────────────────────────────────────────────┘
```

Bestimmte Kombinationen können vom Nutzer nicht neu belegt werden und werden im Aufnahme-Dialog abgelehnt:

- Einzelne Buchstaben/Zahlen ohne Modifier (`A`, `1`, …) – Konflikt mit Texteingabe wahrscheinlich
- System-reservierte Kombinationen je Plattform (`Alt+F4` unter Windows, `Cmd+Tab` auf macOS, `Cmd+H` auf macOS für Fenster verstecken)
- `Tab`, `Shift+Tab` (Fokus-Navigation, darf nicht überschrieben werden)
- `Escape` allein (universelle Abbrechen-Taste)

> **Hinweis zur Standardbelegung:** Wenige App-Standardkürzel verwenden bewusst einzelne Tasten ohne Modifier – `N` für die Schnellerstellung, `Space` zum Öffnen fokussierter Termine bzw. zum Erledigen/Rückgängigmachen fokussierter Aufgaben. Diese Kürzel sind als Standardwerte sicher, weil sie nur in nicht-editierbaren Bereichen wirken (in Eingabefeldern werden sie nicht abgefangen). Der Nutzer kann diese Standardbelegungen **entfernen** oder durch eine andere zulässige Kombination ersetzen – sie aber nicht selbst neu auf einzelne Tasten setzen.

#### Datenmodell

```rust
pub struct ShortcutOverride {
    pub action_id: String,         // z.B. "event.new", "view.day"
    pub keys: Option<KeyCombo>,    // None = bewusst entfernt (kein Kürzel)
    pub modified_at: DateTime<Utc>,
}

pub struct KeyCombo {
    pub modifiers: Modifiers,      // Ctrl, Shift, Alt, Meta
    pub key: String,               // Plattform-neutrale Tastenbezeichnung ("KeyN", "Digit1")
}
```

Aktionen werden über stabile **action_ids** identifiziert (z.B. `event.new`, `task.new`, `view.day`), nicht über die Anzeigenamen – damit Lokalisierung und spätere Umbenennungen die Belegungen nicht brechen.

Die App führt intern eine Standardbelegungs-Tabelle. Der `ShortcutOverride`-Eintrag pro `action_id` überschreibt den Standardwert. Nicht überschriebene Aktionen verwenden die Standardbelegung der jeweiligen Plattform.

#### Synchronisation

Angepasste Tastaturkürzel werden über das Event Log geräteübergreifend synchronisiert. Sie gehören zur Kategorie der **geräteübergreifend sinnvollen Einstellungen** (Abschnitt 19.2.1) – ein Nutzer, der zwei Geräte verwendet, möchte typischerweise auf beiden dieselben Kürzel haben.

Plattformspezifische Belegungen werden korrekt umgesetzt: Wenn ein Nutzer auf Windows `Ctrl+Alt+N` für "Neuer Termin" festlegt, übernimmt sein macOS-Gerät automatisch `Cmd+Alt+N` (Modifier-Substitution). Konflikte mit plattformspezifischen System-Tastenkombinationen werden beim Empfangen geprüft – wenn ein synchronisiertes Kürzel auf der Empfangsplattform nicht zulässig ist (z.B. `Cmd+H` auf macOS), bleibt dort die Standardbelegung erhalten und der Nutzer wird per Hinweis informiert.

Im Event Log:

| Ereignis | Beschreibung |
|---|---|
| `shortcut.set` | Tastaturkürzel für eine `action_id` gesetzt oder geändert |
| `shortcut.reset` | Tastaturkürzel auf Standard zurückgesetzt |
| `shortcut.cleared` | Tastaturkürzel bewusst ohne Ersatz entfernt |

#### Barrierefreiheit

- Aufnahme-Dialog hat `role="dialog"` mit `aria-modal="true"`
- Fokus springt beim Öffnen direkt in das Aufnahmefeld
- Aktuelle Belegung wird per `aria-live="polite"` angekündigt, sobald Tasten gedrückt werden
- Konflikt-Warnung wird per `aria-live="assertive"` angekündigt
- Liste der Kürzel ist als `role="list"` strukturiert, jede Zeile als `role="listitem"`
- Schaltfläche `[⟳]` hat ein klares `aria-label` ("Auf Standard zurücksetzen: Neuer Termin")

---

## 16. Lokalisierung (i18n)

### 16.1 Architektur

- **i18next** als i18n-Framework im Frontend
- Übersetzungsdateien als JSON unter `src/locales/{lang}/`
- Datumsformatierung über `date-fns` mit Locale-Unterstützung
- Zahlen- und Zeitformate folgen den Systemeinstellungen (`Intl`-API)

### 16.2 Startsprachen

| Code | Sprache |
|---|---|
| `de` | Deutsch (Standard) |
| `en` | Englisch |

Weitere Sprachen können durch Hinzufügen einer JSON-Datei ergänzt werden. Community-Übersetzungen via Crowdin oder ähnliches geplant.

### 16.3 Datumsformate & Kalenderwochen

- Datumsformate richten sich nach der gewählten Sprache/Locale
- KW-Berechnung nach **ISO 8601** (Wochenbeginn Montag, erste Woche mit mind. 4 Tagen im Jahr)
- Anzeige konfigurierbar: `KW 20` (DE) / `Week 20` (EN)

---

## 17. Systemintegration

### 17.1 Standard-App für `.ics`-Dateien und Plugin-Dateien

Die App registriert sich als Standard-Anwendung für zwei Dateitypen:

**`.ics`-Dateien (iCalendar):** Doppelklick öffnet einen Import-Dialog (siehe unten).

**`.aperio`-Dateien (App-Plugins):** Doppelklick startet den Plugin-Installations-Ablauf (Abschnitt 20.7) direkt, ohne dass der Nutzer die Datei manuell in die Einstellungen ziehen muss.

#### Registrierung je Plattform

| Plattform | `.ics` | `.aperio` |
|---|---|---|
| **Windows** | `HKEY_CURRENT_USER\Software\Classes\.ics` | `HKEY_CURRENT_USER\Software\Classes\.aperio` |
| **macOS** | `CFBundleDocumentTypes` für `public.calendar` | `CFBundleDocumentTypes` für eigenen UTI `com.aperio.app.plugin` |
| **Linux** | `MimeType=text/calendar;` in `.desktop`-Datei | `MimeType=application/x-aperio-plugin;` in `.desktop`-Datei |

Alle Registrierungen erfolgen benutzerspezifisch (kein Admin-Recht nötig) und werden beim ersten Start per Dialog angeboten.

#### Import-Verhalten

Wenn die App eine `.ics`-Datei öffnet:

1. Datei wird geparst (`cal-adapter-ical`)
2. Enthaltene Termine werden in einer **Vorschau** angezeigt (Titel, Datum, Beschreibung)
3. Nutzer wählt den **Ziel-Kalender** aus einem Dropdown
4. Bestätigung → Termine werden importiert und synchronisiert
5. Bei mehreren Terminen: Option "Alle importieren" oder individuelle Auswahl per Checkbox-Liste

### 17.2 URL-Schema-Handler (`webcal://` und `calendar://`)

Browser und Webseiten verwenden das `webcal://`-Schema, um Kalender-Abonnements oder einzelne Termine direkt in eine Kalender-App zu übergeben. Die App registriert sich als Handler für beide Schemata.

#### Unterstützte Schemata

| Schema | Verwendung |
|---|---|
| `webcal://` | Abonnement eines externen Kalenders (iCal-Feed-URL) oder einzelner Termin |
| `calendar://` | Plattformspezifisch (macOS); Öffnen/Erstellen eines Termins |

#### Registrierung je Plattform

| Plattform | Mechanismus |
|---|---|
| **Windows** | Registry: `HKEY_CURRENT_USER\Software\Classes\webcal` mit `shell\open\command` → Pfad zur Binary + `"%1"` |
| **macOS** | `Info.plist`: `CFBundleURLTypes` mit `webcal` und `calendar` als `CFBundleURLSchemes` |
| **Linux** | `.desktop`-Datei: `MimeType=x-scheme-handler/webcal;`; Registrierung per `xdg-mime default Aperio.desktop x-scheme-handler/webcal` |

#### Verarbeitungslogik

**Kalender-Abonnement** (iCal-Feed):
```
webcal://example.com/calendar.ics
        │
        ▼
1. URL wird als iCal-Feed erkannt (enthält mehrere VEVENTs oder VCALENDAR)
2. Vorschau-Dialog: "Kalender abonnieren?"
   - Name des Feeds (aus X-WR-CALNAME falls vorhanden, sonst Hostname)
   - Aktualisierungsintervall konfigurierbar (stündlich / täglich / manuell)
3. Bestätigung → Feed wird als schreibgeschützter Kalender hinzugefügt
```

**Einzelner Termin** (z.B. Klick auf "Zum Kalender hinzufügen"-Link einer Webseite):
```
1. URL wird geparst → einzelnes VEVENT erkannt
2. Quick-Add-Dialog öffnet sich mit vorausgefüllten Feldern
3. Nutzer wählt Ziel-Kalender und bestätigt
```

#### Verhalten wenn App nicht läuft

- **Windows / Linux:** Binary wird mit der URL als Kommandozeilenargument gestartet (`Aperio "webcal://..."`), verarbeitet die URL und öffnet den entsprechenden Dialog
- **macOS:** Das System startet die App automatisch über den `NSWorkspace`-URL-Handler-Mechanismus

### 17.3 Registrierungs-Assistent (Erststart)

Beim ersten Start erscheint ein barrierefreier Einrichtungsdialog, der alle Systemintegrationen gebündelt anbietet:

```
┌──────────────────────────────────────────────────────┐
│  Systemintegration einrichten                        │
│                                                      │
│  [x] Als Standard-App für .ics-Dateien festlegen     │
│  [x] Als Handler für webcal://-Links registrieren    │
│  [x] Als Standard-App für .aperio-Dateien festlegen  │
│  [ ] Desktop-Verknüpfung erstellen (optional)        │
│                                                      │
│  Diese Einstellungen können jederzeit unter          │
│  Einstellungen → Systemintegration geändert werden.  │
│                                                      │
│  [Übernehmen]                    [Überspringen]      │
└──────────────────────────────────────────────────────┘
```

- Alle Checkboxen per Tastatur bedienbar (`Space` zum Umschalten)
- Dialog ist vollständig mit Screen Readern nutzbar (`role="dialog"`, Fokus-Falle)
- Einstellungen nachträglich unter `Einstellungen → Systemintegration` änderbar, inkl. Rückgängig-Machen aller Registrierungen

### 17.4 Autostart (Start bei der Anmeldung)

Aperio kann sich so registrieren, dass es **automatisch beim Anmelden** an diesem Computer startet. Da Aperio Erinnerungen als native Systembenachrichtigungen zustellt (Abschnitt 14.3), sorgt Autostart dafür, dass der Erinnerungs-Scheduler nach jedem Neustart wieder läuft, ohne dass die App von Hand geöffnet werden muss.

Umgesetzt über `tauri-plugin-autostart`. Die Betriebssystem-Registrierung ist die **alleinige Quelle der Wahrheit** – es gibt keine separate, synchronisierte Einstellung; der Schalter liest und schreibt direkt den OS-Zustand und spiegelt ihn daher auch dann korrekt, wenn der Eintrag außerhalb der App geändert wird.

| Plattform | Mechanismus |
|---|---|
| **Windows** | Registry-Wert unter `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run` |
| **macOS** | LaunchAgent (`~/Library/LaunchAgents`) |
| **Linux** | `.desktop`-Autostart-Datei unter `~/.config/autostart` |

- **Geräte-lokal:** Autostart gilt nur für das Gerät, auf dem er aktiviert wird – wie die Tray- und Darstellungs-Einstellungen wird er **nicht** über das Event Log synchronisiert.
- **Bedienung:** Zwei Häkchen unter `Einstellungen → Allgemein → Systemstart`:
  1. **„Aperio bei der Anmeldung starten"** – schaltet die OS-Registrierung an/aus (Abhaken entfernt den Eintrag wieder vollständig).
  2. **„Minimiert im Infobereich starten"** – nur sichtbar, solange Autostart an ist; steuert das Startverhalten (siehe unten). Geräte-lokale Einstellung `window.autostartMinimized`, Standard **an**; ohne nutzbaren Infobereich deaktiviert.
  Beide vollständig per Tastatur bedienbar und mit Screen Readern nutzbar; ist der Start bei der Anmeldung in der Laufzeitumgebung nicht verfügbar, bleibt der erste Schalter deaktiviert mit erklärendem Hinweis.
- **Startverhalten:** Der beim Autostart registrierte Aufruf trägt das Argument `--autostart` (markiert nur die Startquelle). Beim Start entscheidet die App: Ist es ein Autostart-Aufruf, existiert ein Infobereich (Abschnitt 15.3) **und** ist die Pref „minimiert starten" an, startet Aperio direkt **minimiert im Infobereich** (das Fenster wird mit `visible: false` erzeugt und gar nicht erst eingeblendet – kein Aufblitzen); ein Klick auf das Infobereich-Symbol holt es hervor. Sonst – kein Infobereich, Pref aus, oder manueller Start (ohne `--autostart`) – wird das Fenster normal angezeigt, sodass die App nie unsichtbar-und-unerreichbar startet.
- Registrierung erfolgt benutzerspezifisch (kein Admin-Recht nötig).

Optional lässt sich Autostart auch im Erststart-Assistenten (17.3) als weitere Checkbox anbieten:

```
[ ] Aperio bei der Anmeldung automatisch starten
```

---

## 18. Offline-Fähigkeit & Datensynchronisation

### 18.1 Lokale Datenhaltung

Alle App-Daten (Termine, Aufgaben, Aufgabenlisten, Kontakt-Cache, Kalender-/Listen-Metadaten, Einstellungen) werden in einer lokalen **SQLite-Datenbank** gespeichert. SQLite dient als schneller lokaler Lese-/Schreib-Cache; die geräteübergreifende Synchronisation zwischen mehreren Instanzen der App erfolgt über das in Abschnitt 19 beschriebene Append-Only Event Log – SQLite ist dabei nicht das Sync-Artefakt.

Der Datenbankpfad folgt der portablen Datenpfad-Logik aus Abschnitt 22.2:

```
Portabel:   ./data/db.sqlite                                    (neben der Binary)
Fallback:   %APPDATA%\Aperio\db.sqlite                      (Windows)
            ~/Library/Application Support/Aperio/db.sqlite  (macOS)
            ~/.config/Aperio/db.sqlite                      (Linux)
```

### 18.2 Sync-Architektur: Externe Kalender-, Aufgaben- und Kontakt-APIs

Jede App-Instanz kommuniziert **direkt** mit den externen APIs – es gibt kein koordinierendes "Primary Device". Das ist der natürliche Ansatz, da die APIs (Google Calendar, CalDAV, Microsoft Graph, Vikunja, Todoist etc.) von Natur aus Multi-Writer-Systeme sind und externe Änderungen (z.B. durch Sprachassistenten wie Alexa, Web-Interfaces oder andere Apps) jederzeit auftreten können.

```
Google / CalDAV / EWS / Vikunja / Todoist / ...
        ▲              ▲              ▲
        │              │              │  (jede Instanz direkt)
     Gerät A        Gerät B        Gerät C
  (Lokale SQLite) (Lokale SQLite) (Lokale SQLite)
```

#### Änderungs-Tokens & ETags

Jede API liefert Mechanismen, um nur tatsächlich geänderte Daten abzurufen und Schreibkonflikte zu erkennen:

| API | Pull-Mechanismus | Push-Konfliktschutz |
|---|---|---|
| **Google Calendar / Tasks / People** | `syncToken` – nur Änderungen seit letztem Sync | `ETag` – `412 Precondition Failed` bei Konflikt |
| **Microsoft Graph (Calendar, To Do, Contacts)** | `deltaToken` – nur Änderungen seit letztem Sync | `ETag` – `412 Precondition Failed` bei Konflikt |
| **CalDAV / CardDAV / iCloud** | `CTag` (Kollektion) + `ETag` (Eintrag) | `ETag` – `412 Precondition Failed` bei Konflikt |
| **EWS (Calendar, Tasks, Contacts)** | `SyncFolderItems` mit `SyncState` | Timestamp-basiert |
| **Vikunja** | Polling mit `updated`-Timestamp pro Liste | Timestamp-basiert; bei Konflikt: erneutes Lesen + Merge |
| **Todoist** | `sync_token` (Todoist Sync API) | Timestamp-basiert; bei Konflikt: erneutes Lesen + Merge |

Sync-Tokens und ETags werden pro Gerät lokal in SQLite gespeichert und **nicht** über das Event Log zwischen Geräten ausgetauscht – jedes Gerät führt seinen eigenen Sync-Zustand mit der API.

#### Konfliktauflösung beim Push

Wenn zwei Geräte dasselbe Objekt (Termin, Aufgabe oder Kontakt) gleichzeitig ändern und zur API pushen wollen:

```
1. Gerät A pusht Änderung → API akzeptiert, vergibt neue ETag
2. Gerät B pusht Änderung mit veralteter ETag
   → API antwortet: 412 Precondition Failed
3. Gerät B lädt aktuelle Version von der API
4. Automatische Zusammenführung (unterschiedliche Felder)
   oder Nutzer-Dialog (gleiches Feld) – siehe Abschnitt 19.3
5. Gerät B pusht erneut mit aktueller ETag → Erfolg
```

#### Offline-Puffer

Lokale Änderungen bei fehlender Verbindung werden in der Sync-Queue (SQLite) gepuffert und beim nächsten Verbindungsaufbau mit ETag-Prüfung zur API übertragen.

#### Verhältnis zum Event Log (Abschnitt 19)

Das Event Log synchronisiert ausschließlich Daten, die **außerhalb der externen APIs** liegen:

| Datentyp | Sync via API | Sync via Event Log |
|---|---|---|
| Termine aus externen Kalendern | ✅ Direkt via API | ❌ |
| Aufgaben aus externen Aufgabenlisten | ✅ Direkt via API | ❌ |
| Kontakte aus externen Quellen | ✅ Direkt via API | ❌ |
| Lokale Kalender und ihre Termine | ❌ | ✅ |
| Lokale Aufgabenlisten und ihre Aufgaben | ❌ | ✅ |
| Lokale Einstellungen | ❌ | ✅ |
| Sound-Konfiguration & Audiodateien | ❌ | ✅ |
| Kalender- und Aufgabenlisten-Farben (lokal überschrieben) | ❌ | ✅ |
| Farb-Labels | ❌ | ✅ |
| Plugin-Installationen (Metadaten) | ❌ | ✅ |
| Tastaturkürzel-Belegungen | ❌ | ✅ |
| Offline-gepufferte API-Änderungen | ❌ (bis Verbindung) | ❌ (bleiben lokal bis Push) |

### 18.3 Verbindungsstatus-Anzeige

- Aktueller Sync-Status ist jederzeit sichtbar und per Screen Reader abrufbar
- `aria-live`-Ankündigung bei Verbindungsverlust und Wiederverbindung

---

## 19. Geräteübergreifende Datenbanksynchronisation

### 19.1 Warum nicht SQLite als Sync-Artefakt

SQLite als einzelne Binärdatei ist für echte Parallelnutzung von mehreren Systemen **nicht geeignet**: Laden zwei Geräte dieselbe Datei herunter, schreiben lokal und laden hoch, überschreibt eine Version unweigerlich die andere. Ein dateibasiertes Locking-Protokoll ist über Cloud-Speicher nicht zuverlässig realisierbar.

SQLite bleibt jedoch als **lokaler Lese-/Schreib-Cache** erhalten – es ist weiterhin die schnelle lokale Datenquelle für die App. Es ist nur nicht mehr das Artefakt, das zwischen Geräten übertragen wird.

### 19.2 Architektur: Append-Only Event Log

Die Synchronisation basiert auf einem **Append-Only Event Log**: Jede Änderung (Termin erstellt, Termin bearbeitet, Aufgabe erledigt etc.) wird als unveränderliches, kleines JSON-Ereignis an eine fortlaufende Log-Datei angehängt. Kein Gerät überschreibt je eine bestehende Zeile – es schreibt nur neue ans Ende.

```
sync/
├── log/
│   ├── 2025-05-12T09-14-22Z_device-a.jsonl   # Ereignisse von Gerät A
│   ├── 2025-05-12T11-03-41Z_device-b.jsonl   # Ereignisse von Gerät B
│   └── ...
└── meta.json                                  # Geräteliste, Sync-Versionen
```

Jedes Gerät hat eine eindeutige `device_id` (UUID, beim ersten Start generiert). Log-Dateien sind pro Gerät und Zeitstempel benannt – dadurch können zwei Geräte niemals dieselbe Datei gleichzeitig beschreiben.

#### Ereignisformat

```json
{
  "id": "evt_01jf3k...",
  "device_id": "device-a",
  "timestamp": "2025-05-12T09:14:22.341Z",
  "type": "event.updated",
  "payload": {
    "event_id": "cal_evt_abc123",
    "fields": {
      "title": "Teammeeting",
      "start": "2025-05-15T10:00:00Z"
    }
  }
}
```

#### Ereignistypen

| Typ | Beschreibung |
|---|---|
| `event.created` | Neuer Termin |
| `event.updated` | Termin bearbeitet (nur geänderte Felder) |
| `event.deleted` | Termin gelöscht |
| `task.created` | Neue Aufgabe |
| `task.updated` | Aufgabe bearbeitet |
| `task.deleted` | Aufgabe gelöscht |
| `task_list.created` | Neue Aufgabenliste angelegt |
| `task_list.updated` | Aufgabenliste umbenannt, eingefärbt etc. |
| `task_list.deleted` | Aufgabenliste entfernt |
| `calendar.created` | Neuer Kalender hinzugefügt |
| `calendar.updated` | Kalender-Einstellungen geändert (Farbe, Name etc.) |
| `calendar.deleted` | Kalender entfernt |
| `color_label.created` | Neues Farb-Label angelegt |
| `color_label.updated` | Farb-Label bearbeitet (Name, Farbe) |
| `color_label.deleted` | Farb-Label gelöscht |
| `plugin.installed` | Community-Plugin auf einem Gerät installiert (nur Metadaten, keine Binary) |
| `plugin.updated` | Community-Plugin aktualisiert |
| `plugin.uninstalled` | Community-Plugin entfernt (andere Geräte erhalten Hinweis und entfernen die "Plugin fehlt"-Markierung) |
| `shortcut.set` | Tastaturkürzel für eine Aktion gesetzt oder geändert (siehe 15.10) |
| `shortcut.reset` | Tastaturkürzel auf Standardbelegung zurückgesetzt |
| `shortcut.cleared` | Tastaturkürzel bewusst ohne Ersatz entfernt |
| `settings.updated` | App-Einstellungen geändert (nur synchronisierbare Einstellungen, siehe 19.2.1) |
| `credential.set` | Account-Zugangsdaten gesetzt/geändert — **nur bei aktivem E2E** (§19.7); trägt das Secret und existiert daher ausschließlich im verschlüsselten Log |
| `credential.cleared` | Ein Zugangsdaten-Slot eines Accounts entfernt (gleiche E2E-Kopplung wie `credential.set`) |

#### 19.2.1 Einstellungs-Synchronisation: Granularität

Nicht alle Einstellungen sind geräteübergreifend sinnvoll. Sie werden in drei Kategorien unterteilt:

| Einstellung | Standardverhalten | Konfigurierbar? |
|---|---|---|
| **Immer synchronisiert** | | |
| Kalenderfarben | ✅ Sync | ❌ Nein |
| Bevorzugte Ansicht (Tag/Woche/Monat etc.) | ✅ Sync | ✅ Ja |
| Wochenanfang (Mo/So) | ✅ Sync | ✅ Ja |
| KW-Anzeige (Monatsansicht, optional) | ✅ Sync | ✅ Ja |
| Sprache / Locale | ✅ Sync | ✅ Ja |
| Sound-Konfiguration (Container-, Termin- & Aufgaben-Ebene) | ✅ Sync | ✅ Ja |
| Benutzerdefinierte Sound-Dateien (Audiodaten) | ✅ Sync | ✅ Ja |
| Standard-Erinnerungszeiten (Termine & Aufgaben getrennt) | ✅ Sync | ✅ Ja |
| Erledigte Aufgaben anzeigen | ✅ Sync | ✅ Ja |
| **Nie synchronisiert (immer gerätespezifisch)** | | |
| Fenstergröße & -position | ❌ Lokal | ❌ Nein |
| Systemintegration (.ics, webcal://) | ❌ Lokal | ❌ Nein |
| Geräte-ID | ❌ Lokal | ❌ Nein |
| Adapter-Zugangsdaten (Sync-, Daten-, Videokonferenz-Adapter) | Lokal (Keychain); **bei aktivem E2E** zusätzlich Ende-zu-Ende-verschlüsselt synchronisiert (§19.7) | ❌ Fest an E2E gekoppelt |
| **Konfigurierbar durch Nutzer** | | |
| Sync-Adapter-Auswahl (welcher Adapter aktiv) | Standard: ✅ Sync | ✅ Ja |
| Tastaturkürzel-Belegung | Standard: ✅ Sync (via `shortcut.*`-Ereignisse) | ✅ Ja |
| Dark Mode / Farbschema | Standard: ✅ Sync | ✅ Ja |
| Snooze-Optionen | Standard: ✅ Sync | ✅ Ja |

Konfigurierbare Einstellungen können unter `Einstellungen → Synchronisation → Einstellungen synchronisieren` individuell ein- oder ausgeschaltet werden. Das ermöglicht z.B. unterschiedliche Spracheinstellungen pro Gerät bei sonst vollständiger Synchronisation.

Das `settings.updated`-Ereignis trägt immer den Einstellungsschlüssel und den neuen Wert – niemals die gesamte Einstellungsdatei – damit feldweise Zusammenführung (Abschnitt 19.3) auch für Einstellungen funktioniert.

> **Hinweis zur Tabelle:** Die Tabelle deklariert, **ob** etwas synchronisiert wird – nicht über welchen Ereignistyp. Tastaturkürzel-Belegungen werden zwar wie Einstellungen behandelt (inklusive Toggle), nutzen aber eigene Ereignistypen (`shortcut.set`, `shortcut.reset`, `shortcut.cleared`) statt `settings.updated`. Farb-Labels und Plugin-Metadaten sind hingegen **keine Einstellungen** und tauchen in dieser Tabelle nicht auf – sie werden über `color_label.*` bzw. `plugin.*` synchronisiert (siehe Ereignistypen in Abschnitt 19.2).

#### 19.2.2 Synchronisation benutzerdefinierter Sound-Dateien

Die Sound-Konfiguration (welche Datei für welchen Container bzw. welchen Termin/welche Aufgabe) ist nur dann auf anderen Geräten nutzbar, wenn die referenzierten Audiodateien dort ebenfalls vorhanden sind. Deshalb werden benutzerdefinierte Sound-Dateien **zusammen mit der Konfiguration synchronisiert**.

**Ablagestruktur im Sync-Speicher:**

```
sync/
├── log/                        # Ereignis-Logs (wie bisher)
├── assets/
│   └── sounds/
│       ├── <sha256-hash>.mp3   # Datei, benannt nach ihrem Inhalt-Hash
│       ├── <sha256-hash>.wav
│       └── ...
└── meta.json
```

Audiodateien werden **inhaltsbasiert benannt** (SHA-256-Hash des Dateiinhalts). Das hat drei Vorteile:

- Keine Duplikate: Dieselbe Datei wird nur einmal hochgeladen, egal unter welchem Namen der Nutzer sie gespeichert hat
- Kein Überschreiben: Eine Datei mit demselben Hash existiert bereits → kein Upload nötig
- Integritätsprüfung: Beim Download wird der Hash verifiziert

Die Sound-Konfiguration referenziert Dateien über ihren Hash. Lokal legt die App die Datei unter dem ursprünglichen Nutzernamen ab; die Hash-Zuordnung wird in SQLite gespeichert.

**Upload-Verhalten:**

Beim Hinzufügen einer neuen Sound-Datei prüft die App, ob der Hash bereits im Sync-Speicher existiert (`assets/sounds/<hash>.mp3`). Nur wenn nicht, wird die Datei hochgeladen. Das `settings.updated`-Ereignis für die Sound-Konfiguration enthält den Hash, nicht den lokalen Dateipfad.

**Größenbeschränkung:**

Benutzerdefinierte Sound-Dateien sind auf **5 MB pro Datei** begrenzt, um den Sync-Speicher nicht unnötig zu belasten. Bei Überschreitung wird der Nutzer beim Hinzufügen der Datei informiert.

### 19.3 Konfliktauflösung

Da zwei Geräte dasselbe Objekt gleichzeitig ändern können, wird folgende Strategie angewendet:

**Automatische Zusammenführung** (wo möglich): Wenn zwei Geräte unterschiedliche Felder desselben Objekts (Termin, Aufgabe, Aufgabenliste, Kalender etc.) ändern – z.B. Gerät A ändert den Titel eines Termins, Gerät B ändert den Ort – werden beide Änderungen zusammengeführt; kein Konflikt.

**Nutzer-Entscheidung** (bei echtem Konflikt): Wenn zwei Geräte dasselbe Feld unterschiedlich ändern, wird ein Konflikt-Dialog angezeigt (Beispiel: Termin-Titel):

```
┌─────────────────────────────────────────────────────┐
│  Konflikt: "Teammeeting"                            │
│                                                     │
│  Dieses Gerät:      "Teammeeting Q2"                │
│  Anderes Gerät:     "Teammeeting Q3"                │
│  (vor 3 Minuten auf MacBook geändert)               │
│                                                     │
│  [Meine Version behalten]  [Andere Version nehmen]  │
│  [Beide als separate Termine speichern]             │
└─────────────────────────────────────────────────────┘
```

Konflikte werden per `aria-live` angekündigt und sind vollständig per Tastatur auflösbar.

### 19.4 Lokaler SQLite-Cache & Zustandsrekonstruktion

Beim Start liest die App alle noch nicht lokal angewandten Log-Ereignisse vom Sync-Speicher, wendet sie auf den lokalen SQLite-Cache an und hat damit den aktuellen Stand. Das Log ist die **Quelle der Wahrheit** (Source of Truth); SQLite ist der schnelle lokale Spiegel.

```
Sync-Speicher (Log-Dateien)
        │
        ▼ neue Ereignisse lesen & anwenden
Local SQLite Cache ──► App UI
        │
        ▼ neue lokale Änderungen als Ereignisse schreiben
Sync-Speicher (neues .jsonl hochladen)
```

Log-Kompaktierung: Alte Log-Dateien werden periodisch zu einem Snapshot zusammengefasst. Die vollständige Spezifikation der Kompaktierungs-Trigger und des Algorithmus findet sich in Abschnitt 19.10.

### 19.5 Sync-Adapter-Architektur

Analog zu den Daten-Adaptern (Abschnitt 6.1) wird jeder Sync-Adapter als **eigenständiges Crate** im Cargo-Workspace entwickelt. Die vollständige Workspace-Struktur inkl. aller Sync-Adapter-Crates ist in Abschnitt 6.1 und 23 dokumentiert.

```
crates/
├── sync-core/                  # Gemeinsames Trait & Ereignisformat
├── sync-adapter-webdav/        # WebDAV (verschlüsselt & unverschlüsselt)
├── sync-adapter-ftp/           # FTPS
├── sync-adapter-sftp/          # SFTP
├── sync-adapter-dropbox/       # Dropbox API v2
├── sync-adapter-googledrive/   # Google Drive API v3
└── sync-adapter-local/         # Lokales Dateisystem / NAS
```

#### Das `sync-core`-Crate

```rust
#[async_trait]
pub trait SyncAdapter: Send + Sync {
    /// Neue Log-Dateien vom Sync-Speicher herunterladen
    async fn fetch_new_logs(&self, since: &DeviceTimestamp) -> Result<Vec<LogFile>>;
    /// Lokale Log-Datei hochladen
    async fn push_log(&self, log: &LogFile) -> Result<()>;
    /// Snapshot hochladen (nach Kompaktierung)
    async fn push_snapshot(&self, snapshot: &Snapshot) -> Result<()>;
    /// Aktuellen Snapshot herunterladen
    async fn fetch_snapshot(&self) -> Result<Option<Snapshot>>;
    /// Verbindung testen
    async fn test_connection(&self) -> Result<()>;
}
```

### 19.6 Geplante Sync-Adapter (v1.0)

| Adapter | Protokoll | Auth | Verschlüsselung (Transport) | Implementierung |
|---|---|---|---|---|
| **WebDAV** | WebDAV (RFC 4918) | Basic / Digest / OAuth2 | TLS (HTTPS) optional | `reqwest` + `quick-xml` |
| **FTPS** | FTP über TLS | Benutzername / Passwort | TLS | `suppaftp` (pure Rust, mit `rustls`) |
| **SFTP** | SSH File Transfer Protocol | Passwort / SSH-Key | SSH (immer verschlüsselt) | `russh` + `russh-sftp` (pure Rust, mobile-tauglich) |
| **Dropbox** | Dropbox API v2 | OAuth2 | TLS (immer) | `reqwest` + Dropbox API |
| **Google Drive** | Google Drive API v3 | OAuth2 | TLS (immer) | `google-apis-rs` |
| **Lokales Dateisystem** | Dateisystem | Keine | Keine (Nutzer verantwortlich) | `std::fs` |

> **Plattform-Hinweis:** Alle Sync-Adapter sind als reine Rust-Crates implementiert und nutzen keine plattformspezifischen System-APIs für die Protokollabwicklung. Das bedeutet: Auch wenn iOS/Android FTP/SFTP nicht "nativ" unterstützen, funktionieren die Adapter auf Mobile-Plattformen genauso wie auf Desktop – wir implementieren das Wire-Protokoll selbst und brauchen nur TCP/TLS, was überall verfügbar ist. Für SFTP wurde bewusst `russh` (pure Rust) statt `ssh2` (libssh2-Bindings) gewählt, um die Cross-Compilation für Mobile-Targets zu vereinfachen.

### 19.7 Ende-zu-Ende-Verschlüsselung (optional)

Konfigurierbar pro Sync-Adapter: Der Nutzer kann ein Passwort (oder einen Schlüssel) festlegen, mit dem alle Log-Dateien und Snapshots **vor dem Hochladen** verschlüsselt werden. Der Sync-Speicher sieht nur verschlüsselte Blobs.

- Algorithmus: **AES-256-GCM** (authentifizierte Verschlüsselung)
- Schlüsselableitung: **Argon2id** aus dem Nutzer-Passwort
- Implementierung: `aes-gcm` + `argon2` Rust-Crates
- Der Schlüssel wird **niemals** auf dem Sync-Speicher abgelegt; er muss auf jedem Gerät separat eingegeben werden

> **Hinweis:** Bei aktivierter E2E-Verschlüsselung ist eine Schlüsselwiederherstellung ohne das Passwort nicht möglich. Ein Hinweis darauf wird bei der Einrichtung prominent angezeigt.

**Account-Zugangsdaten (nur bei E2E).** Adapter-Zugangsdaten (CalDAV-/WebDAV-Passwörter, OAuth-Refresh-Tokens, API-Tokens) liegen normalerweise ausschließlich lokal im OS-Keyring (§6.6). **Ist E2E aktiv, werden sie zusätzlich Ende-zu-Ende-verschlüsselt synchronisiert** — als `credential.set` / `credential.cleared`-Ereignisse im (dann verschlüsselten) Event-Log und im `credentials`-Block des (verschlüsselten) Snapshots —, damit Accounts auf allen Geräten ohne erneute Eingabe funktionieren. Die Kopplung ist **fest**: kein E2E → kein Credential-Sync; bei E2E-aus bleiben Zugangsdaten strikt lokal.

- **Single Chokepoint:** Genau eine Stelle (`credential_sync.rs`) verwandelt ein Secret in ein Ereignis, doppelt gegated (E2E muss an sein **und** der Slot muss syncbar sein).
- **Syncbare Slots:** nur `password`, `refresh_token`, `api_token`. Kurzlebige Access-Tokens (pro Gerät aus dem Refresh-Token neu ableitbar) und der E2E-Schlüssel selbst werden **nie** synchronisiert — sowohl beim Senden als auch beim Anwenden per Allowlist erzwungen.
- **Übergänge:** E2E nachträglich aktivieren → alle bestehenden lokalen Zugangsdaten werden in den jetzt verschlüsselten Log gepusht. E2E deaktivieren → der lokale E2E-Pref wird **zuerst** gelöscht (das stoppt weitere Credential-Emits, gated die Kompaktierung, und der für den Downgrade gebaute Adapter ist dadurch echt unverschlüsselt), dann werden die Credential-Ereignisse aus den Logs **und** der `credentials`-Block aus dem Snapshot entfernt (Strip/Purge); lokal bleiben die Zugangsdaten im Keyring erhalten.

**Robustheit der Übergänge.** Der Deaktivieren-Vorgang ist **idempotent**: Wird er unterbrochen (z. B. Netzwerkabbruch, nachdem ein Teil der Logs bereits als Klartext neu geschrieben wurde), kann er gefahrlos erneut ausgeführt werden — bereits als Klartext vorliegende Logs werden beim erneuten Lauf toleriert (AES-GCM authentifiziert; ein Klartext-Blob scheitert an der Entschlüsselung und wird unverändert übernommen) statt einen Entschlüsselungsfehler auszulösen, der den Datensatz halb-konvertiert blockieren würde. Gleichzeitige, **gegenläufige** Übergänge auf zwei Geräten (eines aktiviert, eines deaktiviert) werden nur über `meta.json` koordiniert (Last-Write-Wins); da der Sync-Speicher keine atomare Vergleichs-und-Setze-Operation bietet, ist dieser seltene Fall nicht vollständig ausschließbar — das unterlegene Gerät erkennt die Diskrepanz beim nächsten Abgleich und richtet sich neu ein (kein Datenverlust und kein Klartext-Leak von Credentials, da deren Emit fest an den lokalen E2E-Pref gekoppelt ist).

### 19.8 Sync-Auslöser

| Auslöser | Verhalten |
|---|---|
| **Bei jeder Änderung** | Neue Log-Einträge werden sofort hochgeladen (sofern Verbindung vorhanden); bei fehlender Verbindung gepuffert |
| **Konfigurierbares Intervall** | Vollständiger Abgleich (fetch + push) im einstellbaren Takt (Standard: 5 Minuten) |
| **Manuell** | Per `Ctrl+R` oder Button in der Statusleiste |
| **App-Start** | Automatischer Abgleich beim Start |
| **App-Beenden** | Ausstehende lokale Änderungen werden vor dem Beenden hochgeladen |

### 19.9 Statusanzeige & Barrierefreiheit

- Sync-Status dauerhaft in der Statusleiste sichtbar: `✓ Synchronisiert`, `↑ Wird hochgeladen…`, `⚠ Konflikt`, `✗ Keine Verbindung`
- Per `aria-live="polite"` werden Statusänderungen angekündigt
- Konflikte werden zusätzlich als Systembenachrichtigung gemeldet
- Detailliertes Sync-Protokoll unter `Einstellungen → Synchronisation → Protokoll` einsehbar

### 19.10 Snapshot-Generierung & Log-Kompaktierung

#### Was ist ein Snapshot?

Ein Snapshot ist eine vollständige Momentaufnahme des gesamten App-Zustands zu einem bestimmten Zeitpunkt – alle Termine, Aufgaben, Kalender- und Aufgabenlisten-Konfigurationen, Einstellungen und Sound-Referenzen, serialisiert als JSON (bzw. verschlüsselter Blob bei aktivierter E2E-Verschlüsselung). Er ersetzt konzeptionell alle Log-Ereignisse bis zu seinem Zeitstempel und dient neuen Geräten als Ausgangspunkt.

```
sync/
├── log/
│   ├── 2025-05-01T08-00-00Z_device-a.jsonl
│   ├── 2025-05-10T14-22-11Z_device-b.jsonl
│   └── 2025-05-12T09-14-22Z_device-a.jsonl
├── snapshot.json          # Aktueller Snapshot (ersetzt alle Logs vor snapshot_timestamp)
└── meta.json
```

#### `meta.json` – Geräteregister

`meta.json` ist die zentrale Koordinationsdatei. Sie ist bewusst **immer unverschlüsselt** (auch bei aktivierter E2E-Verschlüsselung), damit Versionsfelder und der E2E-Status ohne Passwort lesbar sind. Sie enthält:

```json
{
  "schema_version": 1,
  "min_app_version": "1.0.0",
  "e2e_enabled": false,
  "snapshot_timestamp": "2025-05-01T00:00:00Z",
  "gc_horizon": "2025-04-17T00:00:00Z",
  "devices": {
    "device-a": {
      "name": "Desktop-PC",
      "last_seen_log": "2025-05-12T09:14:22Z",
      "app_version": "1.2.0",
      "stale": false
    },
    "device-b": {
      "name": "MacBook",
      "last_seen_log": "2025-05-10T14:22:11Z",
      "app_version": "1.1.3",
      "stale": false
    }
  }
}
```

Nach jedem erfolgreichen Sync-Durchlauf aktualisiert ein Gerät seinen eigenen `last_seen_log`- und `app_version`-Eintrag. `last_seen_log` ist der **„held horizon"** des Geräts — `max(Fetch-Cursor, eigenes neuestes Log)`, also der Zeitpunkt, bis zu dem es jedes Ereignis hält (angewendete fremde + selbst geschriebene). `stale: true` wird gesetzt, wenn der `last_seen_log` eines Geräts unter den `gc_horizon` fällt — die Logs, die es zum inkrementellen Aufholen bräuchte, sind bereits gelöscht (Schritt 6 des Kompaktierungs-Algorithmus). `gc_horizon` ist die (monoton steigende) **GC-Obergrenze**: jede Log-Datei mit `timestamp < gc_horizon` wurde gelöscht und kann nicht mehr inkrementell nachgeladen werden — abwesend (`null`) bedeutet, dass noch nie ein Log gelöscht wurde (frischer ODER Alt-Datensatz), sodass die Stale-Prüfung niemandem auslöst. `min_app_version` wird bei einem Schema-Upgrade auf die aktuelle App-Version gesetzt (Abschnitt 19.13).

#### Kompaktierungs-Trigger

Kompaktierung wird ausgelöst, wenn **einer** der folgenden Schwellwerte überschritten wird (konfigurierbar):

| Schwellwert | Standard |
|---|---|
| Anzahl Log-Ereignisse seit letztem Snapshot | 1.000 Ereignisse |
| Zeit seit letztem Snapshot | 30 Tage |
| Gesamtgröße aller Log-Dateien | 50 MB |

Die Prüfung erfolgt bei jedem Sync-Durchlauf. Auslösendes Gerät ist dasjenige, das den Schwellwert zuerst erkennt.

#### Kompaktierungs-Algorithmus

```
1. Alle Log-Dateien seit dem letzten Snapshot herunterladen
2. Log-Ereignisse chronologisch sortieren und auf den letzten
   Snapshot-Zustand anwenden → neuer Gesamtzustand
3. Neuen Snapshot generieren und hochladen (snapshot.json),
   gestempelt mit snapshot_ts = max(eigenes neuestes Log,
   Fetch-Cursor) — dem tatsächlich gehaltenen Inhalt, nicht
   Utc::now(). Ein beitretendes Gerät übernimmt diesen Wert als
   Start-Cursor.
4. meta.json aktualisieren: snapshot_timestamp = snapshot_ts.

5. GC-Schnitt berechnen:
   safe_cutoff = max(niedrigster held_horizon aller Geräte,
                     snapshot_ts − Aufbewahrungsfenster)   [Default 14 Tage]
   → Der erste Term (konservativ) löscht NIE ein Log, das ein noch
     bekanntes Gerät — auch ein parallel kompaktierendes mit
     niedrigerem Horizont — noch nicht hat (Datenverlust-Schutz bei
     gleichzeitiger Kompaktierung ohne ETag-Sperre).
   → Der zweite Term begrenzt, wie weit EIN zurückliegendes Gerät den
     Schnitt zurückhält: ein länger als das Fenster offline gewesenes
     Gerät wird aufgegeben (seine alten Logs werden gelöscht, es
     übernimmt bei Rückkehr den Snapshot) — so wachsen die Logs nicht
     unbegrenzt hinter einem dauerhaft offline Gerät (der ursprüngliche
     Report).
   gc_horizon = max(bisheriger gc_horizon, safe_cutoff) — MONOTON
   (Löschungen sind endgültig), in meta.json veröffentlicht.

6. Geräte, deren held_horizon (last_seen_log) < gc_horizon ist,
   als "stale" markieren: die Logs zum inkrementellen Aufholen sind
   gelöscht. Ein Gerät nur HINTER dem Snapshot, aber ≥ gc_horizon,
   bleibt unmarkiert (es holt über die aufbewahrten Logs auf).

7. Jede Log-Datei mit Zeitstempel STRIKT < safe_cutoff löschen, die
   der Snapshot abdeckt (eigene Logs immer; fremde nur bis zum Cursor —
   ein fremdes Log neuer als der Cursor wurde nie angewendet und steckt
   nicht im Snapshot).
```

#### Umgang mit veralteten Geräten ("stale")

Wenn ein Gerät länger offline war als der Kompaktierungszeitraum, fehlen ihm Logs, die bereits gelöscht wurden. Beim nächsten Start erkennt es seinen `stale`-Status in `meta.json` und lädt automatisch den aktuellen Snapshot als neuen Ausgangspunkt:

```
┌─────────────────────────────────────────────────────┐
│  Dieses Gerät war lange offline                     │
│                                                     │
│  Einige Änderungen wurden kompaktiert. Der          │
│  aktuelle Datenstand wird jetzt vollständig         │
│  heruntergeladen (Snapshot vom 01.05.2025).         │
│                                                     │
│  Lokale Änderungen seit dem letzten Sync            │
│  dieses Geräts werden zusammengeführt.              │
│                                                     │
│  [Fortfahren]                                       │
└─────────────────────────────────────────────────────┘
```

Lokale Änderungen des veralteten Geräts (die es noch nicht hochgeladen hatte) werden als neue Log-Ereignisse auf den frischen Snapshot angewendet – mit normaler Konfliktauflösung (Abschnitt 19.3).

### 19.11 Onboarding: Neue Instanz verbindet sich mit bestehendem Datensatz

Der folgende Ablauf beschreibt, was passiert, wenn die App auf einem neuen Gerät zum ersten Mal gestartet und mit einem bereits befüllten Sync-Speicher verbunden wird.

#### Schritt 1 – Geräte-Registrierung

Beim allerersten Start generiert die neue Instanz eine eindeutige `device_id` (UUID v4) und speichert diese dauerhaft lokal. Der Nutzer vergibt optional einen Gerätenamen ("Laptop", "Büro-PC").

#### Schritt 2 – Sync-Adapter einrichten

Der Nutzer wählt den Sync-Adapter und gibt Zugangsdaten ein. Die App prüft die Verbindung sofort per `test_connection()`. Zugangsdaten werden ausschließlich in der lokalen Keychain gespeichert.

#### Schritt 3 – Bestehenden Datensatz erkennen

Die App prüft, ob `meta.json` im Sync-Speicher existiert. Ist dies der Fall, zeigt sie einen Auswahl-Dialog:

```
┌─────────────────────────────────────────────────────┐
│  Bestehender Datensatz gefunden                     │
│                                                     │
│  Snapshot vom: 01. Mai 2025                         │
│  Bekannte Geräte: Desktop-PC, MacBook               │
│                                                     │
│  [Datensatz übernehmen]      [Neu beginnen]         │
└─────────────────────────────────────────────────────┘
```

"Neu beginnen" legt einen frischen Datensatz an und überschreibt den bestehenden – mit expliziter Warnung und Bestätigung.

#### Schritt 4 – E2E-Passwort (falls aktiv)

Ist E2E-Verschlüsselung aktiviert (erkennbar an einem Flag in `meta.json`, das selbst unverschlüsselt ist), fragt die App nach dem Passwort und leitet daraus den Entschlüsselungsschlüssel ab. Ohne korrektes Passwort ist kein Zugriff auf Snapshot oder Logs möglich.

#### Schritt 5 – Snapshot herunterladen & anwenden

Die App lädt `snapshot.json` herunter, entschlüsselt ihn falls nötig, und befüllt damit den lokalen SQLite-Cache. Der Nutzer sieht einen Fortschrittsbalken mit Statusmeldung ("Daten werden geladen…"), der auch per `aria-live` angekündigt wird.

#### Schritt 6 – Neuere Log-Ereignisse anwenden

Alle Log-Dateien mit einem Zeitstempel nach `snapshot_timestamp` werden heruntergeladen und chronologisch auf den SQLite-Cache angewendet. Das Gerät ist damit vollständig auf dem aktuellen Stand.

#### Schritt 7 – Sound-Dateien nachladen

Die App ermittelt alle Sound-Hashes, die in der synchronisierten Konfiguration referenziert werden, prüft welche lokal fehlen und lädt diese aus `sync/assets/sounds/` nach.

#### Schritt 8 – Konten verbinden

Zugangsdaten (OAuth2-Tokens, API-Tokens, Basic-Auth) sind nicht im Sync-Speicher enthalten und müssen pro Gerät neu eingerichtet werden. Die App zeigt eine Liste aller bekannten Konten (Name, Typ und Capabilities kommen aus dem Snapshot) und fordert für jedes zur Anmeldung auf:

```
┌─────────────────────────────────────────────────────┐
│  Konten verbinden                                   │
│                                                     │
│  Arbeit (Google – Kalender, Aufgaben)  [Anmelden]   │
│  Privat (iCloud – Kalender, Kontakte)  [Anmelden]   │
│  Firma (Exchange EWS – Kalender)       [Anmelden]   │
│  Projekte (Vikunja – Aufgaben)         [Anmelden]   │
│                                                     │
│  [Jetzt verbinden]      [Später erledigen]          │
└─────────────────────────────────────────────────────┘
```

Nicht verbundene Konten werden in der App als "getrennt" markiert und zeigen keine Daten der jeweiligen Capability – der Rest der App ist sofort nutzbar.

#### Schritt 9 – Fehlende Plugins identifizieren

Die App vergleicht die im Snapshot referenzierten Plugin-IDs (aus `plugin.installed`-Ereignissen) mit den lokal verfügbaren Plugins (gebundelt + Community). Für jedes fehlende Community-Plugin wird der Hinweis-Dialog aus Abschnitt 20.8 angezeigt. Nativ gebundelte Plugins sind bereits Teil der App und werden hier ignoriert.

Alle Datenquellen (Kalender, Aufgabenlisten, Kontaktbücher), die ein fehlendes Plugin benötigen, werden in der App als "Plugin fehlt" markiert und ausgegraut angezeigt – bis der Nutzer das Plugin nachinstalliert.

#### Schritt 10 – Geräteregistrierung abschließen

Die neue `device_id` wird mit dem gewählten Gerätenamen in `meta.json` eingetragen (`last_seen_log` wird auf den aktuellen Zeitstempel gesetzt). Ab sofort schreibt das Gerät eigene Log-Ereignisse und wird bei der Kompaktierung berücksichtigt (Abschnitt 19.10).

#### Gesamtübersicht des Ablaufs

```
Neues Gerät startet
        │
        ▼
device_id generieren (lokal)
        │
        ▼
Sync-Adapter einrichten & Verbindung testen
        │
        ▼
meta.json vorhanden?
   │              │
  Ja             Nein → Neuen Datensatz anlegen
   │
   ▼
E2E-Passwort abfragen (falls aktiv)
        │
        ▼
Snapshot herunterladen & SQLite befüllen
        │
        ▼
Neuere Logs herunterladen & anwenden
        │
        ▼
Fehlende Sound-Dateien nachladen
        │
        ▼
Konten verbinden (Kalender, Aufgaben, Kontakte – optional: später)
        │
        ▼
Fehlende Community-Plugins identifizieren (optional: später nachinstallieren)
        │
        ▼
device_id in meta.json eintragen → Fertig
```

### 19.12 Designentscheid: Direkte API-Synchronisation statt Primary Device

#### Verworfener Ansatz: Primary Device

Ein früherer Entwurf sah vor, für jede externe Datenquelle ein "Primary Device" zu designieren – das einzige Gerät, das aktiv zur API pusht. Alle anderen Geräte hätten Änderungen über das Event Log weitergeleitet bekommen.

Dieser Ansatz wurde verworfen, weil er ein Problem löst, das bereits gelöst ist: Externe Datenquellen (Kalender, Aufgabenlisten, Kontaktbücher) sind von Natur aus Multi-Writer-Systeme. Alexa, Google-Webinterface, Outlook-Web, Todoist-Webapp, andere Kalender-/Aufgaben-Apps – all das schreibt jederzeit in dieselben Container, ohne Koordination. Ein künstliches Primary-Device-Konzept schützt nicht vor diesen externen Änderungen und fügt gleichzeitig erhebliche Komplexität hinzu (Koordination, Failover wenn Primary offline).

#### Gewählter Ansatz: Direkte API-Synchronisation mit ETag-Konfliktschutz

Jede App-Instanz kommuniziert direkt mit den externen APIs. Konflikte werden durch den in den APIs eingebauten ETag-Mechanismus (`412 Precondition Failed`) erkannt und durch die in Abschnitt 19.3 beschriebene Konfliktauflösung behandelt.

**Vorteile:**
- Kein Single Point of Failure (Primary offline = kein Push)
- Natürliche Behandlung externer Änderungen (Alexa etc.)
- Geringere Architektur-Komplexität
- APIs sind genau für diesen Multi-Writer-Fall designed

**Einziger Nachteil:** Mehrere Geräte pollen dieselbe API parallel → höherer API-Kontingentverbrauch. Entschärfung durch konfigurierbares Sync-Intervall pro Gerät (Standard: 5 Minuten; bei vielen Geräten empfohlen: 10–15 Minuten).

#### Klare Zuständigkeitstrennung

| Sync-System | Zuständig für |
|---|---|
| **Direkte API-Sync** | Termine & Kalender, Aufgaben & Aufgabenlisten, Kontakte – jeweils aus externen Quellen |
| **Event Log (Abschnitt 19)** | Lokale Einstellungen, lokale Termine/Aufgaben, Aufgabenlisten-Metadaten, Sound-Dateien, Container-Farb-Overrides (Kalender und Aufgabenlisten), Farb-Labels, Plugin-Metadaten, Tastaturkürzel-Belegungen |

### 19.13 Schema-Versionierung & Versionsabsicherung

#### Warum Schema-Versionierung notwendig ist

Wenn die App weiterentwickelt wird, können sich das Format von Log-Ereignissen, Snapshots oder `meta.json` ändern. Eine ältere App-Version, die ein neueres Schema vorfindet, könnte Daten falsch interpretieren oder korrumpieren. Deshalb wird das Schema explizit versioniert und bei jedem Sync geprüft.

#### `schema_version` in `meta.json`

`meta.json` enthält ein `schema_version`-Feld (Integer), das die aktuelle Format-Version des gesamten Sync-Datenbestands angibt. Es wird nur erhöht, wenn eine **breaking change** im Log- oder Snapshot-Format vorgenommen wird – nicht bei rückwärtskompatiblen Ergänzungen.

```json
{
  "schema_version": 1,
  "min_app_version": "1.2.0",
  ...
}
```

Zusätzlich zu `schema_version` enthält `meta.json` ein `min_app_version`-Feld: die minimale App-Version, die diesen Datensatz lesen kann. Dieses Feld wird beim Schema-Upgrade automatisch auf die aktuelle App-Version gesetzt.

#### Prüfablauf beim Sync-Start

Beim Verbinden mit dem Sync-Speicher liest die App zunächst `meta.json` – diese Datei bleibt bewusst **immer unverschlüsselt**, auch bei aktivierter E2E-Verschlüsselung, damit die Versionsfelder ohne Passwort lesbar sind.

```
App liest meta.json
        │
        ▼
app_version < min_app_version?
   │                    │
  Ja                  Nein
   │                    │
   ▼                    ▼
Update-Dialog     schema_version < bekannte Version?
anzeigen             │                    │
                    Ja                  Nein
                     │                    │
                     ▼                    ▼
             Weiter (App kann       Weiter (normaler
             ältere Schemata         Sync-Start)
             rückwärtskompatibel
             lesen)
```

#### Update-Pflicht-Dialog

Erkennt die App, dass ihre Version kleiner als `min_app_version` ist, wird der Sync blockiert und ein nicht schließbarer Dialog angezeigt:

```
┌─────────────────────────────────────────────────────┐
│  Update erforderlich                                │
│                                                     │
│  Der vorhandene Datensatz wurde mit einer neueren   │
│  Version dieser App erstellt und ist nicht          │
│  kompatibel.                                        │
│                                                     │
│  Mindest-Version:  1.2.0                            │
│  Deine Version:    1.0.4                            │
│                                                     │
│  Bitte aktualisiere die App, um fortzufahren.       │
│                                                     │
│  [Jetzt aktualisieren]      [Offline fortfahren]    │
└─────────────────────────────────────────────────────┘
```

- "Jetzt aktualisieren" öffnet den Update-Dialog (Abschnitt 21)
- "Offline fortfahren" erlaubt die Nutzung der App ohne Sync – lokale Daten bleiben lesbar, kein Schreiben in den Sync-Speicher bis zum Update
- Der Dialog ist vollständig per Tastatur und Screen Reader bedienbar

#### Schema-Migration

Wird die App auf eine Version aktualisiert, die ein neues Schema einführt, führt sie beim ersten Start eine **automatische Migration** durch:

1. Lokale SQLite-Datenbank wird auf das neue Schema migriert
2. Ein neuer Snapshot im neuen Format wird generiert und hochgeladen
3. `meta.json` wird mit der neuen `schema_version` und `min_app_version` aktualisiert
4. Alle anderen Geräte erkennen beim nächsten Sync, dass ein Update erforderlich ist

#### Rückwärtskompatible Änderungen

Nicht jede Änderung erfordert eine Schema-Versionserhöhung. Neue optionale Felder in Log-Ereignissen sind rückwärtskompatibel: ältere App-Versionen ignorieren unbekannte Felder (`#[serde(flatten)]` / `serde(deny_unknown_fields)` wird bewusst **nicht** verwendet). Nur strukturelle Brüche (umbenannte Pflichtfelder, geänderter Ereignistyp-Katalog, neues Snapshot-Format) erhöhen `schema_version`.

---

## 20. Plugin-System

### 20.1 Designprinzip

Das Plugin-System ist die architektonische Grundlage für alle Adapter (Kalender, Aufgaben, Kontakte, Sync, Videokonferenz) und Erweiterungen durch Drittentwickler. Alle Aperio-eigenen Adapter sind Teil dieses Workspace ("nativ gebundelt", siehe Abschnitt 6.1) und folgen derselben Trait- und Plugin-Architektur wie Drittentwickler-Plugins. Der Unterschied liegt ausschließlich im Auslieferungsweg: gebundelte Adapter werden mit der App mitgeliefert (auf Desktop dynamisch als shared library geladen oder auf Mobile statisch einkompiliert), während Community-Plugins vom Nutzer manuell als `.aperio`-Archiv installiert werden (siehe Abschnitt 20.7).

**Kernprinzipien:**
- Einheitliche Plugin-ABI für alle Plugin-Typen
- Nativ kompiliert pro Plattform (`.dll` / `.dylib` / `.so`)
- Auf Desktop dynamisch zur Laufzeit geladen; auf Mobile statisch einkompiliert (Feature-Flag `static-plugins`, siehe Abschnitt 20.6)
- Sprach-agnostisch über stabiles C-ABI
- Erweiterbar um neue Plugin-Typen ohne Änderung am Core

### 20.2 Plugin-Typen

| Typ | Beschreibung | Beispiele |
|---|---|---|
| `calendar-adapter` | Datenquelle für Kalender, Aufgabenlisten und/oder Kontakte (je nach deklarierten Capabilities) | Google, CalDAV, Microsoft Graph, Vikunja, Todoist |
| `sync-adapter` | Geräteübergreifende DB-Synchronisation | WebDAV, Dropbox, SFTP |
| `videoconference-adapter` | Videokonferenz-Integration (Link-Generierung + Raumverwaltung) | Zoom, Microsoft Teams, Google Meet, Cisco WebEx |
| `notification` | Benachrichtigungs-Kanal | System, E-Mail, Webhook |

Neue Plugin-Typen können in zukünftigen Versionen hinzugefügt werden, ohne bestehende Plugins zu brechen.

> **Hinweis:** Es gibt **keinen** separaten `task-adapter`-Plugin-Typ. Reine Aufgabenlisten-Anbieter (Vikunja, Todoist, Google Tasks etc.) werden als `calendar-adapter` mit `"capabilities": ["tasks"]` implementiert – siehe Abschnitt 10.2 zur Capability-Trennung. Das hält die Plugin-ABI einfach und vermeidet Doppelimplementierungen für Adapter, die sowohl Kalender als auch Aufgaben anbieten (Google, Microsoft, CalDAV).

> **Hinweis:** Der **lokale Kalender-Adapter** (`cal-adapter-local`) ist bewusst **kein** Plugin. Er teilt sich die SQLite-Datenbank des Hosts (Termine, Aufgaben, Kontakte, Einstellungen) und gehört damit zur Identität der App selbst, nicht zur austauschbaren Plugin-Schicht. Ein Plugin lebt in einer separaten shared library und kann die `Arc<Mutex<Connection>>` des Hosts nicht über die FFI-Grenze hinweg teilen; gleichzeitig ergäben "Lokal deaktivieren" / "Lokal deinstallieren" im Plugin-Manager-UI keinen sinnvollen Use Case (es ist die Heimat der lokal angelegten Daten des Nutzers). Der `LocalAdapter` wird daher direkt von src-tauri als gewöhnlicher Rust-Trait-Impl konstruiert und über alle Stellen gereicht, die ihn brauchen. Das Plugin-System (`PluginManager`, `plugins/bundled/`, Settings → Plugins) bedient ausschließlich **externe Datenquellen**.

### 20.3 Plugin-ABI: Stabiles C-Interface

Da Rust kein stabiles ABI hat, wird die Schnittstelle als **C-ABI** definiert. Jede Sprache, die eine shared library mit C-ABI produzieren kann (Rust, C, C++, Zig, Go, Swift etc.), kann Plugins implementieren.

Das `plugin-core`-Crate stellt bereit:
- Einen C-Header (`aperio_plugin.h`) als offiziellen Schnittstellenvertrag
- Rust-seitige `unsafe`-Wrapper für sicheres Laden
- Ein Rust-SDK-Crate (`plugin-sdk`) mit ergonomischen Abstraktionen für Rust-Plugin-Autoren

> **Verbindlich ist der Header, nicht dieser Abschnitt.** Der Vertrag steht in
> `crates/plugin-core/include/aperio_plugin.h` (Deskriptor, Exporte, Speicher-
> und Threading-Regeln) und `aperio_plugin_vtables.h` (Vtable-Layouts je Typ).
> Was hier steht, ist die Zusammenfassung dazu und wird bei jeder ABI-Änderung
> mitgezogen.

**Aktuelle ABI-Version: 2.** Sie wird beim Laden auf **strikte Gleichheit**
geprüft, nicht auf „mindestens“ — ein Plugin für v1 und ein Plugin für v3
werden beide abgewiesen. Geprüft wird zweimal: gegen `abi_version` im Manifest
und gegen `abi_version` im Deskriptor, und beide müssen auch untereinander
übereinstimmen. v1 kannte genau eine Instanz je geladener Bibliothek und trug
`init`/`destroy` am Deskriptor; v2 führte Instanz-Handles ein, ersetzte die
beiden durch `open_instance`/`close_instance` und stellte jeder Vtable-Methode
das Instanz-Handle als erstes Argument voran.

```c
// aperio_plugin.h (vereinfacht — Feldreihenfolge ist verbindlich)

typedef struct AperioPlugin {
    uint32_t abi_version;     // == APERIO_PLUGIN_ABI_VERSION (2)
    const char* id;           // z.B. "com.example.myplugin"
    const char* name;         // Anzeigename
    const char* version;      // SemVer
    const char* plugin_type;  // "calendar-adapter" | "sync-adapter" | ...

    // Lebenszyklus je KONTO. Ein Plugin bedient beliebig viele Instanzen.
    OpenInstanceResult (*open_instance)(const char* config_json);
    void               (*close_instance)(void* instance);

    // Typ-spezifische Vtable (als void*, gecastet je nach plugin_type)
    void* vtable;
} AperioPlugin;

// Jede shared library exportiert diese beiden Funktionen — sonst nichts Pflicht:
AperioPlugin* aperio_plugin_create(void);
void          aperio_plugin_destroy(AperioPlugin*);
```

Alle Zeichenketten im Deskriptor gehören dem Plugin und bleiben bis zur
Rückkehr von `aperio_plugin_destroy` gültig; der Host gibt sie nie frei.

Daneben gibt es **optionale** benannte Exporte, die der Host beim Laden
best-effort auflöst — fehlt einer, wird das Plugin trotzdem geladen:

| Export | Zweck |
|---|---|
| `aperio_plugin_set_log` | Log-Brücke; wird einmal direkt nach `create` aufgerufen und leitet `tracing` des Plugins in den Host-Log um |
| `aperio_plugin_interactive_auth` | Interaktiver OAuth-Flow außerhalb einer Konto-Instanz |
| `aperio_plugin_discover` | Autodiscover (EWS) |
| `aperio_plugin_probe_host_key` | Host-Key-Abfrage vor dem ersten Verbinden (SFTP, TOFU) |

Jede Vtable ist eine `#[repr(C)]`-Struktur aus `uint32_t vtable_version` plus
Funktionszeigern desselben Typs:

```c
typedef PluginCallResult (*AperioVtableMethodFn)(
    void* instance, const uint8_t* args_ptr, size_t args_len);
```

Alle Fachdaten queren die Grenze als **JSON**; die Argumentschlüssel spiegeln
die Rust-Parameternamen. Ein NULL-Slot bedeutet „nicht unterstützt“ und wird
vom Host in den `Unsupported`-Fehler der jeweiligen Domäne übersetzt. Ergebnis-
puffer allokiert das Plugin und liefert seinen eigenen `free`-Funktionszeiger
mit, sodass kein Allokator geteilt wird. Fehler reisen als `int32`-Status aus
einer festen Tabelle (`APERIO_PLUGIN_CALL_ERR_*`) plus UTF-8-Meldung. Es gibt
**kein Timeout und keinen Abbruch** — der Host wartet unbegrenzt.

Ein `calendar-adapter` zeigt auf eine `AperioCalendarPluginVtable`, die
optionale Sub-Vtables je Fähigkeit enthält:

- `calendar_vtable` (nicht-null, wenn `"calendar"` in `capabilities`)
- `tasks_vtable` (nicht-null, wenn `"tasks"` in `capabilities`)
- `contacts_vtable` (nicht-null, wenn `"contacts"` in `capabilities`)

Die anderen Typen zeigen direkt auf ihre Vtable: `sync-adapter` auf
`AperioSyncVtable`, `videoconference-adapter` auf `AperioVcVtable` mit den
Slots `test_connection`, `create_meeting`, `get_meeting`, `delete_meeting`
(Spiegel von `vc_core::VcAdapter`, siehe Abschnitt 11).

> **Slots anhängen ist derzeit nur zusammen mit einer ABI-Erhöhung sicher.**
> Das Feld `vtable_version` wird von jedem Plugin gesetzt, vom Host aber noch
> **nicht ausgewertet**; geprüft wird allein `abi_version`. Würde man einer
> bestehenden Vtable einen Slot anhängen, ohne die ABI-Version zu erhöhen,
> läse der Host bei einem älteren Plugin über dessen Struktur hinaus und riefe
> auf, was dahinter liegt. Eine **neue** Vtable für einen **neuen** Plugin-Typ
> anzulegen ist dagegen unkritisch: sie wird nur gelesen, wenn dieser Typ
> überhaupt existiert.

### 20.4 Plugin-Manifest (`plugin.json`)

Jedes Plugin wird als Verzeichnis oder Archiv mit einem Manifest ausgeliefert:

```
myplugin/
├── plugin.json           # Manifest
├── myplugin.dll          # Windows
├── myplugin.dylib        # macOS
└── myplugin.so           # Linux
```

```json
{
  "id": "com.example.myplugin",
  "name": "Mein Kalender-Plugin",
  "version": "1.0.0",
  "plugin_type": "calendar-adapter",
  "capabilities": ["calendar"],
  "abi_version": 2,
  "min_app_version": "1.0.0",
  "author": "Max Mustermann",
  "description": "Verbindet sich mit XY-Kalender",
  "signed": false
}
```

`abi_version` im Manifest muss mit der vom Plugin-Manager unterstützten ABI-Version übereinstimmen, sonst wird das Plugin abgelehnt — und zwar exakt, nicht „mindestens“ (siehe Abschnitt 20.3). `min_app_version` ist der Hebel für Vorwärtskompatibilität: ein Plugin, das eine erst später eingeführte Fähigkeit oder einen neuen Plugin-Typ voraussetzt, trägt hier die einführende Release-Version ein, damit ältere Aperio-Versionen mit „Aperio aktualisieren“ scheitern statt mit einer irreführenden Manifest-Fehlermeldung. `capabilities` deklariert die unterstützten Features (`calendar`, `tasks`, `contacts` – siehe Abschnitt 10.2 für Details).

#### 20.4.1 Konto-Schema (`account`)

Ein Adapter, der Konten hat, beschreibt sie **selbst** — im Manifest, nicht im
Kern. Der Host führt diese Beschreibung nur aus: er zeichnet das Formular, sammelt
die Werte, führt die Anmeldung durch, trennt Geheimnisse von Nicht-Geheimnissen
und reicht dem Plugin beim Öffnen genau die Init-Config, die es benannt hat. In
keiner dieser Stufen steht ein Adaptername im Kern-Code. Ein Adapter, den Aperios
Autoren nie gesehen haben, wird darum genauso eingerichtet wie ein mitgelieferter.

Der Block ist optional. Fehlt er, hat der Host keinen generischen Weg, dieses
Plugin zu verbinden — die richtige Antwort für einen Benachrichtigungskanal und
für die Adapter, die noch auf dem älteren, pro Art fest verdrahteten Pfad liegen.

```json
"account": {
  "fields": [
    { "key": "client_id", "kind": "text", "label": "Client ID",
      "label_key": "dialogs.accounts.webexClientIdLabel", "required": true },
    { "key": "client_secret", "kind": "secret",
      "secret_slot": "oauth_client_secret", "label": "Client secret",
      "required": true },
    { "key": "use_personal_room", "kind": "bool",
      "label": "Persönlichen Raum verwenden", "default": false }
  ],
  "oauth": {
    "builtin_provider": "webex",
    "client_id_field": "client_id",
    "client_secret_field": "client_secret",
    "refresh_token_field": "refresh_token"
  },
  "host_channel": true
}
```

**Felder.** `key` ist zugleich der Schlüssel, unter dem der Wert in der
Init-Config des Plugins auftaucht — ein Nicht-Geheimnis wird unter demselben
Namen in `config_json` abgelegt, die Rückreise braucht also keine Zuordnungs-
tabelle. `kind` ist `text`, `url`, `secret` oder `bool` und steuert das
Eingabefeld auf beiden Plattformen (auf Mobil auch die Bildschirmtastatur, was
`url` von `text` unterscheidet). `label` ist die Zeichenkette des Plugin-Autors;
`label_key` benennt zusätzlich einen Übersetzungsschlüssel, den die App in der
Sprache des Nutzers auflöst und der Vorrang hat. Mitgelieferte Adapter setzen
den Schlüssel — so liegen die Texte in den Sprachdateien, wo Übersetzungen
hingehören, und die Struktur im Manifest, wo das Wissen des Adapters hingehört.
Ein Fremd-Plugin ohne Übersetzung fällt auf sein Literal zurück, was ehrlicher
ist als ein fehlender Schlüssel. `hint`/`hint_key` funktionieren genauso.

**Geheimnisse.** Ein Feld ist genau dann ein Geheimnis, wenn es `secret_slot`
nennt, und ein Geheimnis erreicht **niemals** `config_json`: diese Spalte ist als
nicht-geheim dokumentiert und wird bei ausgeschalteter Ende-zu-Ende-Verschlüsse-
lung unverschlüsselt an das Sync-Log angehängt, träfe also im Klartext beim
eigenen Sync-Ziel des Nutzers ein. Beide Richtungen werden beim Laden geprüft:
ein `secret` ohne Slot und ein Slot an einem Nicht-Geheimnis lassen das Manifest
scheitern. Erlaubt sind `access_token`, `refresh_token`, `password`, `api_token`
und `oauth_client_secret`. Der Ende-zu-Ende-Schlüssel ist **nicht benennbar** —
es gibt für ihn keine Variante in diesem Enum, er wird also nicht etwa abgelehnt,
sondern kann gar nicht erst erfragt werden.

**OAuth.** Der Host führt den *Ablauf* (Browser bzw. native Auth-Session, die
beiden Mobil-Phasen, das Verwahren des Ergebnisses) und weiß nichts über den
Anbieter; Endpunkte, Scopes und Tausch gehören dem Plugin und laufen über dessen
`aperio_plugin_interactive_auth`. `client_id_field` und `client_secret_field`
benennen die beiden Felder; `refresh_token_field` und `access_token_field`
sagen, unter welchem Namen die Token beim Öffnen in die Init-Config gehören —
fehlt eins, will das Plugin es nicht haben.

`builtin_provider` benennt den Satz Zugangsdaten, den ein Build für diesen
Anbieter mitbringen kann (siehe `crates/builtin-oauth`). Trägt der Build ihn,
werden die beiden Zugangsdaten-Felder optional: bleiben beide leer, meldet sich
Aperio mit der eigenen Registrierung an, und das Konto merkt sich nur, *welche*
das war (`client_source` + ein zwölfstelliger Fingerabdruck), nicht die Zugangs-
daten selbst. Das ist Absicht: Aperios Client-Secret gehört dem Build und nicht
dem Nutzer, es in dessen Schlüsselbund zu kopieren würde es auf jedes Gerät
tragen, auf das das Konto synchronisiert, und auf dem Wert einfrieren, den es am
Anlegetag hatte — genau die beiden Eigenschaften, die etwas Rotierbares nicht
haben darf. Ein Build mit einer *anderen* Registrierung wird beim Registrieren
am Fingerabdruck erkannt und sagt das, statt Wochen später als unerklärliches
`invalid_grant` zu enden.

Eine *halbe* Angabe — Client-ID getippt, Secret leer — ist ein Fehler und keine
stille Rückfallebene auf die eingebauten Zugangsdaten: wer die eigene ID einträgt,
will die eigene Integration, und eine stille Anmeldung als Aperio bände das Konto
an Zugangsdaten, die der Nutzer nicht gewählt hat und nicht rotieren kann, ohne
dass es von außen sichtbar wäre.

**`host_channel`.** Setzt das Plugin es, bekommt jede Instanz ein Capability-Token
und kann darüber ein rotiertes Zugangsdatum zurückmelden (Abschnitt 20.10). Aus,
solange es nicht verlangt wird: das Token ist Vollmacht, und Vollmacht, die
niemand angefordert hat, hat auch niemand geprüft.

### 20.5 Plugin-Manager (Laufzeit)

Der Plugin-Manager im Rust-Backend ist zuständig für:

- Laden und Entladen von Plugins zur Laufzeit (`libloading`-Crate)
- ABI-Versionscheck vor dem Laden
- Sicherheits-Dialog bei unsignierten Plugins
- Plugin-Registrierung und Lebenszyklusverwaltung

```rust
// Pseudocode Plugin-Ladevorgang
fn load_plugin(path: &Path) -> Result<LoadedPlugin> {
    let manifest = read_manifest(path)?;

    // ABI-Versionscheck
    if manifest.abi_version != SUPPORTED_ABI_VERSION {
        return Err(PluginError::AbiMismatch);
    }

    // Sicherheits-Dialog bei unsignierten Plugins (siehe 20.7)
    if !manifest.signed {
        await_user_confirmation(&manifest)?;
    }

    // Shared library laden
    let lib = libloading::Library::new(platform_lib_path(path, &manifest))?;
    let create_fn: Symbol<unsafe extern "C" fn() -> *mut AperioPlugin>
        = lib.get(b"aperio_plugin_create")?;

    Ok(LoadedPlugin { lib, plugin: unsafe { &*create_fn() }, manifest })
}
```

### 20.6 Nativ gebundelte Plugins

Nativ gebundelte Plugins (alle in diesem Dokument spezifizierten Adapter) werden als shared libraries mit der App ausgeliefert und beim Start automatisch geladen – kein Nutzereingriff nötig. Sie landen in einem `plugins/bundled/`-Verzeichnis relativ zur App-Binary.

Für mobile Plattformen (iOS, Android), wo dynamisches Nachladen von Bibliotheken nicht erlaubt ist, werden gebundelte Plugins **statisch einkompiliert** – über ein Feature-Flag im Build-System:

```toml
# Cargo.toml
[features]
dynamic-plugins = []          # Desktop: dynamisch laden
static-plugins  = [           # Mobile: statisch einkompilieren
    "cal-adapter-google",
    "cal-adapter-microsoft-graph",
    "cal-adapter-ews",
    "cal-adapter-caldav",
    "cal-adapter-ical",
    "cal-adapter-vikunja",
    "cal-adapter-todoist",
    "sync-adapter-webdav",
    "sync-adapter-ftp",
    "sync-adapter-sftp",
    "sync-adapter-dropbox",
    "sync-adapter-googledrive",
    "sync-adapter-local",
    "vc-adapter-zoom",
    "vc-adapter-teams",
    "vc-adapter-meet",
    "vc-adapter-webex",
]
```

`cal-adapter-local` taucht in dieser Liste bewusst nicht auf — er ist host-intern (siehe Hinweis in §20.2) und wird direkt von src-tauri als gewöhnliche Bibliothek genutzt, nicht über den Plugin-Manager. Auf Mobile gilt dasselbe wie auf Desktop: der `LocalAdapter` ist Teil der App-Binary, nicht ein zu ladendes Artefakt.

Der Plugin-Manager erkennt zur Laufzeit, welcher Modus aktiv ist, und lädt Plugins entsprechend. Die Plugin-API bleibt für den Rest der App identisch.

### 20.7 Community-Plugins: Installation & Sicherheit

Community-Plugins werden manuell als Datei (Archiv oder Verzeichnis) installiert. Der Nutzer zieht die Datei in das Plugin-Management-Fenster oder wählt sie über einen Dateiauswahl-Dialog.

#### Installations-Ablauf

```
1. Nutzer wählt Plugin-Archiv (`.aperio`)
2. App entpackt und liest plugin.json
3. ABI-Versionscheck
4. Plugin ist unsigniert → Sicherheits-Dialog:

┌─────────────────────────────────────────────────────┐
│  Plugin installieren                                │
│                                                     │
│  Name:     Mein Kalender-Plugin                     │
│  Autor:    Max Mustermann                           │
│  Version:  1.0.0                                    │
│  Typ:      Kalender-Adapter                         │
│                                                     │
│  ⚠ Dieses Plugin ist nicht signiert. Installiere    │
│  nur Plugins aus vertrauenswürdigen Quellen.        │
│                                                     │
│  [Trotzdem installieren]        [Abbrechen]         │
└─────────────────────────────────────────────────────┘

5. Nach Bestätigung: Plugin in plugins/user/ kopieren
6. Plugin laden und registrieren
7. plugin.installed-Ereignis ins Event Log schreiben
```

#### Dateiendung

Plugins werden als `.aperio`-Archiv (ZIP-basiert) verteilt, das `plugin.json` und die plattformspezifischen shared libraries enthält. Doppelklick auf eine `.aperio`-Datei startet dank der Systemintegration (Abschnitt 17.1) direkt den Installations-Ablauf.

### 20.8 Plugin-Synchronisation zwischen Geräten

Wenn ein Gerät ein neues Plugin installiert, wird dies über das Event Log an andere Geräte kommuniziert:

```json
{
  "type": "plugin.installed",
  "payload": {
    "id": "com.example.myplugin",
    "name": "Mein Kalender-Plugin",
    "version": "1.0.0",
    "plugin_type": "calendar-adapter"
  }
}
```

**Wichtig:** Die Plugin-Binärdatei selbst wird **nicht** über das Event Log synchronisiert – nur der Name und die Metadaten. Andere Geräte erhalten beim nächsten Start einen Hinweis-Dialog:

```
┌─────────────────────────────────────────────────────┐
│  Plugin benötigt                                    │
│                                                     │
│  Ein anderes Gerät nutzt das Plugin:                │
│  "Mein Kalender-Plugin" (v1.0.0)                    │
│                                                     │
│  Ohne dieses Plugin sind zugehörige Datenquellen    │
│  (Kalender, Aufgabenlisten, Kontakte) auf diesem    │
│  Gerät nicht verfügbar.                             │
│                                                     │
│  [Plugin installieren]      [Ignorieren]            │
└─────────────────────────────────────────────────────┘
```

"Plugin installieren" öffnet den Dateiauswahl-Dialog, damit der Nutzer die Plugin-Datei manuell bereitstellt. Die Datenquelle, die das Plugin benötigt, wird bis zur Installation als "Plugin fehlt" markiert und ausgegraut angezeigt.

Nativ gebundelte Plugins werden nie über das Event Log kommuniziert – sie sind auf allen Instanzen immer vorhanden.

### 20.9 Plugin-Updates

Plugin-Updates werden manuell eingespielt (analog zur Installation). Der Plugin-Manager erkennt anhand der `id`, dass es sich um ein Update handelt, und bietet einen Bestätigungs-Dialog:

```
┌─────────────────────────────────────────────────────┐
│  Plugin aktualisieren                               │
│                                                     │
│  "Mein Kalender-Plugin"                             │
│  Installiert:   1.0.0                               │
│  Neu:           1.1.0                               │
│                                                     │
│  [Aktualisieren]            [Abbrechen]             │
└─────────────────────────────────────────────────────┘
```

Nach dem Update wird ein `plugin.updated`-Ereignis ins Event Log geschrieben, damit andere Geräte informiert werden.

### 20.10 Plugin-Management in den Einstellungen

Unter `Einstellungen → Plugins` gibt es eine vollständige Plugin-Verwaltung:

| Bereich | Inhalt |
|---|---|
| **Installierte Plugins** | Liste aller aktiven Plugins (gebundelt + Community), mit Status, Version, Typ |
| **Deaktivieren / Aktivieren** | Plugin temporär deaktivieren ohne Deinstallation |
| **Deinstallieren** | Community-Plugins entfernen (gebundelte nicht deinstallierbar); schreibt `plugin.uninstalled`-Ereignis ins Event Log |
| **Plugin installieren** | Dateiauswahl-Dialog für neue Plugins |
| **Details** | Plugin-Manifest, Autor, ABI-Version, Signatur-Status |

Die gesamte Plugin-Verwaltung ist vollständig per Tastatur navigierbar und Screen-Reader-kompatibel (`role="list"`, `aria-label` pro Plugin-Eintrag).

### 20.11 Plugin-SDK für Entwickler

Für Entwickler, die Plugins in Rust schreiben möchten, wird ein `plugin-sdk`-Crate veröffentlicht, das die unsicheren C-ABI-Details kapselt:

```rust
// Beispiel: Minimales Adapter-Plugin mit Kalender-Capability in Rust
use aperio_plugin_sdk::prelude::*;

#[aperio_plugin(
    type = "calendar-adapter",
    capabilities = ["calendar"]
)]
struct MyCalendarPlugin;

#[async_trait]
impl Adapter for MyCalendarPlugin {
    async fn authenticate(&self, credentials: Credentials) -> Result<AuthToken> {
        // Implementierung
    }
    fn capabilities(&self) -> &[Capability] { &[Capability::Calendar] }
}

#[async_trait]
impl CalendarFeature for MyCalendarPlugin {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        // Implementierung
    }
    // ... weitere Methoden
}

aperio_plugin_export!(MyCalendarPlugin);
```

Plugins, die mehrere Capabilities anbieten, implementieren mehrere Feature-Traits parallel (z.B. `CalendarFeature` + `TasksFeature` + `ContactsFeature`). Das Makro `aperio_plugin_export!` generiert automatisch die C-ABI-Exports (`aperio_plugin_create`, `aperio_plugin_destroy`) und die korrekte `AperioPlugin`-Struktur basierend auf den deklarierten Capabilities.

Für andere Sprachen wird `aperio_plugin.h` als offizieller Schnittstellenvertrag veröffentlicht.

---

## 21. Self-Update-System

### 21.1 Mechanismus

Tauris integriertes **Updater-Plugin** (`tauri-plugin-updater`) wird verwendet:

- Prüft beim App-Start auf neue Version via GitHub Releases API
- Download des neuen Releases im Hintergrund
- **Immer mit Nutzer-Bestätigung** vor der Installation:
  - Dialog: "Version X.Y.Z ist verfügbar – Jetzt installieren?"
  - Optionen: Jetzt installieren / Später erinnern / Diese Version überspringen
- Der Dialog ist vollständig per Tastatur und Screen Reader bedienbar

#### Update-Ablauf für portable App

Da keine feste Installation vorhanden ist, ersetzt das Update die Binary und die Plugin-Bibliotheken direkt im App-Verzeichnis:

1. Neues ZIP-Paket wird in ein temporäres Verzeichnis heruntergeladen
2. App-Binary und `plugins/bundled/`-Inhalte werden ersetzt
3. Das `data/`-Verzeichnis (Nutzerdaten) und `plugins/user/` (Community-Plugins) werden **nicht** berührt
4. App startet automatisch mit der neuen Version neu

Dieser Ablauf stellt sicher, dass bei einem Update keine Nutzerdaten verloren gehen.

### 21.2 Update-Manifest

```json
{
  "version": "1.2.0",
  "notes": "Fehlerbehebungen und Verbesserungen (Beispieltext – wird bei jedem Release aktualisiert)",
  "pub_date": "2025-05-12T00:00:00Z",
  "platforms": {
    "windows-x86_64": { "url": "...", "signature": "..." },
    "darwin-aarch64": { "url": "...", "signature": "..." },
    "darwin-x86_64":  { "url": "...", "signature": "..." },
    "linux-x86_64":   { "url": "...", "signature": "..." }
  }
}
```

### 21.3 Code-Signing

| Plattform | Signing-Methode |
|---|---|
| macOS | Ad-hoc-Signing (`codesign --force --deep -s -`) – kein Developer Account nötig |
| Windows | Vorerst unsigniert; Workflow dokumentiert für spätere Zertifikat-Integration |
| Linux | GPG-Signatur der Binary (optional) |

> **Hinweis:** Unter macOS wird Gatekeeper bei ad-hoc-signierten Apps beim ersten Start einen Warnhinweis anzeigen. Nutzer müssen die App einmalig über "Systemeinstellungen → Datenschutz & Sicherheit → Trotzdem öffnen" freigeben. Dieser Hinweis wird in der README dokumentiert.

---

## 22. Build- & Release-Workflow

### 22.1 GitHub Actions CI/CD

#### Dokumentations-Workflow (bei Änderungen unter `docs/`)

Separater Workflow – siehe Abschnitt 24.5 für vollständige Konfiguration. Baut die Astro-Starlight-Site (`web/`) und deployt sie auf GitHub Pages.

#### Continuous Integration (bei jedem Push/PR)

```yaml
# .github/workflows/ci.yml
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    steps:
      - cargo test
      - cargo clippy
      - npm run test
      - npm run lint
```

#### Release-Workflow (bei neuem Tag `v*.*.*`)

```yaml
# .github/workflows/release.yml
jobs:
  build:
    strategy:
      matrix:
        include:
          - os: ubuntu-latest   → Linux x86_64 portable binary
          - os: windows-latest  → Windows x86_64 portable .exe
          - os: macos-latest    → macOS universal binary (x86_64 + aarch64)
    steps:
      - tauri build --no-bundle   # Keine Installer, nur standalone Binary
      - Ad-hoc-Signing (macOS)
      - Portable Paket schnüren (siehe 22.2)
      - Upload zu GitHub Releases
      - Update-Manifest aktualisieren
```

### 22.2 Portable Binary-Konfiguration

#### Tauri: Installer deaktivieren

Tauris Standard-Build erzeugt NSIS-Installer und MSI-Pakete. Für die portable Einzelbinary wird der Bundle-Schritt übersprungen:

```json
// tauri.conf.json
{
  "bundle": {
    "active": false,
    "windows": {
      "webviewInstallMode": {
        "type": "downloadBootstrapper"
      }
    }
  }
}
```

`"active": false` deaktiviert alle Installer-Artefakte. `downloadBootstrapper` stellt sicher, dass WebView2 bei Bedarf automatisch nachgeladen wird (siehe Abschnitt 2.5).

#### Portable Paketstruktur

Das Release-Artefakt ist ein ZIP-Archiv mit folgender Struktur:

```
Aperio-[VERSION]-[PLATFORM].zip
├── Aperio.exe          # Windows
├── Aperio              # Linux / macOS
├── plugins/
│   └── bundled/            # Nativ gebundelte Plugins (.dll / .dylib / .so)
│       ├── cal-adapter-google.[ext]
│       ├── cal-adapter-microsoft-graph.[ext]
│       ├── cal-adapter-ews.[ext]
│       ├── cal-adapter-caldav.[ext]
│       ├── cal-adapter-ical.[ext]
│       ├── cal-adapter-local.[ext]
│       ├── cal-adapter-vikunja.[ext]
│       ├── cal-adapter-todoist.[ext]
│       ├── sync-adapter-webdav.[ext]
│       ├── sync-adapter-ftp.[ext]
│       ├── sync-adapter-sftp.[ext]
│       ├── sync-adapter-dropbox.[ext]
│       ├── sync-adapter-googledrive.[ext]
│       ├── sync-adapter-local.[ext]
│       ├── vc-adapter-zoom.[ext]
│       ├── vc-adapter-teams.[ext]
│       ├── vc-adapter-meet.[ext]
│       └── vc-adapter-webex.[ext]
└── README.txt              # Kurzanleitung, WebView2-Hinweis (Windows)
```

> **Hinweis:** Die Plugin-Bibliotheken liegen neben der Binary, da dynamisch ladbare shared libraries (.dll etc.) nicht in eine einzelne `.exe` eingebettet werden können. Das ZIP-Archiv als Ganzes ist das portable Artefakt – der Nutzer entpackt es in ein beliebiges Verzeichnis.

#### App-Daten: Portabel neben der Binary

Alle Nutzerdaten und Einstellungen werden **relativ zur Binary** gespeichert – im selben Verzeichnis wie die `.exe` bzw. das ausführbare Binary. Das gesamte ZIP-Verzeichnis kann auf einen anderen Rechner kopiert werden und die App läuft dort mit allen Einstellungen sofort:

```
Aperio-[VERSION]-[PLATFORM]/
├── Aperio.exe
├── plugins/
│   ├── bundled/            # Nativ gebundelte Plugins (App-Bestandteil)
│   └── user/               # Community-Plugins (nach manueller Installation)
├── data/
│   ├── db.sqlite           # Lokale Datenbank
│   ├── config.json         # App-Einstellungen
│   ├── app_config.json     # Fenstergröße, -position (gerätespezifisch)
│   └── sounds/             # Benutzerdefinierte Sound-Dateien
└── README.txt
```

> **Hinweis:** `app_config.json` enthält gerätespezifische Einstellungen (Fenstergröße, -position) und wird beim Kopieren auf ein anderes System einfach ignoriert bzw. neu angelegt – das ist unproblematisch.

#### Erkennung des Daten-Verzeichnisses

Die App erkennt beim Start automatisch, ob sie portabel läuft: Sie prüft, ob ein `data/`-Verzeichnis neben der Binary existiert (oder angelegt werden kann). Ist das der Fall, werden alle Daten dort gespeichert. Andernfalls – z.B. wenn das Binary-Verzeichnis schreibgeschützt ist – fällt die App auf die plattformspezifischen Nutzerprofil-Verzeichnisse zurück:

```rust
fn resolve_data_dir() -> PathBuf {
    let binary_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let portable_data = binary_dir.join("data");

    // Portabler Modus: data/ neben der Binary beschreibbar?
    if portable_data.exists() || std::fs::create_dir(&portable_data).is_ok() {
        return portable_data;
    }

    // Fallback: Systemverzeichnis
    dirs::data_local_dir()
        .unwrap()
        .join("Aperio")
}
```

| Szenario | Datenpfad |
|---|---|
| Portabel (USB-Stick, beliebiges Verzeichnis) | `./data/` neben der Binary |
| Schreibgeschütztes Binary-Verzeichnis | `%APPDATA%\Aperio\` (Windows) / `~/Library/Application Support/Aperio/` (macOS) / `~/.config/Aperio/` (Linux) |

Keine Registry-Einträge außer den optionalen, benutzerspezifischen Systemintegrations-Einträgen (Abschnitt 17), die auf Wunsch des Nutzers gesetzt werden.

### 22.3 Versionierung

Semantische Versionierung (SemVer): `MAJOR.MINOR.PATCH`

---

## 23. Dateistruktur (Projektlayout)

```
Aperio/
├── Cargo.toml                             # Workspace-Root
├── crates/
│   │
│   │── # Kern-Bibliotheken
│   ├── cal-core/                          # Gemeinsame Typen, Traits, Fehlertypen
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── adapter.rs                 # Adapter-Basis-Trait & Feature-Traits (CalendarFeature, TasksFeature, ContactsFeature)
│   │       ├── types.rs                   # Event, Calendar, DateRange, Task, TaskList, Contact, etc.
│   │       └── color.rs                   # ContainerColor, ColorLabelId
│   ├── plugin-core/                       # Plugin-ABI (C-Header), Plugin-Manager
│   │   ├── src/lib.rs
│   │   └── aperio_plugin.h              # C-ABI-Schnittstellenvertrag
│   ├── plugin-sdk/                        # Rust-SDK für Plugin-Entwickler
│   │   └── src/lib.rs
│   ├── sync-core/                         # Sync-Adapter-Trait & Event-Log-Typen
│   │   └── src/lib.rs
│   │
│   │── # Kalender- und Aufgaben-Adapter (nativ gebundelt)
│   ├── cal-adapter-google/                # Google Calendar API v3 + Google Tasks
│   │   └── src/lib.rs
│   ├── cal-adapter-microsoft-graph/       # Outlook / Exchange Online + MS To Do
│   │   └── src/lib.rs
│   ├── cal-adapter-ews/                   # Exchange on-premise (SOAP/EWS)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── soap.rs                    # SOAP-Envelope-Builder
│   │       └── xml.rs                     # XML-Parsing via quick-xml
│   ├── cal-adapter-caldav/                # CalDAV + CardDAV (inkl. iCloud, Nextcloud)
│   │   └── src/lib.rs
│   ├── cal-adapter-ical/                  # Lokale / URL-basierte .ics-Dateien
│   │   └── src/lib.rs
│   ├── cal-adapter-local/                 # Lokaler Kalender (kein externes Protokoll)
│   │   └── src/lib.rs
│   ├── cal-adapter-vikunja/               # Vikunja (REST API, nur tasks)
│   │   └── src/lib.rs
│   ├── cal-adapter-todoist/               # Todoist (REST API, nur tasks)
│   │   └── src/lib.rs
│   │
│   │── # Sync-Adapter (nativ gebundelt)
│   ├── sync-adapter-webdav/               # WebDAV (verschlüsselt & unverschlüsselt)
│   │   └── src/lib.rs
│   ├── sync-adapter-ftp/                  # FTPS
│   │   └── src/lib.rs
│   ├── sync-adapter-sftp/                 # SFTP
│   │   └── src/lib.rs
│   ├── sync-adapter-dropbox/              # Dropbox API v2
│   │   └── src/lib.rs
│   ├── sync-adapter-googledrive/          # Google Drive API v3
│   │   └── src/lib.rs
│   ├── sync-adapter-local/                # Lokales Dateisystem / NAS
│   │   └── src/lib.rs
│   │
│   │── # Videokonferenz-Adapter (nativ gebundelt)
│   ├── vc-adapter-zoom/                   # Zoom API
│   │   └── src/lib.rs
│   ├── vc-adapter-teams/                  # Microsoft Teams (via Graph API)
│   │   └── src/lib.rs
│   ├── vc-adapter-meet/                   # Google Meet (via Calendar API)
│   │   └── src/lib.rs
│   └── vc-adapter-webex/                  # Cisco WebEx API
│       └── src/lib.rs
│
├── src-tauri/                             # Tauri-App (Rust Backend)
│   ├── src/
│   │   ├── main.rs
│   │   ├── commands/                      # Tauri-Commands (API zu Frontend)
│   │   ├── db/                            # SQLite-Datenbankschicht
│   │   ├── sync/                          # Sync Engine, Queue & Event Log
│   │   └── updater/                       # Self-Update-Logik
│   └── Cargo.toml
├── src/                                   # React Frontend (TypeScript)
│   ├── components/
│   │   ├── views/                         # Kalenderansichten
│   │   │   ├── DayView.tsx
│   │   │   ├── WeekView.tsx
│   │   │   ├── MonthView.tsx
│   │   │   ├── YearView.tsx
│   │   │   └── AgendaView.tsx
│   │   ├── tasks/
│   │   │   ├── TaskView.tsx               # Dedizierte Aufgaben-Ansicht
│   │   │   └── Backlog.tsx                # Backlog-Sidebar
│   │   ├── EventDialog.tsx                # Termin erstellen/bearbeiten
│   │   ├── TaskDialog.tsx                 # Aufgabe erstellen/bearbeiten
│   │   ├── QuickAdd.tsx                   # Schnellerstellungs-Dialog
│   │   ├── Sidebar.tsx                    # Mini-Kalender, Kalender- und Aufgabenlisten-Filter
│   │   ├── PluginManager.tsx              # Plugin-Verwaltung (Einstellungen)
│   │   └── Settings.tsx                   # Einstellungen
│   ├── locales/
│   │   ├── de/translation.json
│   │   └── en/translation.json
│   └── App.tsx
├── .github/
│   └── workflows/
│       ├── ci.yml
│       ├── release.yml
│       └── docs.yml                           # Dokumentation → GitHub Pages
├── docs/
│   ├── dev/                               # Entwickler-Dokumentation (Englisch)
│   ├── plugin-dev/                        # Plugin-Entwickler-Dokumentation (Englisch)
│   └── user/                              # Nutzer-Dokumentation (Deutsch)
└── README.md
```

> **Community-Plugins** (Drittentwickler-Erweiterungen für proprietäre oder Nischen-Systeme, die nicht im Aperio-Kern enthalten sind) liegen in **separaten Repositories** außerhalb dieses Workspace und werden vom Nutzer als `.aperio`-Archiv manuell installiert (siehe Abschnitt 20.7). Alle in diesem Dokument spezifizierten Adapter sind hingegen **Aperio-eigene, gebundelte Adapter** – sie sind Teil dieses Workspaces und werden mit jeder App-Version mitgeliefert.

---

## 24. Dokumentation

Die App erfordert drei eigenständige Dokumentations-Bereiche, die von Anfang an parallel zur Entwicklung gepflegt werden – nicht nachgelagert.

### 24.1 Übersicht & Ablageort

Alle Dokumentationen liegen im Repository unter `web/` – eine Astro-Starlight-Site (Landing + Docs), siehe Abschnitt 24.5:

```
web/src/content/docs/
├── index.mdx                   # Splash-Landing (Englisch); de/index.mdx (Deutsch)
├── privacy.md / terms.md / impressum.md   # Rechtsseiten (Englisch; de/ darunter)
├── guides/                     # Benutzerhandbuch (Englisch, Root-Locale)
│   ├── index.md                # Einstieg (Willkommen)
│   ├── tutorial/               # 01-installation … 09-tastaturkuerzel
│   ├── tastaturkuerzel.md      # Vollständige Kürzel-Referenz
│   ├── barrierefreiheit.md     # Tipps für Screen-Reader-Nutzer
│   └── google-oauth.md         # OAuth-Anleitung (Übergangslösung)
├── developers/                 # Entwickler-Dokumentation (Englisch)
│   ├── getting-started.md / architecture.md / contributing.md / testing.md
│   └── adapters/               # overview, local, caldav, google, microsoft, ews, vikunja, todoist
├── plugins/                    # Plugin-Entwicklung (Englisch)
│   ├── getting-started.md / abi-reference.md / rust-sdk.md / manifest.md
│   └── examples/               # hello-world, calendar-adapter-template
└── de/guides/                  # Benutzerhandbuch (Deutsch, gespiegelte Struktur)
```

> **Hinweis:** Inhalte sind Markdown/MDX mit `title`-Frontmatter; Navigation (Sidebar, Reihenfolge, Gruppen) steuert `web/astro.config.mjs`. Zweisprachigkeit, Volltextsuche und Theming liefert Starlight (Abschnitt 24.5).

### 24.2 Entwickler-Dokumentation (Englisch)

Zielgruppe: Entwickler, die an der App mitentwickeln möchten.

#### `dev/src/getting-started.md`

Schnellstart für neue Mitwirkende:

- Voraussetzungen (Rust, Node.js, Tauri CLI)
- Repository klonen und Workspace aufsetzen
- Entwicklungs-Build starten (`cargo tauri dev`)
- Übersicht der wichtigsten `cargo`-Befehle
- Wo fange ich an? (Wegweiser zu relevanten Crates je nach Interesse)

#### `dev/src/architecture.md`

- Architektur-Diagramm (Tauri Backend ↔ Frontend ↔ Plugin-System)
- Erklärung des Cargo-Workspace-Aufbaus
- Kommunikationsfluss: Tauri-Commands, Event-System
- Datenpfad-Logik (portabel vs. Fallback)
- Event-Log-Architektur (Abschnitt 19) in Kurzform

#### `dev/src/contributing.md`

- Branching-Strategie (z.B. `feature/`, `fix/`, `docs/`)
- Commit-Konventionen (Conventional Commits empfohlen)
- PR-Prozess & Review-Erwartungen
- Barrierefreiheit als Pflicht-Kriterium bei jedem PR
- Code-Style (rustfmt, clippy, ESLint)

#### `dev/src/testing.md`

- Unit-Tests je Crate (`cargo test`)
- Integration-Tests für Adapter (Mock-Server)
- Barrierefreiheits-Tests (welche Screen-Reader-Kombinationen müssen getestet werden)
- CI-Matrix (welche Plattformen, welche Tests)

#### `dev/src/adapters/`

Pro Adapter eine Datei mit:
- Protokoll-Übersicht
- Authentifizierungsablauf
- Besonderheiten / bekannte Eigenheiten des Anbieters
- Testanleitung (Sandbox-Zugänge, Mock-Server)

### 24.3 Plugin-Entwickler-Dokumentation (Englisch)

Zielgruppe: Entwickler, die ein eigenes Plugin schreiben möchten.

#### `plugin-dev/src/getting-started.md`

Ziel: Ein funktionierendes Minimal-Plugin in unter 15 Minuten.

1. Voraussetzungen (Rust empfohlen, C/C++/Zig möglich)
2. Rust-SDK einbinden (`plugin-sdk` als Dependency)
3. Plugin-Typ-spezifisches Trait implementieren (z.B. `CalendarAdapterPlugin`, `SyncAdapterPlugin` etc. – je nach gewähltem Plugin-Typ)
4. `plugin.json`-Manifest erstellen
5. Build & Test
6. Als `.aperio`-Archiv paketieren
7. In der App installieren und ausprobieren

#### `plugin-dev/src/abi-reference.md`

Vollständige Spezifikation des C-ABI:
- Haupt-Plugin-Struktur (`AperioPlugin`)
- Lifecycle-Funktionen (`init`, `destroy`)
- Vtable-Layout je Plugin-Typ
- Fehlerbehandlung & Rückgabewerte
- Speicherverwaltung (wer alloziert, wer gibt frei)
- ABI-Versionsgarantien

#### `plugin-dev/src/manifest.md`

Alle Felder von `plugin.json` mit Typen, Pflichtfeldern und Beispielen.

#### `plugin-dev/src/examples/`

- `hello-world/`: Minimales Plugin, das nur `list_calendars` implementiert und eine leere Liste zurückgibt – perfekter Ausgangspunkt
- `calendar-adapter-template/`: Vollständiges Template für einen Kalender-Adapter mit allen Methoden als Stubs und inline-Kommentaren

### 24.4 Nutzer-Dokumentation (Deutsch)

Zielgruppe: Endnutzer, insbesondere auch Screen-Reader-Nutzer. Geschrieben in einfacher, klarer Sprache ohne Entwickler-Jargon.

#### Schritt-für-Schritt-Tutorial

Das Tutorial führt neue Nutzer linear durch alle Kernfunktionen der App. Jedes Kapitel baut auf dem vorherigen auf:

| Kapitel | Inhalt |
|---|---|
| **01 – Installation & Start** | ZIP entpacken, App starten, Erststart-Assistent durchlaufen, Systemintegration einrichten |
| **02 – Kalender und Aufgabenlisten verbinden** | Google-Konto verbinden, iCloud einrichten, lokalen Kalender anlegen, reine Aufgabenlisten anbinden (Vikunja, Todoist), mehrere Konten verwalten |
| **03 – Termine** | Termin erstellen (Schnellerstellung & vollständiges Formular), bearbeiten, löschen, verschieben, wiederkehrende Termine |
| **04 – Aufgaben** | Aufgabe erstellen, Aufgabenliste wählen, Backlog verstehen, Aufgabe einplanen, Wochenplanung, wiederkehrende Aufgaben |
| **05 – Ansichten** | Alle Ansichten im Überblick: Tages-, Wochen-, Monats-, Jahres-, Agenda- und Aufgaben-Ansicht; Ansicht wechseln, KW-Anzeige, Wochenplanung nutzen |
| **06 – Benachrichtigungen** | Erinnerungen konfigurieren, Sounds einstellen, Snooze nutzen |
| **07 – Suche** | Termine und Aufgaben suchen, Filter verwenden |
| **08 – Synchronisation** | Geräteübergreifende Sync einrichten (WebDAV, Dropbox etc.), Konfliktauflösung verstehen |
| **09 – Tastaturkürzel** | Übersicht der wichtigsten Kürzel, Kürzel anpassen |

Jedes Kapitel enthält:
- Eine kurze Einleitung was in diesem Kapitel gelernt wird
- Schritt-für-Schritt-Anleitungen mit konkreten Tastenkombinationen
- Einen Screen-Reader-Hinweis-Kasten, wo das Verhalten für NVDA/JAWS/VoiceOver explizit beschrieben wird
- Eine Zusammenfassung am Ende

#### `user/src/barrierefreiheit.md`

Dedizierte Seite für Screen-Reader-Nutzer:
- Welche Screen Reader werden unterstützt und wie gut
- Warum kein Wechsel in den Browse-Modus nötig ist (`role="application"`)
- Navigationsmuster (Outlook-Modell) erklärt
- Tipps für NVDA, JAWS, VoiceOver und Narrator jeweils separat
- Bekannte Einschränkungen und Workarounds

### 24.5 Hosting: GitHub Pages mit Astro Starlight

Landing Page **und** Dokumentation liegen als **eine** Astro-Starlight-Site im Verzeichnis `web/` und werden über GitHub Pages ausgeliefert. Das ersetzt die früheren vier separaten mdBooks.

**Warum Starlight:** barrierefreies, tastatur- und screenreaderfreundliches Standard-Theme; native Zweisprachigkeit (i18n) mit Sprachumschalter und Fallback; eingebaute Volltextsuche (Pagefind, läuft im Browser); Markdown/MDX-Inhalte; zusätzlich freie Landing-/Rechtsseiten im selben Projekt. Ein Build und ein Deploy decken so Marketing-Landing (für die OAuth-Verifizierung nötig), Rechtsseiten und alle Docs ab.

**Struktur (`web/src/content/docs/`):**

```
web/
├── astro.config.mjs            # Starlight-Konfiguration (i18n, Sidebar, Base)
└── src/content/docs/
    ├── index.mdx               # Splash-Landing (en); de unter de/index.mdx
    ├── privacy.md / terms.md / impressum.md   # Rechtsseiten (en; de unter de/)
    ├── guides/                 # Benutzerhandbuch (Englisch, Root-Locale)
    ├── developers/             # Entwickler-Doku (Englisch)
    ├── plugins/                # Plugin-Entwicklung (Englisch)
    └── de/guides/              # Benutzerhandbuch (Deutsch)
```

**i18n:** `en` ist die Root-Locale (Auslieferung ohne Präfix), `de` die zweite (`/de/…`). Nur das Benutzerhandbuch ist zweisprachig; Entwickler-/Plugin-Doku sind Englisch und fallen für `de`-Besucher automatisch auf Englisch zurück.

**Deployment:** `.github/workflows/docs.yml` baut bei jedem Push auf `web/**` die Site (`npm ci && npm run build`) und deployt `web/dist` nach GitHub Pages. Interimsziel ist der Projektpfad `https://timtam.github.io/aperio/` (daher `base: '/aperio/'`); ein „Diese Seite bearbeiten"-Link je Seite zeigt direkt auf GitHub.

**Erreichbare URLs:**

```
https://timtam.github.io/aperio/              → Landing
https://timtam.github.io/aperio/guides/       → Benutzerhandbuch (Englisch)
https://timtam.github.io/aperio/de/guides/    → Benutzerhandbuch (Deutsch)
https://timtam.github.io/aperio/developers/   → Entwickler-Doku
https://timtam.github.io/aperio/plugins/      → Plugin-Entwicklung
```

Beim Wechsel auf eine eigene Domain (für die OAuth-Verifizierung ohnehin nötig) genügt eine Config-Änderung (`SITE`/`BASE`); Details in `web/README.md`.

### 24.6 Pflege & Aktualität

- Dokumentation liegt im selben Repository wie der Code – kein separates Wiki
- PRs, die neue Features einführen, müssen die zugehörige Dokumentation mitliefern (wird im PR-Template als Checkliste vermerkt)
- Nutzer-Dokumentation wird bei jedem Release auf Aktualität geprüft
- "Diese Seite bearbeiten"-Links auf jeder Doku-Seite senken die Hürde für Community-Beiträge
- Versionierung: Dokumentation wird gemeinsam mit dem Code im selben Repository (`web/`) gepflegt und mitversioniert

---

## 25. Offene Punkte & Ausstehend

> **Hinweis:** Eine laufende, gegen den Code abgeglichene Aufgabenliste der noch
> offenen *Implementierungs*-Baustellen liegt in [`TODO.md`](TODO.md). Die Tabelle
> hier verfolgt den **Spezifikations**-Status; `TODO.md` verfolgt, was davon im
> Code tatsächlich umgesetzt ist.

Die folgenden Punkte sind noch nicht vollständig spezifiziert und werden in zukünftigen Iterationen ergänzt:

| Punkt | Status | Notiz |
|---|---|---|
| **Visuelles Design / Layout** | ⏳ Ausstehend | Wird vom Auftraggeber nachgeliefert |
| **Farbpalette & Theming** | ⏳ Ausstehend | Abhängig von Design-Spezifikation |
| **Icon-Set** | ⏳ Ausstehend | Barrierefreie Icons (alle mit `aria-label` oder `aria-hidden`) |
| **App-Name & Plugin-Dateiendung** | ✅ Festgelegt | App heißt **Aperio**, Plugin-Endung `.aperio` |
| **Barrierefreiheit** | ✅ Spezifiziert | Siehe Abschnitt 3 |
| **Architektur & Tech-Stack** | ✅ Spezifiziert | Siehe Abschnitte 2 & 4 |
| **Adapter-Architektur** | ✅ Spezifiziert | Siehe Abschnitt 6 – Kalender-, Aufgaben-, Kontakt-Adapter |
| **Videokonferenz-Integration** | ✅ Spezifiziert | Siehe Abschnitt 11 |
| **Feiertage** | ✅ Spezifiziert | Über iCal-Abonnement abgedeckt — siehe Abschnitt 12 |
| **Self-Update** | ✅ Spezifiziert | Siehe Abschnitt 21 |
| **Benachrichtigungs-System** | ✅ Spezifiziert | Siehe Abschnitt 14 |
| **Systemintegration (.ics, webcal://, .aperio)** | ✅ Spezifiziert | Siehe Abschnitt 17 |
| **Suchfunktion** | ✅ Spezifiziert | Siehe Abschnitt 13 |
| **Aufgaben-Management** | ✅ Spezifiziert | Siehe Abschnitt 9 – Kern-Feature gleichrangig mit Kalendern |
| **Geräteübergreifende Synchronisation** | ✅ Spezifiziert | Siehe Abschnitt 19 |
| **Plugin-System** | ✅ Spezifiziert | Siehe Abschnitt 20 |
| **Kontakte & CardDAV** | ✅ Spezifiziert | Siehe Abschnitt 10 |
| **Tastaturkürzel & Anpassung** | ✅ Spezifiziert | Siehe Abschnitt 15.7 (Referenz) und 15.10 (Anpassung) |
| **Dokumentation** | ✅ Spezifiziert | Siehe Abschnitt 24 |
| **Import / Export (.ics)** | 🔲 Geplant | Import: Abschnitt 17.1; Export noch auszuarbeiten |
| **Drucken** | 🔲 Geplant | Druckfreundliche Kalenderansichten |
| **Mobile Companion App** | ❓ Optional | Siehe Abschnitt 25.1 – strategische Abwägung dokumentiert |
| **Thunderbird-Integration** | ❓ Optional | Via CalDAV möglich |

### 25.1 Mobile-Strategie (strategische Abwägung)

Eine spätere Portierung auf iOS und/oder Android ist denkbar, aber noch nicht Teil des aktuellen Scopes. Die folgende Abwägung hält den Stand der Evaluierung fest.

#### Tauri Mobile (Stand Mai 2026)

Tauri 2.x ist seit Ende 2024 stabil und unterstützt iOS und Android als reguläre Build-Targets. Damit wäre eine Wiederverwendung von Rust-Backend und Web-Frontend grundsätzlich möglich. Folgende Einschränkungen sind jedoch zu beachten:

- Nicht alle offiziellen Tauri-Plugins sind vollumfänglich auf Mobile portiert; Funktionsumfang sollte vor einer Mobile-Portierung pro genutztem Plugin verifiziert werden
- Barrierefreiheit über VoiceOver (iOS) und TalkBack (Android) ist mit Tauri-Apps noch wenig in der Breite erprobt – für die Kernanforderung dieses Projekts ein kritischer Punkt
- Die Mobile-spezifischen UI-Anpassungen (Touch-Bedienung, Bildschirmgrößen) müssen vollständig neu entwickelt werden – das Web-Frontend ist auf Desktop-Tastatur und -Maus optimiert
- `tauri-action` (offizielle CI/CD-Integration) unterstützt mittlerweile auch Mobile-Builds

#### Szenarien für eine spätere Mobile-Portierung

| Szenario | Ansatz | Aufwand | Bewertung |
|---|---|---|---|
| **A – Tauri Mobile abwarten** | Mobile-Plugin-Reife für relevante Funktionen abwarten; Rust-Backend vollständig wiederverwendbar, UI für Mobile neu entwickeln | Gering (wenn Plugins stabil) | Bevorzugt, falls Mobile kein kurzfristiges Ziel |
| **B – Flutter-Frontend** | Flutter ersetzt das Web-Frontend; Rust-Backend bleibt via FFI/IPC erhalten | Hoch (Frontend-Neuentwicklung) | Sinnvoll bei ernsthafter Mobile-Priorisierung |
| **C – Separate Mobile-App** | Desktop bleibt Tauri; Mobile wird als eigenständige React-Native- oder Flutter-App gebaut, die dasselbe Rust-Backend über eine gemeinsame API nutzt | Sehr hoch | Sauberste Trennung, größter Aufwand |

#### Entscheidung (aktuell)

Es wird vorerst **Szenario A** verfolgt: Die Desktop-App wird mit Tauri entwickelt, ohne Kompromisse für Mobile einzugehen. Die saubere Workspace-Architektur mit unabhängigen Crates stellt sicher, dass das gesamte Rust-Backend unabhängig vom gewählten Mobile-Ansatz wiederverwendet werden kann.

Diese Entscheidung wird mit dem Reifegrad der Mobile-Tauri-Plugins für die für dieses Projekt relevanten Funktionen (Barrierefreiheit, Updater, Filesystem) regelmäßig neu bewertet.

---
