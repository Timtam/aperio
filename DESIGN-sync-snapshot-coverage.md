# Schnappschuss nennt, was er enthält

Stand 2026-10-27. Befund von Toni: Ein neues Gerät (Laptop) trat dem Sync
bei, übernahm den Schnappschuss vom 19.10. und alle Konten. Aufgaben, die seit
dem 19.10. erledigt oder neu angelegt wurden, fehlten oder standen weiter als
überfällig da.

## Befund (Protokoll des Laptops, Sync-Ordner auf dem NAS)

- Schnappschuss `snapshot_timestamp` = 2026-10-19T16:07:24Z, erstellt vom
  iPhone beim Start einer Sitzung (sein eigener Sitzungsstempel war der
  Inhalts-Horizont `max(own_newest_log, cursor)`).
- `accept_remote` setzte den Cursor auf diesen Zeitpunkt und holte nur
  Dateien mit späterem Namen: genau 5 (22.10. bis 27.10. früh), 13
  Änderungen. Spätere Runden: `fetched=0`.
- Die Desktop-Sitzung `2026-10-19T07-58-29Z_ab0d…` begann vor dem
  Schnappschuss und lief bis zum 22.10. Was danach in sie geschrieben wurde,
  steht in einer Datei mit „altem“ Namen. Der Laptop übersprang sie.

## Ursache

Der Abruf entscheidet nach dem **Namen** einer Datei (Sitzungsbeginn), der
Schnappschuss deckt aber **Ereignisse** ab. Eine Sitzungsdatei wächst nach
ihrem Namen weiter:

1. **Beitritt und Wiederaufnahme** (`accept_remote`, `resume_from_stale`):
   Cursor = Schnappschusszeit, keine bekannten Längen. Eine fremde Datei mit
   Namen vor dem Schnappschuss, die danach wuchs, wird nie geholt.
2. **Gewöhnliche Runde**: `DeviceCursor::wants_sized` holt eine Datei nur,
   wenn ihr Name nach dem Cursor liegt oder sie schon angewendet wurde und
   gewachsen ist. Eine Datei, die ein Gerät **verspätet** hochlädt (offline
   geschrieben) und deren Name vor dem Cursor liegt, holt kein anderes Gerät
   je („Unknown filename … never refetched“).

Alles erneut abzuspielen geht nicht: Der Anwender führt pro Zeile nach
`updated_at` zusammen. Ein altes, schon im Schnappschuss enthaltenes Ereignis
mit einem inzwischen geänderten Wert würde als Konflikt gemeldet.

## Entscheidungen (Toni, 2026-10-27)

- Der Schnappschuss nennt die **Kennungen der Ereignisse**, die er schon
  enthält, soweit sie noch in Protokolldateien liegen.
- Eigener PR, vor PR 7 Runde 15.
- Der Laptop wird nicht von Hand neu aufgesetzt, die Korrektur heilt ihn.

## Entwurf

### A. Der Compactor schreibt die Abdeckung

Vor dem Bau des Schnappschusses holt der Compactor alle Protokolldateien
(`DeviceCursor::epoch()`; das tut er heute erst danach, fürs Aufräumen, und
nutzt denselben Abruf dann für beides). Abgedeckt ist ein Ereignis in einer
dieser Dateien, wenn es

- von diesem Gerät stammt (seine Datenbank enthält jede eigene Änderung), oder
- in `sync_applied_events` steht (angewendet).

Diese Kennungen kommen als `covered_event_ids` in den Rumpf des Schnappschusses
(`AperioSnapshotBody`, `serde(default)`). Ein Ereignis, das in einer Datei
steht, aber noch nicht angewendet ist (gewachsen seit dem letzten Abruf), ist
nicht abgedeckt: Ein neues Gerät wendet es an, der Compactor selbst holt es mit
der nächsten Runde nach.

### B. Beitreten und Wiederaufnehmen nutzen sie

