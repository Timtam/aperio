---
title: "Connecting Google – your own OAuth credentials (interim guide)"
---

> **Note:** This guide is an **interim solution**. Aperio does not ship a
> verified Google app registration yet — so for now you create your own
> (free) project in the Google Cloud Console once. As soon as Aperio ships
> official credentials, this whole section becomes obsolete and "Add
> account → Google → Sign in" will be all you need.

What to expect: about **15 minutes** of clicking, completely **free**, no
credit card required. All you need is your Google account. At the end you
will have two strings — a **client ID** and a **client secret** — which you
enter in Aperio.

> **Why is this needed?** The client ID identifies the *application* to
> Google (not you). Since Aperio is not registered with Google as a
> verified app yet, you effectively register your own private "Aperio app".
> Aperio never sees your Google password — signing in always happens on the
> regular Google sign-in page in your browser.

## Step 1: Create a project

1. Open [console.cloud.google.com](https://console.cloud.google.com) and
   sign in with your Google account. (On your very first visit you must
   accept the terms of service.)
2. Click the **project picker** in the top bar (initially shows "Select a
   project"), then **"New project"**.
3. Project name e.g. `Aperio`, leave the organization empty → **"Create"**.
4. Wait a moment, then select the new project in the project picker (the
   top bar should now show "Aperio").

## Step 2: Enable the APIs

Aperio talks to three Google services. Enable them for your project:

1. Menu (☰) → **"APIs & Services" → "Library"**.
2. Search for each of the following entries, open it, and click
   **"Enable"**:
   - **Google Calendar API** (events)
   - **Google Tasks API** (tasks)
   - **People API** (contacts and attendee suggestions)

> **Optional – Google Drive as sync storage:** If you also want to run
> [device synchronization](/guides/tutorial/09-synchronization/) through Google
> Drive, additionally enable the **Google Drive API**. You can then reuse
> the same project and the same credentials from this guide in the sync
> dialog.

## Step 3: Configure the OAuth consent screen

This is the page Google shows you in the browser when connecting.

1. Menu (☰) → **"APIs & Services" → "OAuth consent screen"**.
   (Google occasionally restructures the console — the area is also known
   as **"Google Auth Platform"** with the subpages *Branding*, *Audience*,
   *Clients*.)
2. The first visit starts a setup wizard:
   - **App name:** `Aperio`
   - **User support email:** your email address
   - **Audience / user type:** **External**
   - **Developer contact information:** your email address
3. Everything else (logo, domains, scopes) can be **left empty** — Aperio
   requests the permissions it needs during sign-in. Finish the wizard.

## Step 4: Add yourself as a test user

Freshly created apps are in **"Testing"** status — only listed test users
may sign in.

1. In the OAuth consent screen / **"Audience"** area, find the **"Test
   users"** section.
2. **"+ Add users"** → enter the Gmail address(es) of **every** Google
   account you want to connect in Aperio → save.

> **Important — the 7-day catch:** In "Testing" status Google expires
> sign-ins after **7 days** — you would have to reconnect the account in
> Aperio every week. The fix: on the same page, click **"Publish app"**
> (status "In production"). Your sign-in then stays valid permanently. In
> exchange, Google shows a one-time warning page when connecting ("Google
> hasn't verified this app") — that is harmless, because it is *your own*
> app registration: click **"Advanced"** and then **"Go to Aperio
> (unsafe)"**. **Recommendation: publish.**

## Step 5: Create the OAuth client (type "Desktop app")

Now the two strings for Aperio are created:

1. Menu (☰) → **"APIs & Services" → "Credentials"**.
2. **"+ Create credentials" → "OAuth client ID"**.
3. **Application type: "Desktop app"** (important — not "Web
   application"). Any name, e.g. `Aperio Desktop` → **"Create"**.
4. A dialog now shows:
   - the **client ID** — a long string ending in
     `.apps.googleusercontent.com`
   - the **client secret** — usually starting with `GOCSPX-`
5. Copy both (e.g. park them in a text editor). You can always view them
   again later: **Credentials** → click your client.

## Step 6: Enter them in Aperio

1. In Aperio: **Settings → Accounts → Add account** → choose the
   **Google** provider.
2. Paste the client ID into the **"Google OAuth client ID"** field and the
   secret into **"Google OAuth client secret"**.
3. Click **"Add"**. Aperio opens your browser with the Google sign-in:
   - Pick your Google account.
   - If the "Google hasn't verified this app" warning page appears (see
     step 4): **"Advanced" → "Go to Aperio (unsafe)"**.
   - **Approve** the requested permissions (calendar, tasks, contacts).
4. The browser tab closes automatically; your Google calendars, task lists
   and contacts appear in the sidebar.

## Troubleshooting

| Symptom | Cause & fix |
|---|---|
| **"Error 403: access_denied"** during sign-in | Your account is not listed as a test user (step 4) — add it **or** publish the app. |
| Warning page **"Google hasn't verified this app"** | Normal for a self-registered app. **"Advanced" → "Go to Aperio (unsafe)"**. |
| **Signed out** after about a week | The app is still in "Testing" status (7-day limit). **"Publish app"** in the console, then reconnect the account via **Settings → Accounts**. |
| **"accessNotConfigured"** / "API has not been used" | One of the APIs from step 2 is not enabled — enable it (the error message names the missing API). |
| **"invalid_client"** | Client ID or secret copied incorrectly or with whitespace — re-copy both from the console. |

> **Security:** The client ID and secret only identify your app
> registration — on their own they grant no access to your account. The
> actual access tokens are created by your browser sign-in and stored
> locally by Aperio; with
> [end-to-end encryption](/guides/tutorial/09-synchronization/) enabled they
> additionally travel encrypted between your devices.
