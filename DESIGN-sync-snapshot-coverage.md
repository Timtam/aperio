# Schnappschuss nennt, was er enthält

Stand 2026-10-30 (Befund 2026-10-27). Befund von Toni: Ein neues Gerät (Laptop) trat dem Sync
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

### B. Beitreten, Wiederaufnehmen und Nachholen: ein Durchgang

`accept_remote`, `resume_from_stale` und das einmalige Nachholen in der Runde
enden im selben Durchgang (`OnboardingService::apply_every_log`):

1. Beitritt und Wiederaufnahme wenden zuerst den Schnappschuss an (Upsert wie
   bisher). Nennt er Kennungen, werden sie im selben Zug als angewendet
   eingetragen.
2. **Alle** vorhandenen Protokolldateien holen, eigene eingeschlossen, dazu
   die eigenen noch nicht hochgeladenen (`pending/`). Jede Änderung zählt
   einmal (nach Kennung).
3. Alle Änderungen **gemeinsam** nach Zeit anwenden, nicht Datei für Datei:
   Sitzungen verschiedener Geräte überlappen sich.
4. Eine Änderung gilt nur, wenn sie neuer ist als ihre Zeile (siehe E), ohne
   Konfliktmeldung. Schon Angewendetes wird erneut gewogen. So bekommt eine
   Zeile, die ein Schnappschuss zurückgesetzt hat, ihre neuere Änderung zurück.
5. Längen der fremden Dateien merken (eine unlesbare mit 0, die nächste Runde
   versucht sie wieder). Cursor = Maximum aus bisherigem Cursor (beim Beitritt
   und bei der Wiederaufnahme: Schnappschuss) und neuester fremder Datei, nie
   zurück.
6. Danach ist das Gerät **genau** (`sync.cursor.coverageExactSince`).

Erst holen, dann ändern: Ein abgebrochener Abruf lässt den Bestand unberührt.
Das löst auch, was „Ursache“ gegen das Wiederabspielen einwandte: Es geschieht
still, und nur Neueres gewinnt.

### C. Gewöhnliche Runden

- **Einmal nachholen.** Ein Gerät, das nicht genau ist, holt in der Runde
  einmal alles nach (B). Das betrifft jedes Gerät, das vor dieser Korrektur
  beigetreten oder wiederaufgenommen ist, und eines nach „Neu beginnen“.
  Schlägt es fehl, versucht die nächste Runde es wieder; die Runde selbst
  läuft weiter.
- **Unbekanntes holen.** Ein genaues Gerät holt auch eine fremde Datei, deren
  Name nicht nach dem Cursor liegt und deren Länge es nicht kennt
  (`DeviceCursor::fetch_unknown`), ohne Untergrenze. Das ist eine verspätet
  hochgeladene Sitzung, oder eine nach dem Aufräumen erneut hochgeladene mit
  Namen unter `gc_horizon`. Ein nicht genaues Gerät behält die alte Regel, bis
  es nachgeholt hat.
- **Gemerkte Längen.** Die 256 neuesten (nach Namen), nicht nach `gc_horizon`
  beschnitten: Eine noch laufende Sitzung mit altem Namen muss ihr Wachstum
  weiter zeigen. Grenze: Liegen mehr als 256 Dateien vor, ist die älteste jede
  Runde „unbekannt“ und wird erneut geholt. Das ist richtig, kostet aber
  Bandbreite.

### D. Kennungen im Schnappschuss

Der Compactor nennt die gehaltenen Kennungen nur, wenn das Gerät **genau** ist
und jedes Ereignis in jeder vorhandenen Datei hält (eigenes oder angewendet).
Sonst nennt er nichts. Kennungen sparen einem beitretenden Gerät Arbeit und
schützen Zeilen ohne bekannte Zeit (E). Nötig sind sie nicht mehr: Das
Nachholen heilt auch ohne sie.

### E. Zeilen-Uhr: Neuer gewinnt

`sync_row_clock` (Migration 49) hält je Zeile zwei Zeiten:

- `changed`: die neueste Änderung jeder Art.
- `written`: die neueste, die die ganze Zeile schrieb (Anlegen, ganze Zeile,
  Löschen). Leer, solange nur Feld-Zusammenführungen die Zeile berührten.