`accept_remote` und `resume_from_stale`:

1. Schnappschuss anwenden (wie heute).
2. `covered_event_ids` in `sync_applied_events` eintragen, im selben Zug.
   Nur zusammen mit dem Anwenden ist das richtig: Die Wirkung dieser
   Ereignisse steht dann in den Zeilen.
3. **Alle** vorhandenen fremden Dateien holen (nicht erst ab dem
   Schnappschuss), zeitlich sortiert anwenden. Abgedecktes überspringt die
   Idempotenz, der Rest wird angewendet.
4. Längen aller angewendeten Dateien merken, Cursor = max(Schnappschuss,
   neueste Datei).
5. Merken, welcher Schnappschuss übernommen wurde und ob er Kennungen trug
   (`sync.snapshot.consumed` = Zeitpunkt + `has_coverage`).

### C. Gewöhnliche Runden holen Unbekanntes

`wants_sized`: Eine fremde Datei, deren Name nicht nach dem Cursor liegt und
für die keine angewendete Länge gemerkt ist, wird **geholt** (bisher: nie),
sofern ihr Name nicht vor `meta.gc_horizon` liegt (`DeviceCursor` bekommt
dafür eine Untergrenze, `serde(default)` = altes Verhalten). Damit kommt eine
verspätet hochgeladene Datei an. Das Wiederanwenden ist durch die Idempotenz
sicher. Einmalig nach dem Neuaufbau lädt jedes Gerät dafür die vorhandenen
Dateien, deren Länge es noch nicht kennt (heute 19 Dateien, rund 70 KB).

**Nur auf einem genauen Gerät.** Ein Gerät, dessen `sync_applied_events` nicht
genau sagt, was sein Bestand enthält (beigetreten über einen Schnappschuss ohne
Kennungen), würde damit alte, schon enthaltene Ereignisse erneut anwenden und
falsche Konflikte melden. Die Regel gilt deshalb erst, wenn das Gerät **genau**
ist: beigetreten oder neu aufgebaut aus einem Schnappschuss mit Kennungen, oder
selbst einen solchen geschrieben (`sync.coverage.exactSince`).

**Gemerkte Längen.** Bisher die 64 neuesten Dateien; eine herausgefallene ältere
Datei wäre mit der neuen Regel jede Runde wieder „unbekannt“. Stattdessen fallen
Einträge mit Namen vor `gc_horizon` heraus (die Dateien sind gelöscht oder
abgedeckt); eine große Sicherheitsgrenze bleibt.

### D. Heilen: jedes Gerät baut einmal neu auf (Toni, 2026-10-27)

Geräte, die vor der Korrektur beigetreten oder wiederaufgenommen sind, lassen
sich nicht sicher erkennen: Kein Gerät merkte sich, welchen Schnappschuss es
übernahm, und nach einer neuen Verdichtung sind ihm fehlende Dateien vielleicht
schon gelöscht. Toni: kein festes Datum, kein Erkennen, sondern **jedes Gerät
baut einmal neu auf**, sobald ein Schnappschuss mit Kennungen da ist.

**Wer den ersten schreibt.** Ein betroffenes Gerät kennt nicht genau, welche
Ereignisse sein Bestand enthält (ein Teil kam über einen Schnappschuss ohne
Kennungen). Darum schreibt der Compactor Kennungen nur, wenn das Gerät
**vollständig** ist: Jedes Ereignis in jeder vorhandenen Protokolldatei stammt
von ihm oder steht in `sync_applied_events`. Sonst verdichtet er wie bisher,
ohne Kennungen. `meta.json` sagt, ob der aktuelle Schnappschuss Kennungen trägt
(`snapshot_coverage`, `serde(default)`), damit ein vollständiges Gerät nach dem
Update von selbst einmal verdichtet, wenn es noch keinen solchen gibt.
Bei Toni ist das der Desktop. Gibt es kein vollständiges Gerät mehr (Gründer
weg, alle über alte Schnappschüsse beigetreten), bleibt der Datensatz beim
alten Verhalten — sagen, nicht still.

