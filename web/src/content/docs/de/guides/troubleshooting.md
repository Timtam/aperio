---
title: "Fehlersuche & Protokolle"
---

Wenn etwas nicht wie erwartet funktioniert, sind Aperios Protokolle der
schnellste Weg zur Ursache. Aperio führt auf deinem Gerät eine rollierende
Protokolldatei — auch im normalen (Release-)Build — die du exportieren und
einem Fehlerbericht beilegen kannst.

## Der Protokolle-Bereich

Öffne **Einstellungen → Protokolle**. Dort kannst du:

- **Den Detailgrad einstellen.** *Normal* ist die Voreinstellung und für den
  Alltag richtig. Wechsle nur zu *Debug* oder *Trace*, während du ein Problem
  nachstellst — sie protokollieren deutlich mehr und machen das Protokoll
  umfangreicher. Die Auswahl wird auf diesem Gerät gemerkt und **nicht** auf
  deine anderen Geräte synchronisiert.
- **Das aktuelle Protokoll ansehen** — die letzten Zeilen der aktuellen
  Protokolldatei, mit einer **Aktualisieren**-Schaltfläche.
- **Das Protokoll in eine Datei exportieren** — Speicherort wählen und dem
  Bericht beilegen.
- **Das Protokoll in die Zwischenablage kopieren** — praktisch zum Einfügen in
  ein Ticket oder einen Chat.
- **Protokolle löschen** — entfernt die gespeicherten Protokolldateien (die
  aktuelle Sitzung protokolliert weiter).

## Datenschutz

Der Export ist zum Teilen gedacht, daher ist **Persönliche Daten entfernen**
standardmäßig aktiv: E-Mail-Adressen und Zugriffstokens werden vor dem
Verlassen des Geräts durch Platzhalter ersetzt. Passwörter, die
Sync-Passphrase oder Konto-Tokens protokolliert Aperio ohnehin nie — die
liegen ausschließlich im Schlüsselbund deines Betriebssystems. Lass die
Schwärzung aktiviert, sofern der Support nicht ausdrücklich ein
ungeschwärztes Protokoll anfordert.

## Wo die Protokolle liegen

Die Protokolldateien liegen in deinem Datenverzeichnis im Ordner `logs/`
(`aperio.log.<Datum>`). Einstellungen → Protokolle zeigt den genauen Pfad mit
einer **Pfad kopieren**-Schaltfläche. Dateien, die älter als 14 Tage sind,
werden automatisch entfernt.

## Ein Konto aktualisiert sich nicht mehr

Wenn ein verbundenes Konto nicht mehr aktualisiert werden kann — meist, weil
das Passwort bzw. App-Passwort geändert oder widerrufen wurde — zeigt Aperio
weiter die zuletzt bekannten Daten und warnt dich, statt still zu scheitern:

- **Desktop:** Das Konto in der Seitenleiste trägt eine Warnung, und eine
  höfliche Screenreader-Ansage verweist auf **Einstellungen → Konten**. Dort
  listet das betroffene Konto jeden fehlschlagenden Kalender bzw. jede Liste,
  den Fehler des Anbieters und den Zeitpunkt der letzten erfolgreichen
  Aktualisierung. Deuten die Fehler auf ein Anmeldeproblem hin, öffnet eine
  Schaltfläche **Passwort neu eingeben** direkt den Verbinden-Dialog.
- **Mobil:** Die Sync-Schaltfläche in der Kopfzeile wird zur Warnung (ihre
  Beschriftung nennt das Problem), die Details stehen im Bereich
  **Synchronisierung**, und das betroffene Konto erhält auf dem
  Konten-Bildschirm eine Schaltfläche **Neu verbinden**, um das Passwort neu
  einzugeben bzw. die Anbieter-Anmeldung zu wiederholen.

Der Satz am Ende einer Aktualisierung nennt, was nicht aktualisiert werden
konnte: „Externe Daten aktualisiert, außer: Arbeit und Zuhause.“ (am Handy
nach jeder Aktualisierung, am Desktop nach einer, die du gestartet hast).
Konnte die Aktualisierung gar nichts lesen, sagt er, dass nichts aktualisiert
werden konnte, und die Warnung mit ihrer Ursache folgt; ein erster, noch
unbestätigter Netzaussetzer sagt nur, dass die Aktualisierung beendet ist. Die genannten Konten
werden nicht ein zweites Mal angesagt, außer ihre Ursache wird schwerer, und
ihre Warnungen erscheinen zusammen mit dem Satz.