Der Writer stempelt eigene Änderungen beim Schreiben, der Anwender jede
übernommene. Eine eigene Änderung ist immer neuer als alles, was ihre Zeile
gesehen hat: Geht die Uhr dieses Geräts nach, bekommt sie einen Stempel knapp
über der Uhr der Zeile, sonst übersprängen die anderen sie dauerhaft. Die Zeit einer Zeile ist `changed`. Bei Terminen und Aufgaben ist
sie mindestens ihr eigenes `updated_at`.

Ein Schnappschuss trägt die Uhren seines Autors (`row_clocks`). Wer ihn
übernimmt, stempelt sie, jede nie zurück, aber nur, wenn alle seine Zeilen
geschrieben wurden; sonst trägt er auch keine Kennungen ein. Ein Beitritt
beginnt mit leeren Uhren: Die eines früheren Datensatzes hielten die
Änderungen des neuen als älter draußen.

- **Ganze Zeile und Anlegen:** Älter als die Zeile wird übersprungen
  (`skipped_older`) und als angewendet vermerkt.
- **Feld-Zusammenführung** (Termin, Aufgabe, Liste, Kalender, Farbe): wägt die
  Zeit der Zeile wie bisher `updated_at`. Ältere Felder ergeben einen
  Konflikt; beim Nachholen bleibt die neuere Zeile still. Das `updated_at`
  einer älteren Zusammenführung bleibt draußen, sonst verlöre die Zeile ihre
  Zeit. Fehlt die Zeile, obwohl ihre Uhr eine Schreibung der ganzen Zeile
  kennt, wurde sie gelöscht: Die Zusammenführung bleibt draußen und legt sie
  nicht neu an.
- **Löschen:** verliert nur gegen eine neuere Schreibung der ganzen Zeile,
  nicht gegen eine neuere Feldänderung. Eine Feldänderung kann eine gelöschte
  Zeile auf dem löschenden Gerät nicht zurückholen. Verlöre das Löschen,
  liefen die beiden Geräte dauerhaft auseinander. Ein älteres Anlegen nach dem
  Löschen bleibt gelöscht.

**Übergang.** Die Uhr beginnt leer. Eine Zeile ohne bekannte Zeit (keine Uhr,
kein `updated_at`) kann einen neueren Stand aus einer schon gelöschten Datei
halten. Beim Nachholen wird darum für so eine Zeile eine schon angewendete
oder eigene Änderung, ein Löschen eingeschlossen, **nicht** erneut
angewendet; ihre Zeit wird die Uhr der Zeile. Eigene Änderungen gelten als
angewendet, denn die Zeilen des Geräts halten sie. Erst wenn das Nachholen die Zeile selbst ändert, mit einer nie
angewendeten Änderung, werden spätere angewendete wieder gewogen. Grenze: Eine
nie angewendete ältere Änderung auf so einer Zeile gewinnt noch, wie in jeder
Runde zuvor.

## Was nicht Teil davon ist

- Lokale Termine und Kalender werden beim Start nicht nachgetragen (TODO B3).
  Was nie im Protokoll stand, bringt das Nachholen nicht, löscht es aber auch
  nicht.
- Echte Gleichzeitigkeit (Vektoruhren) bleibt wie in §19.3: Bei gleich alten
  Änderungen verschiedener Geräte entscheidet die Reihenfolge.
- Zwischen dem neuen Auflisten und dem Löschen bleibt ein kurzes Fenster:
  Hängt ein Gerät genau dann an, geht das Angehängte verloren. Ohne Sperre im
  Sync-Speicher lässt sich das nicht schließen.
- Varianten, die diese Version nicht anwendet (Tastenkürzel), gelten als
  nicht gehalten: Eine Datei mit ihnen bliebe stehen, und das Gerät nennte
  keine Kennungen. Noch schreibt kein Gerät solche Ereignisse.

## Schritte

- C1 bis C5, die erste Fassung mit Neuaufbau: nach Prüfrunde 1 ersetzt.
- R1 Längen: die 256 neuesten, kein Beschnitt nach Horizont. Erledigt.
- R7 Zeilen-Uhr, mit zwei Zeiten, Löschen und Übergang (E). Erledigt.
- R3 Kennungen nur, wenn genau und vollständig. Erledigt.
- R4 und R5 Nachholen statt Neuaufbau, ein Durchgang für Beitritt,
  Wiederaufnahme und Runde. Erledigt.
