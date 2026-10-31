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
geht fehlender Zugriff einem Anmeldeproblem vor, ein Anmeldeproblem einem
ausgelasteten Anbieter und ein ausgelasteter Anbieter allem anderen. Ein
Konto, das schon ausfällt, wird nur dann noch einmal angesagt, wenn seine
Ursache schwerer wird.

Ist der Anbieter ausgelastet — er weist Anfragen ab, weil in kurzer Zeit
viele kamen —, sagt die Warnung das, und dass du nichts tun musst: Am Desktop
versucht Aperio es von selbst erneut, am Handy beim nächsten Aktualisieren,
zum Beispiel wenn du die App wieder öffnest. Seine Zeilen sagen „Der Anbieter
ist ausgelastet.“ Solange er ausgelastet ist, fragen die Ansichten ihn nicht
nach dem, was sie schon zeigen; eine Woche oder ein Monat, die noch nie
geladen wurden, oder eine Liste, die noch nie geladen wurde, werden trotzdem
einmal angefragt.

Ein kurzer, einmaliger Verbindungsaussetzer löst die Warnung nicht aus: Ein
Netzwerkfehler oder ein ausgelasteter Anbieter wird erst gezeigt, wenn er bei
einer zweiten automatischen Aktualisierung mindestens eine Minute nach der
ersten wieder auftritt — ein Kaltstart mit noch nicht bereitem Netz erzeugt
so keinen Fehlalarm, und die zwei oder drei Aktualisierungen beim Öffnen des
Handys zählen einmal. Trifft eine Ansicht, die von sich aus anfragt, auf einen
ausgelasteten Anbieter, zählt das nicht; jeder andere Fehler dabei zählt jedes
Mal. Ein Anmeldeproblem, das sich nie von selbst behebt, erscheint sofort, und
eine Aktualisierung, die du startest, oder die Änderung eines Kontos meldet
ihr Ergebnis für die betroffenen Konten unmittelbar, auch in den ersten
Sekunden nach dem Start. Die Warnung verschwindet von selbst, sobald eine
Aktualisierung wieder gelingt.

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

## Ein Exchange-Termin ist verschoben, nachdem er zur Serie wurde

Hast du in Aperio einen Termin auf einem Exchange-Konto zur Serie gemacht oder
einer ganztägigen Exchange-Serie Uhrzeiten gegeben, stand der Termin danach
meist um ein oder zwei Stunden verschoben, nahe Mitternacht auch an einem
anderen Tag. Ein in
Aperio angelegter Termin am Montag um 00:30 stand danach zum Beispiel sonntags
um 23:30.

Exchange behält beim Wechsel der Zeitzone die gespeicherte Uhrzeit und deutet
sie in der neuen Zone. Seit dieser Version schickt Aperio die Zeitzone zuerst
und danach Beginn und Ende, so bleibt der Termin, wo er war. Bekommt eine
ganztägige Serie Uhrzeiten, schickt Aperio Beginn und Ende vor und nach der
Zeitzone und danach die Wiederholung: Die Zeitzone zuerst lehnt Exchange bei
einer täglichen Serie ab, und ohne die Wiederholung begänne die Serie einen
Tag später.

Was du tun kannst: Eine Serie, die schon verschoben ist, verschiebst du einmal
von Hand an die richtige Stelle.

## Aperio fragt, ob Vorkommen einer Serie verloren gehen dürfen

Beim Speichern einer Exchange-Serie fragt Aperio manchmal „Vorkommen gehen
verloren“, mit einem Satz wie: „„Teamrunde“: Diese Änderung schreibt Beginn und
Ende der Serie neu. Exchange verwirft dabei ein Vorkommen, das einzeln geändert
wurde: Es nimmt wieder die Angaben der Serie an. Trotzdem speichern?“

Exchange verwirft jedes einzeln geänderte und jedes gelöschte Vorkommen einer
Serie, sobald Aperio ihren Beginn und ihr Ende, ihre Zeitzone oder das Muster
ihrer Wiederholung neu schreibt: wenn du die Serie auf eine andere Uhrzeit oder
einen anderen Tag legst, ihre Dauer änderst, sie ganztägig machst oder andere
Wochentage, einen anderen Abstand oder eine andere Häufigkeit wählst (täglich
statt wöchentlich). Titel, Ort und Erinnerung ändern daran nichts, und in aller
Regel auch die Anzahl der Termine und ihr Ende nicht; dann fragt Aperio nicht.
Nur bei einer Serie, deren Zeitzone Aperio mit ihrer Wiederholung neu schreibt
(siehe unten), bringen auch sie die Frage.