Gibt das Betriebssystem die Daten eines Kontos nicht frei — die Kalender des
Telefons ohne Zugriff —, steht beim Konto eine Zeile statt einer pro Kalender
(„Aperio darf die Kalender nicht lesen“), am Handy mit einer Schaltfläche, die
die Konten öffnet, wo **Zugriff erlauben…** steht. Wo Aperio nur eines nennt,
geht fehlender Zugriff einem Anmeldeproblem vor und ein Anmeldeproblem allem
anderen. Ein Konto, das schon ausfällt, wird nur dann noch einmal angesagt,
wenn seine Ursache schwerer wird.

Ein kurzer, einmaliger Verbindungsaussetzer löst die Warnung nicht aus:
Ein Netzwerkfehler wird erst gezeigt, wenn er erneut auftritt — ein
Kaltstart mit noch nicht bereitem Netz erzeugt so keinen Fehlalarm. Ein
Anmeldeproblem, das sich nie von selbst behebt, erscheint sofort, und eine
manuelle Aktualisierung meldet ihr Ergebnis immer unmittelbar, auch in den
ersten Sekunden nach dem Start. Die Warnung verschwindet von selbst, sobald
eine Aktualisierung wieder gelingt.

## „Dieses Gerät“ aktualisiert sich nach dem Umzug auf ein neues Telefon nicht mehr

Beim Umzug auf ein neues iPhone kommen Aperios Daten mit, auch das Konto
**Dieses Gerät**, aber nicht die Erlaubnis, die Kalender und Erinnerungen des
Telefons zu lesen: Danach fragt iOS auf jedem Telefon neu. Bis sie erteilt ist,
zeigt Aperio weiter, was es zuletzt gelesen hat, und das Konto aktualisiert
sich nicht.

Gibt es das Konto und hat iOS auf diesem Telefon noch nie gefragt, fragt
Aperio beim Start, sobald die App entsperrt ist und der erste Bildschirm
geladen hat. Erlaube den vollen Zugriff auf die Kalender und auf die
Erinnerungen; Aperio aktualisiert das Konto dann sofort und sagt es an.

Fehlt der Zugriff, trägt **Dieses Gerät** unter Einstellungen → Konten ein
Abzeichen, das sagt, was fehlt (**Kein Zugriff**, **Kalender ohne Zugriff**
oder **Erinnerungen ohne Zugriff**), und die Aktion **Zugriff erlauben…**. Hat
iOS noch nicht gefragt, fragt es jetzt. Hast du abgelehnt, fragt iOS nicht
noch einmal: Die Aktion sagt dann, was fehlt, und öffnet Aperios Seite in der
App „Einstellungen“. Stelle dort **Kalender** und **Erinnerungen** jeweils auf
**Voller Zugriff** (bis iOS 16: einschalten). „Nur Termine hinzufügen“ reicht
nicht: Aperio muss deine Kalender lesen können. Kommst du zurück zu Aperio,
aktualisiert es das Konto und sagt es an, oder es sagt, wofür der Zugriff noch
fehlt. Verbietet eine Einschränkung wie Bildschirmzeit den Zugriff, sagt Aperio
auch das; ändern lässt er sich dann nur bei dieser Einschränkung.

Unter Android gilt dasselbe für die Kalender. Gibt es das Konto und hat
Aperio auf diesem Telefon noch nie gefragt — auch auf einem neuen —, fragt es
beim Start einmal. Danach fragt die Aktion, solange Android fragt, und führt
sonst zu Aperios Berechtigungen in den Android-Einstellungen.

Es gibt ein Konto **Dieses Gerät** pro Telefon. Wählst du unter Konto
hinzufügen noch einmal **Dieses Gerät**, entsteht kein zweites: Fehlt der
Zugriff, führt es zum selben **Zugriff erlauben…**, sonst sagt es, dass das
Konto schon hinzugefügt ist, und springt zu ihm. Steht **Dieses Gerät** zweimal
da (von einer älteren Version zweimal hinzugefügt), erscheint jeder
Gerätekalender doppelt; lösche eines der beiden.

