---
title: Privacy Policy
description: How Aperio handles your data.
lastUpdated: false
---

Aperio is an accessibility-first calendar, task and contact application for
desktop and mobile (Android and iOS). This policy explains what data the app and
this website process. In short: **Aperio runs no server of its own and contains
no analytics, advertising, tracking or crash reporting; your data stays on your
device or in the accounts and storage you choose.** Only when you send feedback
yourself does that message go to the developer (see
[Sending feedback and logs](#sending-feedback-and-logs)).

The German version under [`/de/privacy/`](/de/privacy/) is the legally
authoritative one for an operator based in Germany.

## Who is responsible

The controller for data processing in connection with this website and the
Aperio application is:

> Toni Barth · Robert-Koch-Str. 44 · 06110 Halle (Saale) · Germany
> Email: contact@toni-barth.online

## What Aperio processes

Aperio is **local-first**. Your calendars, tasks and contacts are stored **on
your device**. Where you connect an external account (Google, iCloud/CalDAV,
Microsoft/Exchange, Vikunja, Todoist, an iCal feed, or your device's own
calendar), Aperio communicates **directly** with that provider on your behalf to
read and write only the items you ask it to. Aperio operates **no server** that
receives, stores or relays your content.

Optional cross-device synchronisation runs over **storage you choose** (e.g.
WebDAV, SFTP, Dropbox, Google Drive or a local folder) and can be
**end-to-end encrypted**, so the sync storage never sees your unencrypted data.

Account credentials and OAuth tokens for the providers you connect are stored
**only on your device** and are used solely to talk to those providers.

## Mobile app permissions

The Android and iOS apps request only the permissions a feature needs, and only
when you use that feature:

- **Calendars / Reminders** (`READ_CALENDAR` / `WRITE_CALENDAR` on Android,
  Calendar and Reminders access on iOS): only if you enable the built-in
  *device calendar* — to read and write the calendars and reminders already on
  your device. If you don't use it, no calendar permission is requested. On a
  phone where the system has never asked yet (after moving to a new one),
  Aperio asks at start, and only while such an account exists. Android cannot
  say whether it ever asked, so Aperio keeps a note on the phone that it did;
  the note is left out of backups, so a new phone is asked again.
- **Photos**: only the single image you explicitly pick (e.g. a contact photo);
  the app has no bulk access to your photo library and the image is processed
  locally.
- **Notifications**: to show your reminders, which are scheduled **locally** on
  your device. There is **no push server** — no remote message ever reaches the
  app.

The app contains **no analytics, no advertising SDKs, no tracking and no crash
reporting**, and it collects neither an advertising identifier nor your location.

## Google user data

Aperio's use and transfer of information received from Google APIs to any other
app will adhere to the
[Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy),
including the Limited Use requirements. Google data (calendar, tasks, contacts)
is used only to provide the features you invoke in the app, is processed locally
on your device, and is not sold, transferred or shared with third parties.

## Sending feedback and logs

Under **Settings → Feedback** (and through **Send logs** in the Logs section)
you can send the developer a message. This happens only when you press
**Send**; nothing leaves the device before, and everything that goes along is
shown in the form.

- **What is sent:** your message; your email address, if you give one, so that
  a reply can reach you; a line with the Aperio version, the operating system
  and its version (on the phone also the build number, on Android the device
  model); if you choose, the app's logs as a ZIP file, at most the newest 2 MB,
  with email addresses and access tokens replaced by placeholders first. As
  with any connection, the service technically receives your IP address.
- **Through whom:** the message goes to a form at
  [Static Forms](https://www.staticforms.dev/privacy-policy), which emails it
  to the controller and keeps it in its inbox. Static Forms is a processor; a
  data processing agreement is in place. The form data is stored in the EU
  (AWS, Frankfurt). Static Forms sends the notification email through AWS in
  the United States.
- **Spam check:** Static Forms rates each submission with an AI spam filter.
  Only the fields needed for the rating go to an AI provider; attachments,
  credentials and tokens are removed and email addresses masked first. The
  provider stores nothing and does not train on the data; the rating may take
  place outside the EU. The sub-processors involved are named in
  [Static Forms' list](https://www.staticforms.dev/dpa/sub-processors). Against
  bots the form is also protected by a computation Aperio solves on your device
  (ALTCHA); no data about you is transferred for it.
- **Purpose and legal basis:** to read and answer your feedback and to fix
  problems in Aperio (Art. 6(1)(f) GDPR); sending is voluntary.
- **Retention:** submissions are deleted once the feedback has been dealt with,
  at the latest after 12 months. If you would like a message deleted sooner,
  write to contact@toni-barth.online.

## This website

This documentation site is served as static files via **GitHub Pages**. The host
may process technical access data (e.g. IP address, user agent) in server logs
as far as technically necessary to deliver the site; see
[GitHub's Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement).
This site sets no tracking cookies and embeds no third-party analytics. The
on-page search runs entirely in your browser.

## Your rights

Under the GDPR you have the right to access, rectification, erasure, restriction
of processing, data portability and objection. To exercise them, contact
contact@toni-barth.online. You also have the right to lodge a complaint with a
data-protection supervisory authority.

_Last updated: 16 November 2026._