Gelöschte Vorkommen löscht Aperio nach dem Speichern selbst wieder, dort, wo sie
jetzt stehen. Nur wenn du das Muster und die Uhrzeit oder den Tag zugleich
änderst oder Aperio die Zeitzone der Serie nicht lesen kann, lässt sich nicht
sagen, wo sie stehen; dann zählt die Frage sie mit. Einzeln geänderte Vorkommen
gehen verloren: Sie nehmen wieder die Angaben der Serie an.

Was du tun kannst: „Abbrechen“ lässt die Serie, wie sie ist, und deine Änderung
bleibt im Editor stehen. „Trotzdem speichern“ speichert sie. Ließ sich ein
gelöschtes Vorkommen danach nicht wieder löschen, nennt Aperio seinen Tag; lösche
es dann einzeln.

Die Zeitzone einer Serie kannst du in dieser Version nicht selbst wählen. Nach
einer anderen Zeitzone fragt Aperio deshalb nur bei einer Serie aus einem
anderen Programm, deren Ende in einer anderen Zeitzone steht als ihr Beginn,
oder wenn die Zeitzone der Serie anderswo geändert wurde, seit Aperio sie
zuletzt gelesen hat; im zweiten Fall öffne die Serie neu, sobald Aperio den
Kalender aktualisiert hat. Würde bei einer solchen Serie etwas verloren gehen —
einzeln geänderte Vorkommen oder gelöschte, die Aperio nicht zuordnen kann —,
kommt die Frage, sobald du ihre Wiederholung änderst, auch nur die Anzahl der
Termine oder ihr Ende; sie ab einem späteren Termin zu ändern oder zu löschen
lehnt Aperio dann mit dem Satz ab (unten). Sonst speichert Aperio und löscht die
gelöschten wieder.

Ziehst du eine Serie mit der Maus und wählst „Ganze Serie“, stellt Aperio
danach dieselbe Frage in einem eigenen Dialog; „Trotzdem speichern“ verschiebt
die Serie, „Abbrechen“ lässt sie, wo sie war. Überträgst du eine Änderung an
Kopien in anderen Kalendern, überträgt Aperio sie zuerst an alle übrigen Kopien
und fragt dann einmal nach den Kopien, deren Serie Vorkommen verlöre, jede mit
ihrem Kalender: „Trotzdem übertragen“ überträgt die Änderung auch dorthin;
„Abbrechen“ lässt diese Kopien unverändert, und Aperio nennt sie unter „Nicht
geändert“.

Eine Serie ab einem ihrer Termine zu ändern oder zu löschen kürzt nur die alte
Serie, und das behält ihre einzeln geänderten und gelöschten Vorkommen. Wo
Aperio dabei die Zeitzone neu schriebe (siehe oben) und etwas verloren ginge,
kürzt es die Serie nicht: Aperio fragt dort nicht, sondern sagt den Satz und
ändert nichts. Das geht dann in Outlook. Auch eine Serie mit Uhrzeit, die
Exchange in einer Zeitzone speichert, die Aperio nicht lesen kann, kürzt Aperio
nicht (siehe den nächsten Abschnitt). Ab ihrem ersten Termin geändert, wird die
ganze Serie neu geschrieben, und Aperio fragt wie im Editor.

Kann Aperio den aktuellen Stand einer Serie nicht vom Server lesen, ändert es
sie nicht, denn dann weiß es nicht, was verloren ginge. Aperio sagt dann:
„Aperio konnte den aktuellen Stand dieses Termins nicht vom Server lesen und hat
ihn deshalb nicht geändert. Versuche es gleich noch einmal.“

## Exchange speichert einen Termin in einer Zeitzone, die Aperio nicht lesen kann

Aperio sagt dann: „Exchange speichert diesen Termin in einer Zeitzone, die
Aperio nicht lesen kann. Seinen Tag kannst du deshalb hier nicht ändern; den
Titel und anderes schon. Es wurde nichts geändert.“

Exchange legt einen ganztägigen Termin auf die Mitternächte der Zeitzone, in
der es ihn speichert. Damit der Termin einen Tag lang bleibt, schreibt Aperio
einen neuen Tag deshalb als Mitternacht in genau dieser Zeitzone. Meist kennt
Aperio sie: eine Windows-Zeitzone wie „W. Europe Standard Time“ oder einen
Namen wie „Europe/Berlin“. Eine eigene Zeitzone, etwa „Customized Time Zone“
bei Terminen aus anderen Programmen, kann Aperio lesen, wenn Exchange ihre
Regeln mitschickt. Schickt Exchange nur einen Namen, den Aperio nicht kennt,
ohne ihre Regeln, rät Aperio nicht: Sonst stünde der Termin danach in Outlook
über zwei Tage.