- R6 Unbekanntes ohne Untergrenze. Erledigt.
- R2 Aufräumen: nach dem Schnappschuss neu auflisten, nur ganz Gehaltenes
  löschen, Leeres und Unlesbares nie, das Ende ausstehender Sitzungen vor dem
  Wegräumen hochladen (scheitert es, bleibt die Datei). Erledigt.
- Doku, dann Prüfrunde 2.

Je Schritt Tests und Gegenproben; `cargo test -p cal-ffi` bei jeder Änderung an
Compactor oder Runde (Lehre aus dem Compactor-Bogen).

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

## Prüfrunde 2 (PR #130, 2026-10-30)

Gezielt: 4 Prüfer, die 10 schwersten Befunde gegengeprüft (alle bestätigt),
18 weitere selbst nachgeprüft, ein Kritiker. Einzelheiten: scratchpad
`coverage/review_r2.json`.

**Die Zeit der Zeile hält nicht**
- Ein Schnappschuss schreibt Zeilen, aber keine Uhren. Nach Beitritt oder
  Wiederaufnahme gewinnt dann eine ältere Änderung gegen seinen neueren
  Inhalt.
- Eine ältere Feld-Zusammenführung übernimmt ihr `updated_at` als Metadatum.
  Die Zeile verliert ihre Zeit, und die nächste ältere Änderung gewinnt.
- Eigene Ereignisse stehen nie in `sync_applied_events`. Die Übergangsregel
  schützt darum nicht vor den eigenen älteren Änderungen.
- Ein Gerät mit nachgehender Uhr schreibt eine spätere Änderung mit älterem
  Stempel; die anderen überspringen sie dauerhaft.

**Löschen gegen Bearbeiten:** Eine Feld-Zusammenführung auf eine fehlende
Zeile legt sie neu an. Das löschende Gerät bekommt die Zeile zurück, das
bearbeitende löscht sie: Sie laufen auseinander, entgegen Abschnitt E.

**Aufräumen:** Der Compactor löscht eine Datei, deren Ende ein anderes Gerät
noch nicht geholt hat, auch die eigene gleich nach dem Hochladen. Er löscht,
was ein Gerät vor seinem Nachholen braucht (der Laptop). Er vertraut seinem
eigenen Protokoll, auch wenn es nicht genau ist. Das Hochladen vor dem
Wegräumen umgeht den Push und lädt nach dem Abschalten von E2E Zugangsdaten
im Klartext hoch.

**Weiteres:** Der Bericht des Nachholens geht verloren (Ansichten laden nicht
neu). Ein Schnappschuss mit fehlgeschlagenen Zeilen trägt trotzdem seine
Kennungen ein. Uhren eines früheren Datensatzes bleiben beim Beitritt.
„Neu beginnen“ lässt die alten Dateien liegen, und das Nachholen holt sie
herein.

## Entscheidungen nach Prüfrunde 2 (Toni, 2026-10-30)

- **Aufräumen: warten und 14 Tage.** Der Compactor löscht nichts, solange ein
  aktives Gerät (nicht als veraltet markiert) noch nicht nachgeholt hat; jedes
  Gerät meldet das in `meta.json`. Und er löscht eine Datei erst, wenn ihr
  jüngster Eintrag älter ist als das Aufbewahrungsfenster (14 Tage). Im
  Sync-Ordner liegen damit immer mindestens 14 Tage Protokoll.
- **„Neu beginnen“ löscht die Protokolle und den Schnappschuss** des alten
  Datensatzes.
- **Das Klartext-Leck wird hier behoben:** Der Push entfernt
  Zugangsdaten-Ereignisse, solange E2E aus ist, an einer Stelle für alle
  Wege. Der Compactor lädt nichts mehr selbst hoch; eine ausstehende Datei,
  deren Ende noch fehlt, bleibt für den nächsten Push liegen.
- Ohne Rückfrage, weil es Abschnitt E erst wahr macht: Der Schnappschuss trägt
  die Uhren seines Autors (und nur, wenn alle Zeilen geschrieben wurden);
  Metadaten einer älteren Zusammenführung bleiben draußen; eigene Ereignisse
  gelten beim Nachholen als gehalten; eine Zusammenführung auf eine gelöschte
  Zeile bleibt draußen; eine eigene Änderung ist immer neuer als ihre Zeile;
  der Beitritt beginnt mit leeren Uhren; der Bericht des Nachholens geht in
  die Runde ein.