**Der Neuaufbau** (einmal je Gerät; danach ist es „genau“,
`sync.cursor.coverageExactSince`). Erst alles holen, dann lokal arbeiten — ein
abgebrochener Abruf lässt den Bestand unberührt:

1. Schnappschuss (er muss Kennungen tragen) und **alle** Protokolldateien
   holen, auch die eigenen.
2. `sync_applied_events` leeren. Was das Gerät früher angewendet hat und der
   Schnappschuss nicht enthält, muss erneut angewendet werden: Der Schnappschuss
   kann von einem Gerät stammen, das in einer Zeile hinter diesem lag.
3. Schnappschuss anwenden (Upsert, wie bisher) — das trägt seine Kennungen ein.
4. **Gelöschtes:** Eine abgedeckte Lösch-Änderung (`*.deleted`,
   `event_group.dissolved`, `account.deleted`), deren Zeile der Schnappschuss
   nicht enthält, wird wieder als nicht angewendet geführt
   (`AperioSnapshotBody::deletes_to_replay`). So entfernt Schritt 5 eine
   übrig gebliebene Zeile; wo sie fehlt, tut die Löschung nichts.
   **Nicht** alles leeren, was der Schnappschuss nicht enthält: Lokale Termine
   und Kalender werden beim Start nicht nachgetragen; ein Gerät kann Zeilen
   haben, die nie ins Protokoll kamen, und die wären verloren.
5. Alle Ereignisse aller Dateien anwenden, eigene eingeschlossen
   (`apply_envelopes_force_own`, zeitlich sortiert, die Idempotenz überspringt
   das Abgedeckte); dann die eigenen noch nicht hochgeladenen (`pending/`).
6. Längen der fremden Dateien merken, Cursor = max(Schnappschuss, neueste
   fremde Datei), „genau“ ab dem Schnappschuss.

Dem Laptop kommt so die Desktop-Sitzung vom 19.10. zu, Löschungen
eingeschlossen. `resume_from_stale` wird derselbe Neuaufbau, wenn der
Schnappschuss Kennungen trägt (sonst wie bisher). Grenze: Eine Löschung, deren
Datei schon aufgeräumt ist, kann nicht mehr nachgeholt werden.

**Aufräumen wird genau.** Der Compactor löscht eine fremde Datei nur noch,
wenn jedes Ereignis darin abgedeckt ist (bisher: Name bis zum Cursor). Ein
betroffenes Gerät, das verdichtet, kann so keine Datei löschen, deren Rest es
nie angewendet hat.

## Was nicht Teil davon ist

- Beitreten mit eigenen lokalen Daten (`accept_remote` auf einem Gerät, das
  schon etwas hat) leert nichts: Es führt zusammen wie bisher, nur mit
  Kennungen und allen Dateien.
- Echte Gleichzeitigkeit (Vektoruhren) bleibt wie in §19.3.

## Schritte

- C1 Compactor: alle Dateien vor dem Bau, Vollständigkeit, `covered_event_ids`
  im Rumpf, `meta.snapshot_coverage`, genaues Aufräumen; ein vollständiger
  Compactor wird „genau“ (sync-engine).
- C2 Beitreten: Kennungen eintragen, alle Dateien holen, „genau“ (host-core).
- C3 Neuaufbau als ein Weg für Wiederaufnahme und einmaliges Heilen;
  Auslöser in der Runde; einmalige Verdichtung eines vollständigen Geräts.
- C4 `wants_sized` holt Unbekanntes auf genauen Geräten (sync-core,
  Orchestrator), gemerkte Längen nach `gc_horizon` statt 64 neueste.
- C5 Handy (cal-ffi) mitziehen, Doku (DESIGN-sync-engine.md, Entwickler-Doku),
  TODO.