Was du tun kannst: Ändere den Tag in Outlook. Alles andere, etwa Titel, Ort
oder Erinnerung, kannst du in Aperio weiter ändern, solange du den Tag dabei
nicht verschiebst.

Derselbe Satz kommt, wenn du eine Serie mit Uhrzeit ab einem ihrer Termine
änderst oder löschst und Exchange sie in so einer Zeitzone speichert. Den Tag,
an dem die gekürzte Serie endet, zählt Exchange in dieser Zeitzone; Aperio
wüsste also nicht, an welchem Tag es sie enden lassen soll, und ändert nichts.
Ändere oder lösche diese Serie ab dem Termin in Outlook. Titel, Ort und
Erinnerung der Serie kannst du in Aperio weiter ändern.

Ähnlich klingt: „Aperio konnte den aktuellen Stand dieses Termins nicht vom
Server lesen und hat ihn deshalb nicht geändert.“ Um die Zeitzone zu kennen,
liest Aperio den Termin vor dem Speichern vom Server. Gelingt das nicht, etwa
weil der Server gerade nicht antwortet, schickt Aperio nichts. Versuche es
nach einem Moment noch einmal.

## Nach dem Verschieben, Kopieren oder Duplizieren einer Serie ist ein Termin wieder da

Verschiebst du eine ganze Serie in einen anderen Kalender, kopierst sie,
duplizierst sie mit `Strg+D` oder wählst im Editor einen anderen Kalender für
sie, schreibt Aperio sie dort als neue Serie. Sie nimmt die gelöschten Termine
der alten Serie mit, sonst nichts:

- Ein einzeln geänderter Termin, etwa in Outlook, ist zu seiner üblichen Zeit
  wieder da, mit Titel, Ort und Uhrzeit der Serie. Nach dem Verschieben ist
  seine eigene Änderung mit der alten Serie weg; nach dem Kopieren oder
  Duplizieren bleibt sie in der alten.
- Nach Exchange oder Google verschoben oder kopiert, oder dort dupliziert,
  behält die neue Serie die Löschungen ab heute: Exchange nimmt beim Anlegen
  einer Serie keine an, und Google löscht einen Termin, indem es ihn absagt,
  deshalb entfernt Aperio sie danach einzeln. Die vor heute erscheinen
  wieder.
- Nach Microsoft 365 verschoben oder kopiert, zeigt die neue Serie gelöschte
  Termine vorerst wieder: Microsoft 365 nimmt beim Anlegen einer Serie keine
  Löschungen an.
- Aus einem Google-Kalender kennt Aperio gelöschte Termine von etwa drei
  Monaten vor heute bis ein Jahr danach. Ein weiter entfernt gelöschter ist in
  der neuen Serie wieder da.

Was du tun kannst: Lösche oder ändere den wieder aufgetauchten Termin im neuen
Kalender.

## Aperio hat eine neue Serie wieder entfernt

Bei Exchange und Google legt Aperio eine neue Serie zuerst an und entfernt
ihre gelöschten Termine dort danach einzeln, während der Dialog **Vorgang
läuft, bitte warten …** zu sehen ist: Exchange nimmt eine neue Serie ohne sie
an, und Google löscht einen Termin, indem es ihn absagt. Gelingt das nicht
ganz — der Kalender findet einen Termin nicht, antwortet zu langsam, oder die
Serie hat danach nicht genau die Löschungen, die Aperio gemacht hat —,
entfernt Aperio die neue Serie wieder und sagt das: Es wurde nichts geändert,
die alte Serie ist, wie sie war. Bei Exchange liegt die entfernte Serie in den
gelöschten Elementen. Hatte die Serie Leute eingeladen, haben sie auch die
Absage bekommen, und die Meldung sagt das; bei Google können sie, wenn du sie
benachrichtigt hast, auch für jeden Termin eine Absage bekommen haben, den
Aperio vorher abgesagt hatte. Sagt sie, es habe zu lange gedauert, hat der
Kalender langsam geantwortet, oder die Serie hat ab heute sehr viele
gelöschte Termine. Sagt sie, der Anbieter sei ausgelastet gewesen, hat der
Kalender Aperio immer wieder warten lassen: Das tut er, wenn in kurzer Zeit
viele Anfragen kommen.

Was du tun kannst: Versuche es später noch einmal. Scheitert es weiter,
schick das Protokoll mit der Meldung (siehe unten): Es nennt den Schritt, der
gescheitert ist, und warum, ohne Kennungen von Terminen oder Kalendern.

