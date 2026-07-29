---
title: "Video meetings with Cisco Webex"
---

Aperio can do two things with Webex, and they are independent of each other.

**Joining** works straight away, for any meeting, from any tool. An invitation
written by Outlook, by eM Client or by Webex itself carries a join link, and
Aperio finds it — in the event's location or anywhere in its text, in any
language. You do not need a Webex account in Aperio for this, and you do not
need to set anything up. Every event with a meeting gets a **Join** entry: in
the event editor, in the context menu (`Shift+F10` or the Menu key on any
event), and in the rotor on the phone.

**Creating** is what needs an account. Once you have connected one, an event
gains a **Create a meeting** button: Aperio mints a Webex meeting for that
event, writes its link into the event where every other calendar app can read
it, and remembers the meeting so it can also take it back down.

## Connecting a Webex account

Go to **Settings → Accounts → Add account** and pick **Cisco Webex**.

What you see next depends on the build you are running:

- If it asks only for a name, this build carries Aperio's own Webex
  registration. Enter a name and press **Add** — your browser opens the Webex
  sign-in page, you grant access, and the tab closes itself.
- If it also asks for a **client ID** and a **client secret**, this build
  carries none, and you register your own integration once. That takes about
  five minutes and is free; the next section walks through it.

Nothing else to decide here. Whether a meeting is a fresh one or your permanent
room, and whether Webex should mail the attendees, are both answered per meeting
— and Aperio answers the second one for you. See *Who tells the attendees*
below.

## Registering your own integration

Only needed when the connect form asks for a client ID and secret.

1. Open [developer.webex.com/my-apps](https://developer.webex.com/my-apps) and
   sign in with your Webex account.
2. **Create a New App → Integration.**
3. Give it a name and description — these are for you; nobody else sees them.
   An icon is required; any square PNG will do.
4. **Redirect URI:** enter exactly

   ```
   http://127.0.0.1:8080/oauth/webex
   ```

   This has to match to the character. It is a loopback address — the page never
   leaves your machine; Aperio listens on it for the moment the sign-in comes
   back.
5. **Scopes:** tick `meeting:schedules_read`, `meeting:schedules_write` and
   `meeting:preferences_read`. Webex adds `spark:kms` by itself; that is normal
   and nothing to worry about.
6. Save. Webex shows a **client ID** and a **client secret**. Copy both into
   Aperio's connect form.

The secret goes into your system keychain, never into Aperio's account database
— which matters because that database is what synchronises to your other
devices.

> **A note about "mobile SDK".** If Webex asks whether the integration uses a
> mobile SDK, answer **no**. Aperio talks to the Meetings REST API, not to
> Webex's own app SDK.

## Creating a meeting for an event

Open an event, save it if it is new, and choose one of two:

- **Create a meeting** mints a fresh Webex meeting with its own link and
  password, just for this event.
- **Link the Personal Meeting Room** points the event at your permanent room
  instead. That needs no scheduling licence and has no daily cap, but it is
  always the same room behind the same link, so back-to-back events can walk
  into each other there.

The choice is per event, not per account: which of the two a meeting should be
is a property of that meeting, and it is asked at the moment you know the
answer. Either way, Aperio:

- creates the meeting on Webex with the event's title and time,
- hands Webex the event's attendees, so the meeting knows who it is for,
- writes the join link into the event's location (if it was empty) and appends a
  short block with the link and password to the description,
- and records that this meeting belongs to this event.

Anyone you invite sees the link in a perfectly ordinary event, whatever calendar
app they use.

**Remove the meeting** appears once an event has one. It deletes the meeting at
Webex and takes the link back out of the event.

You will see the remove button only for meetings **Aperio created**. An event
carrying a colleague's Webex link gets a Join button and nothing else — that
meeting is not yours to delete.

## Meetings that have no calendar entry

A meeting you create straight in Webex's own web interface exists only there. It
has no calendar entry, so no calendar app has ever shown it — the first reminder
you get is the meeting starting.

Once a Webex account is connected, Aperio adds a **read-only calendar named
after that account**, holding exactly those meetings. It behaves like any other
calendar: toggle it in the sidebar, see the meetings in the day and week views,
join them from the context menu.

It shows only meetings with **no** calendar entry. A meeting that already has
one — because Aperio created it, or because an invitation brought it in — is not
listed twice. The two are matched by their join link, which is exact.

The calendar cannot be edited. It shows what exists at Webex; to create a
meeting, add an event to one of your own calendars and use **Create a meeting**
on it. Your permanent Personal Meeting Room does not appear either: it is always
on, which is precisely not an appointment.

## Things worth knowing

**A meeting the invitation brought in.** When Webex mails you an invitation and
your calendar turns it into an event, that event has a meeting but Aperio did
not create it. The editor offers **Take over the meeting**: it looks the meeting
up by its join link, and from then on it can be removed like one Aperio made.
Nothing is written to the event — the link is already there.

**Who tells the attendees.** Webex can email everyone an invitation itself, and
its mails carry a calendar attachment. That is a duplicate when your own
calendar already invites people server-side — Exchange, Google and a CalDAV
server with scheduling all do — because each attendee then gets two invitations
and two entries. But on a calendar that cannot invite anyone at all (a local
calendar, a subscribed feed, plain CalDAV), Webex's mail is the only invitation
there will ever be, and suppressing it means nobody is told.

So it is not a setting. When Aperio creates a meeting it looks at the event's
own calendar and asks Webex to mail the attendees exactly when that calendar
cannot. An event with no attendees mails nobody either way.

**Who is really invited.** The attendees on such an event are whatever the
invitation mail addressed, which is often just you and Webex's own sending
address (`messenger@webex.com`). Aperio additionally shows Webex's own invitee
list under "Invited at the provider", so you can see who is actually coming. If
Webex declines to answer — reading the invitee list of a meeting you only
attend is not always permitted — the section is simply absent.

**One meeting per event, including recurring ones.** A recurring series shares
one meeting, exactly as a recurring meeting does in Webex itself.

**Removing works from the device that created it.** The record of which meeting
belongs to which event stays on the machine that made it — it is not
synchronised, because it is bookkeeping about a Webex object rather than part of
your event. On another device you can still delete the event; the meeting then
stays on Webex, where you can remove it in Webex's own interface.

**Moving an event does not move the meeting.** Webex's API has no update in the
set Aperio uses. If a time changes materially, remove the meeting and create it
again.

**Removing a linked personal room.** *Remove the meeting* takes the link back
out of the event. The room itself stays — it belongs to your account, not to any
one event, and Webex has no way to delete it.

**A licence is needed for scheduling.** Creating a meeting per event requires a
Webex account that may schedule meetings. If yours may not, switch **Use the
Personal Meeting Room** on — that works without one.

## If something goes wrong

**"No plugin serves this adapter kind."** The Webex plugin is not loaded or was
switched off in **Settings → Plugins**.

**Sign-in never comes back.** Check the redirect URI on your integration
character by character, including the port and `/oauth/webex`. If port 8080 is
in use by something else on your machine, Aperio says so before opening the
browser.

**"Sign in to Webex again" out of nowhere.** Your build's Webex registration
changed — this happens when you move between an official build and one of your
own. Connect the account again.

Aperio's log (**Settings → Troubleshooting**) records the failing request path
and status without ever recording your tokens.