Je Schritt Tests und Gegenproben; `cargo test -p cal-ffi` bei jeder Änderung an
Compactor oder Runde (Lehre aus dem Compactor-Bogen). Prüfrunde mit
Gegenprüfern vor dem Merge.

## Prüfrunde 1 (PR #130, 2026-10-30)

4 Prüfer + Gegenprüfer + Kritiker (40 Agenten): 35 bestätigt, 0 widerlegt,
dazu 7 vom Kritiker. Einzelheiten: scratchpad `coverage/review_r1.json`.

**Kritisch**
- Gemerkte Längen nach `gc_horizon` beschnitten: Eine noch laufende Sitzung mit
  altem Namen fällt heraus, ihr Wachstum wird nie mehr geholt — der
  Laptop-Fehler kehrt zurück, auch beim Beitritt (dreimal gefunden).
- „Vollständig“ ist nicht beweisbar: Der Compactor sieht nur vorhandene
  Dateien. Fehlen einem Gerät Ereignisse aus schon gelöschten Dateien, oder hat
  ein alter Schnappschuss (Upsert bei der Wiederaufnahme) Zeilen zurückgesetzt,
  deren Ereignisse als angewendet vermerkt bleiben, gilt es trotzdem als
  vollständig — sein Schnappschuss würde Grundlage aller Neuaufbauten.
- Der Neuaufbau setzt zurück: Was ein Gerät mehr wusste als der Autor des
  Schnappschusses, ist verloren, wenn die Dateien schon gelöscht sind.
- Verspätete Dateien unter `gc_horizon` holt niemand; erneut hochgeladene alte
  Sitzungen ebenso.

**Schwer:** Beitritt wendet ganze Dateien nach Namen an statt Ereignisse nach
Zeit; Aufräumen auf einer Liste von vor dem Schnappschuss (Wachstum dazwischen
gelöscht), leere/abgerissene Lesungen; das Ende der rotierten Sitzung wird
weggeräumt, ohne hochgeladen zu sein; alte Builds löschen das Meta-Feld.

## Umbau nach Prüfrunde 1 (Toni, 2026-10-30)

- **Nachholen statt ersetzen.** Kein Neuaufbau aus dem Schnappschuss. Jedes
  Gerät, das nicht genau ist, holt **einmal** alle vorhandenen Dateien nach,
  eigene eingeschlossen, in Zeitreihenfolge; eine Änderung gilt nur, wenn sie
  neuer ist als die Zeile, ohne Konfliktmeldung, Löschungen ebenso. Danach ist
  es genau. So wird nichts zurückgesetzt; es repariert auch Zeilen, die ein
  alter Schnappschuss zurückgesetzt hat, solange die Ereignisse noch in Dateien
  liegen. Neuaufbau, Auto-Verdichtung, `meta.snapshot_coverage` und
  `deletes_to_replay` entfallen.
- **Neuer gewinnt auch ohne Zeilenzeit** (Toni: gleich mitbeheben). Wo eine
  Tabelle `updated_at` hat, setzt das Anwenden sie auf die Zeit des Ereignisses
  und vergleicht damit; ein älteres Ereignis gewinnt nicht mehr — auch nicht,
  wenn es spät ankommt (offline geschrieben, gewachsene Datei).
- **Längen:** die 256 neuesten behalten, nicht nach Horizont beschneiden.
- **Aufräumen:** nach dem Schnappschuss neu auflisten, nur löschen, was mit
  aktuellem Inhalt ganz gehalten wird; leere oder unlesbare Dateien nie; das
  Ende der rotierten Sitzung vor dem Wegräumen hochladen.
- **Kennungen** schreibt nur ein Gerät, das genau ist UND alles hält.
- **Beitritt und Nachholen** wenden die Ereignisse aller Dateien gemeinsam
  nach Zeit an.
- **Unbekannte Dateien** holt ein genaues Gerät ohne Untergrenze.