## Eine neue Serie steht möglicherweise zusätzlich im Kalender

Konnte Aperio eine neue Serie bei Exchange oder Google weder fertigstellen
noch wieder entfernen, sagt es, dass sie jetzt möglicherweise zusätzlich im Kalender
steht — neben der alten, ab dem Tag, an dem du geteilt hast, oder im anderen
Kalender nach dem Verschieben oder Kopieren. Diese Meldung lässt sich nur
schließen: Erneutes Speichern legte noch eine Serie an. Können schon
Einladungen rausgegangen sein, sagt die Meldung auch das.

Was du tun kannst: Öffne den Kalender, in Aperio oder in der App des
Kalenders (Outlook, Google Kalender), und lösche die Serie, die doppelt da
ist.

## Der Anbieter ist gerade ausgelastet

Kommen in kurzer Zeit viele Anfragen, weisen manche Anbieter weitere eine
Weile ab. Aperio sagt dann „Der Anbieter ist gerade ausgelastet“: beim
Speichern mit „Es wurde nichts geändert“, beim Laden, bei der Verfügbarkeit
und bei Meetings ohne. In einem Editor bleibt deine Eingabe stehen; die
Ansichten zeigen, was zuletzt geladen wurde. Konnten am Desktop die
Abschnitte einer Liste noch nicht geladen werden, sagt das die
Abschnittsauswahl im Aufgaben-Editor und unter Verschieben/Kopieren; ein
Abschnitt, den du dort anlegst, steht trotzdem darin. Wie lange der
Anbieter warten lässt, sagt Aperio nicht, es steht im Protokoll.

Was du tun kannst: Warte einen Moment und versuche es noch einmal.

## Eine Änderung wurde nur zum Teil gespeichert

Manche Anbieter speichern eine Aufgabe, einen Termin oder einen Kontakt in
mehreren Schritten: erst das Element, dann zum Beispiel die Zuständigen, das
Foto oder die Mitglieder einer Gruppe. Ist der Anbieter nach dem ersten
Schritt ausgelastet, steht das Element, aber nicht alles daran. Aperio sagt
dann, dass die Änderung nur zum Teil gespeichert ist, und die Meldung lässt
sich nur schließen: Erneutes Speichern legte das Element ein zweites Mal an.
Die Ansicht lädt neu und zeigt, was beim Anbieter steht.

Nimmt der Anbieter das Foto eines neuen Kontakts nicht an, steht der Kontakt
ohne Foto, und Aperio sagt das.

Legst du eine Aufgabe mit Unteraufgaben an und scheitert eine Unteraufgabe,
bearbeitet der Editor danach die angelegte Aufgabe: Die noch nicht angelegten
Unteraufgaben stehen unter einer eigenen Überschrift, und Speichern übernimmt
deine Änderungen an der Aufgabe und legt die noch fehlenden Unteraufgaben an.
Eine Unteraufgabe, deren Antwort verloren ging, wurde vielleicht angelegt:
Aperio sagt das und bietet sie nicht noch einmal an; prüfe sie.

Wurde eine Aufgabe nur zum Teil gespeichert, kam ihr neuer Status aber an
(ein späterer Schritt wie ihre Zuständigen wurde abgewiesen), übernehmen die
Aufgaben darüber und darunter ihn trotzdem, wie beim Abhaken; nimmt eine ihn
nicht an, sagt die Meldung das danach. Kam der Status selbst nicht an,
behalten sie ihren.

Was du tun kannst: Öffne das Element später noch einmal und ergänze, was
fehlt.

## Nach dem Verschieben steht das Original noch da

Verschiebst du eine Aufgabe oder einen Termin in eine andere Liste oder einen
anderen Kalender — außer zwischen zwei lokalen von Aperio, auch innerhalb
eines Kontos —, legt Aperio dort eine Kopie an und löscht dann das Original. Ist der Anbieter beim Löschen ausgelastet, steht das
Verschobene am Ziel, und das Original bleibt, wo es war. Aperio nennt die
Liste oder den Kalender, in dem es noch steht.

Was du tun kannst: Lösche das Original dort selbst. Verschiebe es nicht noch
einmal, denn das legte am Ziel eine zweite Kopie an.

## Einen Fehler melden

1. Stelle in Einstellungen → Protokolle die Stufe auf **Debug**.
2. Stelle das Problem nach.
3. **Exportiere** das Protokoll (oder kopiere es) und lege es deinem Bericht
   bei — zusammen mit dem, was du getan und was du erwartet hast.