## Eine Aufgabenzeit hat sich einmalig verschoben

Aufgaben mit einer **Uhrzeit** auf einem **CalDAV**-Konto (iCloud Erinnerungen,
Nextcloud, Radicale, Tasks.org) hat Aperio früher als UTC-Zeit abgelegt, obwohl
die Uhrzeit eine lokale Wanduhrzeit ist. Für Aperio selbst fiel das nicht auf,
weil beim Lesen derselbe Fehler rückwärts gemacht wurde — in jedem anderen
Programm war die Aufgabe aber um deinen Zeitzonen-Abstand verschoben.

Seit dieser Version wird die Uhrzeit als das geschrieben, was sie ist. Aufgaben,
die **Aperio selbst** früher mit Uhrzeit angelegt hat, verschieben sich dadurch
**einmalig** um genau diesen Abstand — eine 09:00-Aufgabe steht in
Mitteleuropa danach auf 11:00. Korrigiere sie einmal, danach bleibt sie stehen.
Aufgaben, die in einem anderen Programm angelegt wurden, waren bisher falsch und
sind jetzt richtig.

Zwei weitere Dinge sind damit behoben: eine Aufgabe, deren Uhrzeit eine
**Zeitzone** mitbringt (so schreiben es Thunderbird, Tasks.org und Nextcloud),
verlor bei uns nicht nur die Uhrzeit, sondern den **ganzen Tag** und lag ohne
Datum im Backlog. Und eine Aufgabe aus **Microsoft To Do**, die jemand in seiner
eigenen Zeitzone angelegt hat, konnte bei uns einen Tag zu früh erscheinen.

## Eine Terminserie steht nach der Zeitumstellung eine Stunde daneben

Beim Speichern einer Terminserie im Editor von Aperio ging bisher ihre Zeitzone
verloren. Das betraf jede Serie mit Zeitzone, die seit Juni 2026 in Aperio als
Ganzes bearbeitet wurde, auch Serien, die anderswo angelegt wurden, etwa auf dem
iPhone, in Outlook oder im Google-Kalender, und im lokalen Kalender genauso wie
bei iCloud, Google, Microsoft 365 oder Exchange. Ein Termin, der im Editor zur
Serie wurde, bekam gar keine Zeitzone.

Ohne Zeitzone behält eine Serie ihre Uhrzeit in UTC. Nach der Zeitumstellung
steht sie eine Stunde zu früh oder zu spät, bei uns und in jedem anderen
Programm, das den Kalender liest. In den Kalenderansichten sieht man es schon,
wenn man über die nächste Umstellung hinaus blättert.

Seit dieser Version behält der Editor die Zeitzone, und ein Termin, der zur
Serie wird, bekommt die Zeitzone deines Geräts. Meldet dein Gerät UTC oder eine
Zeitzone, die Aperio nicht kennt, bekommt die Serie keine Zeitzone.

Eine Serie, deren Zeitzone UTC unter einem anderen Namen ist, etwa „Etc/UTC“
oder „GMT“, gilt als Serie ohne Zeitzone; ihre Zeiten ändern sich dadurch nicht.
Dasselbe gilt für eine Zeitzone, die Aperio nicht kennt, etwa den Namen einer
Windows-Zeitzone oder einen reinen Versatz wie „+05:30“. Eine Serie mit so einem
Versatz wiederholt sich jetzt in UTC, und nahe Mitternacht können ihre Termine
auf andere Tage fallen als bisher. Alle diese Serien wiederholen sich in UTC, in
den Ansichten und in den Erinnerungen gleich, und stehen nach der Zeitumstellung
eine Stunde daneben, wie oben beschrieben.

Eine Serie, die ihre Zeitzone schon verloren hat, ändert Aperio nicht von
selbst: Für Aperio sieht sie genauso aus wie eine Serie, die absichtlich in UTC
geführt wird. Um eine betroffene Serie zu reparieren, lege sie neu an oder
stelle ihre Zeitzone in dem Programm wieder ein, aus dem sie stammt.

## Einen Fehler melden

1. Stelle in Einstellungen → Protokolle die Stufe auf **Debug**.
2. Stelle das Problem nach.
3. **Exportiere** das Protokoll (oder kopiere es) und lege es deinem Bericht
   bei — zusammen mit dem, was du getan und was du erwartet hast.
