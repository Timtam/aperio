import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/announcerContext';
import { collectValues, firstMissingField } from '@aperio/shared';

import { FocusableNote } from '../a11y/FocusableNote';
import {
  accountFormSpec,
  connectAccount,
  listAdapterKinds,
  connectGoogleAccount,
  connectMicrosoftAccount,
  createAccount,
  syncContactsNow,
  deleteAccount,
  discoverEwsEndpoint,
  getUserPref,
  isCommandError,
  listAccounts,
  listAccountsMissingCredentials,
  renameAccount,
  resetAccountSync,
  setUserPref,
  testCaldavConnection,
  testEwsConnection,
  testIcalFeed,
  testTodoistConnection,
  testVikunjaConnection,
} from '../api/client';
import type { AccountFormSpec, AdapterKindInfo } from '../api/client';
import type { Account, AdapterKind } from '../api/types';
import { useCalendarStore } from '../state/calendarStoreContext';
import { useDialogState } from '../state/dialogStateContext';
import {
  clampErrorText,
  useRefreshErrors,
} from '../state/useRefreshErrors';
import { AccountSchemaForm } from './AccountSchemaForm';
import { ConfirmDialog } from './ConfirmDialog';
import { ContactsPrivacyNoticeModal } from './ContactsPrivacyNoticeModal';

/** `user_prefs` key gating the one-shot privacy notice on the first
 *  contacts-capable account connect (DESIGN.md §10.6). Stored as the
 *  string "true" once acknowledged. */
const PREF_PRIVACY_NOTICE_ACK = 'contacts.privacyNoticeAcknowledged';

/** Adapter kinds whose ContactsFeature impl pulls remote address-book
 *  data and therefore trigger the privacy notice on first connect.
 *  Local accounts are excluded — no remote pull, no notice. */
const CONTACTS_CAPABLE_KINDS: ReadonlySet<AdapterKind> = new Set([
  'google',
  'microsoft_graph',
  'ews',
  'caldav',
]);

/**
 * Account management panel (DESIGN.md §6.2 / §6.4). Rendered inside
 * the Settings dialog's `role="tabpanel"`. The standalone Modal
 * wrapper moved up to SettingsDialog — global configuration lives
 * behind one entry point now.
 *
 * Two halves stacked vertically:
 *   - top: existing accounts with a Delete button each
 *   - bottom: "add account" form
 *
 * The kind picker shows every adapter the spec lists, with the
 * not-yet-implemented entries disabled and labelled "coming soon".
 */

/** The kinds the host has no plugin for because it implements them itself.
 *
 *  `local` is the built-in store; it is created during bootstrap and is not
 *  offered in the picker. Everything else in the picker comes from
 *  `listAdapterKinds()`, i.e. from the plugins that are actually installed. */
const HOST_INTERNAL_KINDS: ReadonlySet<AdapterKind> = new Set([
  'local',
  'device_calendar',
]);

interface CaldavFields {
  serverUrl: string;
  username: string;
  password: string;
}

const EMPTY_CALDAV: CaldavFields = {
  serverUrl: '',
  username: '',
  password: '',
};

interface IcalFields {
  feedUrl: string;
  username: string;
  password: string;
}

const EMPTY_ICAL: IcalFields = {
  feedUrl: '',
  username: '',
  password: '',
};

interface GoogleFields {
  clientId: string;
  clientSecret: string;
}

const EMPTY_GOOGLE: GoogleFields = {
  clientId: '',
  clientSecret: '',
};

interface MicrosoftFields {
  clientId: string;
  authority: string;
}

const EMPTY_MICROSOFT: MicrosoftFields = {
  clientId: '',
  authority: 'common',
};

interface EwsFields {
  endpoint: string;
  username: string;
  password: string;
}

const EMPTY_EWS: EwsFields = {
  endpoint: '',
  username: '',
  password: '',
};

interface VikunjaFields {
  serverUrl: string;
  apiToken: string;
}

const EMPTY_VIKUNJA: VikunjaFields = {
  serverUrl: '',
  apiToken: '',
};

interface TodoistFields {
  apiToken: string;
}

const EMPTY_TODOIST: TodoistFields = {
  apiToken: '',
};

export function AccountsPanel() {
  const { t, i18n } = useTranslation();
  const announce = useAnnouncer();
  // Per-account refresh-error surface: failing containers per account
  // (silent-staleness warning + the re-enter-password hint).
  const { errorsByAccount } = useRefreshErrors();
  // The store owns the calendar / task-list catalog the sidebar
  // renders from. We have to nudge it manually after creating or
  // deleting an account — otherwise the new account's calendars
  // wouldn't show up until something else triggers a store refresh.
  const {
    refreshCalendars,
    refreshTaskLists,
    refreshContactLists,
    refreshAccounts,
  } = useCalendarStore();
  // §19.11 step 8 — manual entry point for the "Konten verbinden"
  // wizard. The auto-popup only fires right after accept_remote;
  // when an external account is added on another device and
  // arrives here via a regular sync round it has no path back to
  // the wizard. This panel surfaces the reconnect call site for
  // every account whose secret slot is empty on this device.
  const { openSyncAccountsConnect, dataVersion } = useDialogState();

  const [accounts, setAccounts] = useState<Account[]>([]);
  /** Account ids that don't have a credential in the OS keychain.
   *  Drives the banner above the listbox + a "(needs credentials)"
   *  suffix in each affected row's aria-label. Empty set ⇒ no UI
   *  surface, panel renders like before. */
  const [missingIds, setMissingIds] = useState<Set<string>>(() => new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirmTarget, setConfirmTarget] = useState<Account | null>(null);
  /** When set, render the privacy-notice modal with the new
   *  account's adapter kind so the body text can mention the
   *  specific provider. Closed by acknowledging — which writes
   *  the `contacts.privacyNoticeAcknowledged` flag so future
   *  connects skip the modal. */
  const [privacyNoticeFor, setPrivacyNoticeFor] = useState<AdapterKind | null>(
    null,
  );

  // Empty until the host answers: the picker cannot preselect an adapter
  // before it knows which exist. Set to the first offered kind once it does.
  const [kind, setKind] = useState<AdapterKind>('');
  const [displayName, setDisplayName] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [testing, setTesting] = useState(false);
  const [discovering, setDiscovering] = useState(false);
  const [testMessage, setTestMessage] = useState<string | null>(null);
  const [caldav, setCaldav] = useState<CaldavFields>(EMPTY_CALDAV);
  const [ical, setIcal] = useState<IcalFields>(EMPTY_ICAL);
  const [google, setGoogle] = useState<GoogleFields>(EMPTY_GOOGLE);
  // The connect form for the selected kind, as that ADAPTER declares it. Null
  // while it is being fetched, or for the adapters still on the older per-kind
  // path below. Nothing in this component knows what any of the fields mean.
  // Which adapters this build can connect, straight from the host. Empty until
  // the first answer arrives; the picker simply has nothing to offer until then,
  // which is honest — it does not yet know.
  const [availableKinds, setAvailableKinds] = useState<AdapterKindInfo[]>([]);
  const [formSpec, setFormSpec] = useState<AccountFormSpec | null>(null);
  const [formValues, setFormValues] = useState<
    Record<string, string | boolean>
  >({});
  const [microsoft, setMicrosoft] = useState<MicrosoftFields>(EMPTY_MICROSOFT);
  const [ews, setEws] = useState<EwsFields>(EMPTY_EWS);
  const [vikunja, setVikunja] = useState<VikunjaFields>(EMPTY_VIKUNJA);
  const [todoist, setTodoist] = useState<TodoistFields>(EMPTY_TODOIST);

  const refresh = useCallback(() => {
    setLoading(true);
    setError(null);
    listAccounts()
      .then(setAccounts)
      .catch((err) => {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      })
      .finally(() => setLoading(false));
    // Re-probe the missing-credentials set in parallel. Failures
    // are logged but don't tank the panel — the banner is purely
    // additive, the list of accounts itself is what matters.
    listAccountsMissingCredentials()
      .then((missing) =>
        setMissingIds(new Set(missing.map((acc) => acc.id))),
      )
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('list_accounts_missing_credentials failed', err);
        setMissingIds(new Set());
      });
  }, []);

  // The panel mounts when the user lands on its tab, so "on mount" is
  // the right moment to fetch — no need to gate on an `isOpen` flag any
  // more (the host SettingsDialog handles open/close).
  useEffect(() => {
    refresh();
  }, [refresh]);

  // The adapter list is a property of the installed plugins, so it is fetched
  // rather than declared. Re-fetched on `dataVersion` because enabling or
  // disabling a plugin in Settings changes the answer.
  useEffect(() => {
    let cancelled = false;
    listAdapterKinds()
      .then((kinds) => {
        if (cancelled) return;
        const offered = kinds.filter((k) => !HOST_INTERNAL_KINDS.has(k.kind));
        setAvailableKinds(offered);
        // Preselect the first one, but never overwrite a choice the user has
        // already made — this effect re-runs whenever a plugin is toggled.
        setKind((current) =>
          offered.some((entry) => entry.kind === current)
            ? current
            : (offered[0]?.kind ?? ''),
        );
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('list_adapter_kinds failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, [dataVersion]);

  // Fetch the selected adapter's own connect form. Deferred to the moment a
  // kind is picked rather than fetched on mount: it is a question about one
  // adapter, and the answer includes whether this build carries credentials for
  // it, which can change between builds.
  //
  // The values reset with the spec, so switching kinds cannot carry a value
  // from one adapter's field into another adapter's field of the same name.
  useEffect(() => {
    let cancelled = false;
    setFormSpec(null);
    setFormValues({});
    accountFormSpec(kind, i18n.language)
      .then((spec) => {
        if (!cancelled) setFormSpec(spec);
      })
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('account_form_spec failed', err);
      });
    return () => {
      cancelled = true;
    };
  }, [kind, i18n.language]);

  // Closing the reconnect wizard bumps `dataVersion`. Re-probe so
  // rows that just got their credentials drop off the banner
  // without forcing the user to leave + re-enter the tab.
  useEffect(() => {
    if (dataVersion === 0) return;
    listAccountsMissingCredentials()
      .then((missing) =>
        setMissingIds(new Set(missing.map((acc) => acc.id))),
      )
      .catch((err) => {
        // eslint-disable-next-line no-console
        console.warn('list_accounts_missing_credentials failed', err);
      });
  }, [dataVersion]);

  /** Open the wizard with the subset of accounts that currently
   *  show as missing. Re-resolves against the live `accounts` /
   *  `missingIds` so a stale render can't accidentally hand the
   *  wizard a deleted row. */
  const openReconnectWizard = useCallback(() => {
    const targets = accounts.filter((acc) => missingIds.has(acc.id));
    if (targets.length === 0) return;
    openSyncAccountsConnect(targets);
  }, [accounts, missingIds, openSyncAccountsConnect]);

  const validateCaldav = useCallback((): string | null => {
    if (!caldav.serverUrl.trim()) return t('dialogs.accounts.serverUrlRequired');
    if (!caldav.username.trim()) return t('dialogs.accounts.usernameRequired');
    if (!caldav.password) return t('dialogs.accounts.passwordRequired');
    return null;
  }, [caldav, t]);

  const validateIcal = useCallback((): string | null => {
    // iCal only requires the URL — username + password are optional
    // (most public feeds are anonymous).
    if (!ical.feedUrl.trim()) return t('dialogs.accounts.feedUrlRequired');
    return null;
  }, [ical, t]);

  const validateGoogle = useCallback((): string | null => {
    if (!google.clientId.trim()) return t('dialogs.accounts.clientIdRequired');
    if (!google.clientSecret.trim())
      return t('dialogs.accounts.clientSecretRequired');
    return null;
  }, [google, t]);

  /** Validation for a schema-driven form: the adapter says what is required,
   *  so this needs no per-adapter branch either. */
  const validateSchemaForm = useCallback((): string | null => {
    if (!formSpec) return null;
    const missing = firstMissingField(formSpec, formValues);
    if (!missing) return null;
    // Already resolved by the host, in the plugin's own words.
    return t('dialogs.accounts.fieldRequired', { field: missing.label });
  }, [formSpec, formValues, t]);

  const validateMicrosoft = useCallback((): string | null => {
    if (!microsoft.clientId.trim())
      return t('dialogs.accounts.clientIdRequired');
    return null;
  }, [microsoft, t]);

  const validateEws = useCallback((): string | null => {
    if (!ews.endpoint.trim()) return t('dialogs.accounts.ewsEndpointRequired');
    if (!ews.username.trim()) return t('dialogs.accounts.usernameRequired');
    if (!ews.password) return t('dialogs.accounts.passwordRequired');
    return null;
  }, [ews, t]);

  const validateVikunja = useCallback((): string | null => {
    if (!vikunja.serverUrl.trim())
      return t('dialogs.accounts.vikunjaServerUrlRequired');
    if (!vikunja.apiToken.trim())
      return t('dialogs.accounts.vikunjaApiTokenRequired');
    return null;
  }, [vikunja, t]);

  const validateTodoist = useCallback((): string | null => {
    if (!todoist.apiToken.trim())
      return t('dialogs.accounts.todoistApiTokenRequired');
    return null;
  }, [todoist, t]);

  const onSubmit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      setTestMessage(null);
      const name = displayName.trim();
      if (!name) {
        setError(t('dialogs.accounts.nameRequired'));
        return;
      }
      if (!availableKinds.some((entry) => entry.kind === kind)) {
        // Defence in depth: the picker only offers what the host reported, but
        // a plugin can be switched off between opening the form and submitting.
        setError(t('dialogs.accounts.kindUnavailable'));
        return;
      }
      if (kind === 'caldav' && !formSpec) {
        const v = validateCaldav();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'ical' && !formSpec) {
        const v = validateIcal();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'google' && !formSpec) {
        const v = validateGoogle();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'microsoft_graph' && !formSpec) {
        const v = validateMicrosoft();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'ews' && !formSpec) {
        const v = validateEws();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'vikunja' && !formSpec) {
        const v = validateVikunja();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'todoist' && !formSpec) {
        const v = validateTodoist();
        if (v) {
          setError(v);
          return;
        }
      }
      if (formSpec) {
        const v = validateSchemaForm();
        if (v) {
          setError(v);
          return;
        }
      }
      setSubmitting(true);
      setError(null);
      try {
        let created;
        if (kind === 'google') {
          // Google takes its own path: the backend opens the system
          // browser, runs the PKCE dance and only then writes the
          // account row + secrets. Returns the same Account shape
          // as createAccount so the rest of the flow is identical.
          //
          // The INPUTS come from whichever form is on screen. Google now
          // declares a schema, so the generic form owns the two client fields
          // and the legacy state is never filled; reading it here would send
          // two empty strings into the sign-in. Moving the whole flow onto
          // `connectAccount` — which already runs OAuth from the schema, as it
          // does for Webex — is the next step and wants a live test first.
          const clientId = formSpec
            ? String(formValues.client_id ?? '').trim()
            : google.clientId.trim();
          const clientSecret = formSpec
            ? String(formValues.client_secret ?? '').trim()
            : google.clientSecret.trim();
          created = await connectGoogleAccount(clientId, clientSecret, name);
        } else if (kind === 'microsoft_graph') {
          // Same OAuth-then-persist flow as Google, minus the
          // client_secret (Microsoft honours PKCE for public clients).
          const clientId = formSpec
            ? String(formValues.client_id ?? '').trim()
            : microsoft.clientId.trim();
          const authority = formSpec
            ? String(formValues.authority ?? '').trim()
            : microsoft.authority.trim();
          created = await connectMicrosoftAccount(
            clientId,
            name,
            authority || undefined,
          );
        } else if (formSpec) {
          // The adapter declared its own form, so creating the account is one
          // generic call. Which values are secrets, where they are kept and
          // whether an OAuth sign-in runs first are all decided by the schema,
          // on the Rust side, from what this adapter published.
          created = await connectAccount({
            adapter_kind: kind,
            display_name: name,
            values: collectValues(formSpec, formValues),
          });
        } else {
          const configJson =
            kind === 'caldav'
              ? JSON.stringify({
                  server_url: caldav.serverUrl.trim(),
                  username: caldav.username.trim(),
                  auth_kind: 'basic',
                })
              : kind === 'ical'
                ? JSON.stringify({
                    feed_url: ical.feedUrl.trim(),
                    username: ical.username.trim() || null,
                  })
                : kind === 'ews'
                  ? JSON.stringify({
                      endpoint: ews.endpoint.trim(),
                      username: ews.username.trim(),
                    })
                  : kind === 'vikunja'
                    ? JSON.stringify({
                        server_url: vikunja.serverUrl.trim(),
                      })
                    : kind === 'todoist'
                      ? '{}'
                      : '{}';
          const secret =
            kind === 'caldav'
              ? caldav.password
              : kind === 'ical' && ical.password
                ? ical.password
                : kind === 'ews'
                  ? ews.password
                  : kind === 'vikunja'
                    ? vikunja.apiToken.trim()
                    : kind === 'todoist'
                      ? todoist.apiToken.trim()
                      : undefined;
          created = await createAccount({
            adapter_kind: kind,
            display_name: name,
            config_json: configJson,
            secret,
          });
        }
        announce(t('dialogs.accounts.created', { name: created.display_name }));
        // Phase 10k privacy notice (DESIGN.md §10.6): show the
        // one-shot modal the first time the user connects an
        // account whose ContactsFeature impl will pull remote
        // address-book data. Skipped on the local kind (no
        // remote pull) and on subsequent connects (the prefs
        // flag is sticky once set). The user_pref read happens
        // here rather than at panel mount because the check is
        // only meaningful at the moment of a contacts-capable
        // connect — pre-fetching on mount would mean an extra
        // Tauri round-trip on every Settings open.
        if (CONTACTS_CAPABLE_KINDS.has(kind)) {
          try {
            const acknowledged = await getUserPref(PREF_PRIVACY_NOTICE_ACK);
            if (acknowledged !== 'true') {
              setPrivacyNoticeFor(kind);
            }
          } catch (err) {
            // Treat a failed pref read as "already acknowledged"
            // so a misbehaving DB doesn't block account creation
            // with a stuck modal. The notice will surface again
            // on the next connect if the read recovers.
            // eslint-disable-next-line no-console
            console.warn('privacy notice pref read failed', err);
          }
        }
        setDisplayName('');
        setCaldav(EMPTY_CALDAV);
        setIcal(EMPTY_ICAL);
        setGoogle(EMPTY_GOOGLE);
        setMicrosoft(EMPTY_MICROSOFT);
        setEws(EMPTY_EWS);
        setVikunja(EMPTY_VIKUNJA);
        setTodoist(EMPTY_TODOIST);
        setFormValues({});
        refresh();
        // Re-fetch the calendar / task-list catalog so the sidebar
        // picks up the new account's containers without the user
        // having to open another dialog (or restart the app). Errors
        // here are non-fatal — the account row was already
        // persisted; a stale sidebar fixes itself on the next normal
        // store refresh.
        //
        // Contacts: the address-book CATALOG is fetched on-demand just
        // like calendars (the host's `list_contact_lists` has a blocking
        // cold path), so `refreshContactLists` is what makes the new
        // account's books appear in the sidebar — without it the sidebar
        // shows no contacts section until the app restarts. We ALSO kick
        // `syncContactsNow` to pre-warm each book's contents (respecting
        // the user's "include read-only directories" pref) so opening the
        // Contacts view is instant.
        //
        // An account that owns no calendars and no task lists refreshes only
        // the account list — the two catalog calls have a blocking cold path
        // and there would be nothing at the end of them. Which adapters those
        // are comes from the adapter's own declaration, not from a list here.
        const refreshes: Promise<unknown>[] =
          formSpec && !formSpec.owns_containers
            ? [refreshAccounts()]
            : [refreshAccounts(), refreshCalendars(), refreshTaskLists()];
        if (CONTACTS_CAPABLE_KINDS.has(kind)) {
          refreshes.push(refreshContactLists(), syncContactsNow());
        }
        void Promise.allSettled(refreshes);
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      } finally {
        setSubmitting(false);
      }
    },
    [
      availableKinds,
      displayName,
      kind,
      formSpec,
      formValues,
      validateSchemaForm,
      caldav,
      ical,
      google,
      microsoft,
      ews,
      vikunja,
      todoist,
      validateCaldav,
      validateIcal,
      validateGoogle,
      validateMicrosoft,
      validateEws,
      validateVikunja,
      validateTodoist,
      announce,
      refresh,
      refreshAccounts,
      refreshCalendars,
      refreshTaskLists,
      refreshContactLists,
      t,
    ],
  );

  const onTestConnection = useCallback(async () => {
    setTestMessage(null);
    setError(null);
    setTesting(true);
    // Whichever form is on screen owns the values. Once an adapter declares its
    // own schema the generic form fills `formValues` and the legacy state stays
    // empty, so a probe that read the legacy state would refuse a connection
    // that is perfectly well typed in. (Declaring this button in the manifest,
    // so the host stops naming adapters here at all, is the next piece of work.)
    const field = (key: string, legacy: string) =>
      formSpec ? String(formValues[key] ?? '') : legacy;
    try {
      if (kind === 'caldav') {
        const v = formSpec ? validateSchemaForm() : validateCaldav();
        if (v) {
          setError(v);
          return;
        }
        await testCaldavConnection(
          field('server_url', caldav.serverUrl).trim(),
          field('username', caldav.username).trim(),
          field('secret', caldav.password),
        );
      } else if (kind === 'ical') {
        const v = formSpec ? validateSchemaForm() : validateIcal();
        if (v) {
          setError(v);
          return;
        }
        await testIcalFeed(
          field('feed_url', ical.feedUrl).trim(),
          field('username', ical.username).trim() || null,
          field('password', ical.password) || null,
        );
      } else if (kind === 'ews') {
        const v = formSpec ? validateSchemaForm() : validateEws();
        if (v) {
          setError(v);
          return;
        }
        await testEwsConnection(
          field('endpoint', ews.endpoint).trim(),
          field('username', ews.username).trim(),
          field('password', ews.password),
        );
      } else if (kind === 'vikunja') {
        const v = formSpec ? validateSchemaForm() : validateVikunja();
        if (v) {
          setError(v);
          return;
        }
        await testVikunjaConnection(
          field('server_url', vikunja.serverUrl).trim(),
          field('token', vikunja.apiToken).trim(),
        );
      } else if (kind === 'todoist') {
        const v = formSpec ? validateSchemaForm() : validateTodoist();
        if (v) {
          setError(v);
          return;
        }
        await testTodoistConnection(field('token', todoist.apiToken).trim());
      } else {
        return;
      }
      setTestMessage(t('dialogs.accounts.testOk'));
      announce(t('dialogs.accounts.testOk'));
    } catch (err) {
      if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
      else setError(String(err));
    } finally {
      setTesting(false);
    }
  }, [
    kind,
    caldav,
    ical,
    ews,
    vikunja,
    todoist,
    validateCaldav,
    validateIcal,
    validateEws,
    validateVikunja,
    validateTodoist,
    validateSchemaForm,
    formSpec,
    formValues,
    announce,
    t,
  ]);

  /**
   * Run POX-Autodiscover for the e-mail in the username field and
   * pre-fill the endpoint URL on success. Requires the user to have
   * typed at least the username + password — the discovery probe
   * authenticates with Basic auth using the same credentials the
   * eventual EWS calls will use.
   *
   * Failure-mode UX: we set `error` so the existing error region
   * surfaces it (with focus restored), and we leave the endpoint
   * field alone so the user can keep typing it manually.
   */
  const onDiscover = useCallback(async () => {
    setTestMessage(null);
    setError(null);
    // Autodiscover reads and writes the SAME two fields whichever form is on
    // screen. Once EWS declares its own schema the generic form owns the
    // values, and the legacy state is not being filled at all — reading it
    // there would find empty strings and refuse a probe that would have worked.
    // (Declaring this button in the manifest, so the host stops naming EWS at
    // all, is the next piece of work.)
    const onSchema = formSpec != null;
    const username = (
      onSchema ? String(formValues.username ?? '') : ews.username
    ).trim();
    const password = onSchema
      ? String(formValues.password ?? '')
      : ews.password;
    if (!username) {
      setError(t('dialogs.accounts.ewsDiscoverNeedsEmail'));
      return;
    }
    if (!password) {
      setError(t('dialogs.accounts.ewsDiscoverNeedsPassword'));
      return;
    }
    setDiscovering(true);
    try {
      const result = await discoverEwsEndpoint(username, password);
      // If autodiscover took us through a RedirectAddr step, the canonical
      // login is the one we actually authenticated as — surface that to the
      // user so the password they typed matches the username we stored.
      if (onSchema) {
        setFormValues((prev) => ({
          ...prev,
          endpoint: result.ews_url,
          username: result.account_email,
        }));
      } else {
        setEws((prev) => ({
          ...prev,
          endpoint: result.ews_url,
          username: result.account_email,
        }));
      }
      const okMsg = t('dialogs.accounts.ewsDiscoverOk', {
        url: result.ews_url,
      });
      setTestMessage(okMsg);
      announce(okMsg);
    } catch (err) {
      if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
      else setError(String(err));
    } finally {
      setDiscovering(false);
    }
  }, [ews, formSpec, formValues.username, formValues.password, announce, t]);

  // Armed when the user triggers a delete from the listbox. The
  // post-refresh effect below sees the flag, moves focus back onto
  // the listbox so the user can keep arrow-navigating, then clears
  // it. Crucially the flag stays false on initial mount and on
  // create — those must not steal focus from whatever the caller
  // had focused (the Settings tab, the new-account form, …).
  const refocusListAfterReloadRef = useRef(false);

  const performDelete = useCallback(
    async (acc: Account) => {
      setError(null);
      try {
        await deleteAccount(acc.id);
        announce(t('dialogs.accounts.deleted', { name: acc.display_name }));
        refocusListAfterReloadRef.current = true;
        refresh();
        // Same as on create: nudge the store so the just-removed
        // account's calendars, task lists and address books disappear
        // from the sidebar.
        void Promise.allSettled([
          refreshAccounts(),
          refreshCalendars(),
          refreshTaskLists(),
          refreshContactLists(),
        ]);
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      }
    },
    [
      announce,
      refresh,
      refreshAccounts,
      refreshCalendars,
      refreshTaskLists,
      refreshContactLists,
      t,
    ],
  );

  // Force a full cold re-sync of one external account: clears its delta tokens +
  // cached window across every container, then kicks a warm pass so each
  // re-bootstraps from the provider. The recovery path for a "stuck" external
  // cache — a bootstrap that enumerated an INCOMPLETE resource set yet persisted
  // a sync-token, so later deltas reported "no changes" over permanently-missing
  // events. Credentials are untouched, so there's no app-specific-password
  // re-entry (the whole reason we don't just tell the user to re-add the account).
  const [resyncing, setResyncing] = useState(false);
  const onResetSync = useCallback(
    async (acc: Account) => {
      setError(null);
      setResyncing(true);
      try {
        await resetAccountSync(acc.id);
        announce(t('dialogs.accounts.resyncStarted', { name: acc.display_name }));
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      } finally {
        setResyncing(false);
      }
    },
    [announce, t],
  );

  const headingId = useId();
  const optionId = (i: number) => `${headingId}-acc-${i}`;

  // Two-track focus model:
  //  - When the list has rows, the <ul> takes the tab stop and uses
  //    aria-activedescendant to spotlight the focused row. Arrow keys
  //    move the spotlight, Enter / Delete open the confirm dialog
  //    (no-op for the local row).
  //  - When the list is empty, the surrounding <section> takes the
  //    tab stop so the screen reader still hears the heading and the
  //    empty-state hint instead of skipping straight to the form.
  const sectionRef = useRef<HTMLElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  // Surface form-validation / backend errors to keyboard + SR users
  // by moving focus onto the alert region the moment it appears.
  // `role="alert"` already triggers an aria-live announcement on
  // its own, but NVDA's focus mode (which is what the rest of the
  // app uses) doesn't let the user navigate to prose without
  // dropping into browse mode. Focusing the region directly lets
  // them re-read it via Shift+arrow keys without a mode switch,
  // and gives sighted keyboard users a clear visual landing point.
  const errorRef = useRef<HTMLParagraphElement>(null);
  const [focusIndex, setFocusIndex] = useState(0);
  // The refresh-error details are focus stops that a background refetch
  // (60s poll / pass end) can UNMOUNT while they hold keyboard focus —
  // the errors just cleared. Focus would fall to <body>, outside the
  // application role, dropping NVDA out of focus mode with the dialog's
  // key handlers dead (the stranded-focus class of a3746ada). Catch the
  // drop after each error-set change and put focus back on the list.
  useEffect(() => {
    if (
      document.activeElement === document.body ||
      document.activeElement === null
    ) {
      listRef.current?.focus({ preventScroll: true });
    }
  }, [errorsByAccount]);
  // Track whether the listbox currently owns DOM focus. Used to gate
  // `aria-activedescendant` so it only points at an option while the
  // listbox is actually focused. With the attribute set unconditionally
  // some screen readers (NVDA in particular) treat the listbox as the
  // current focus target the moment it enters the accessibility tree
  // and start reading the topmost option, even though keyboard focus
  // is still on the Settings tab. The gating turns that into a no-op.
  const [listHasFocus, setListHasFocus] = useState(false);
  // Inline rename (F2 on the focused row). `editingId` drives which
  // row renders an <input>; `editingRef` mirrors it synchronously so
  // commit-on-blur and the focus-return blur can't double-fire (state
  // updates are async, the ref is not).
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editDraft, setEditDraft] = useState('');
  const editingRef = useRef(false);
  const editInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (focusIndex >= accounts.length) {
      setFocusIndex(Math.max(0, accounts.length - 1));
    }
  }, [accounts.length, focusIndex]);

  // Focus + select the rename input when it appears.
  useEffect(() => {
    if (editingId) {
      editInputRef.current?.focus();
      editInputRef.current?.select();
    }
  }, [editingId]);

  // After a delete completes (refresh has finished, `loading` flipped
  // back to false) the row that was focused no longer exists — its
  // delete button got unmounted along with it, so the Modal's focus
  // restoration ends up at <body>. Pull focus back onto the listbox
  // so the user can keep arrow-navigating. On initial load and on
  // create we leave focus alone: the SettingsDialog tab keeps it on
  // entry, the form keeps it after add.
  useEffect(() => {
    if (loading) return;
    if (!refocusListAfterReloadRef.current) return;
    refocusListAfterReloadRef.current = false;
    (listRef.current ?? sectionRef.current)?.focus({ preventScroll: true });
  }, [loading, accounts.length]);

  // Pull focus to the error region whenever it transitions from
  // empty → set. See the `errorRef` comment for the rationale.
  // `preventScroll: true` keeps the dialog viewport stable when the
  // alert is already on screen.
  useEffect(() => {
    if (error && errorRef.current) {
      errorRef.current.focus({ preventScroll: true });
    }
  }, [error]);

  const isLocalAt = (i: number): boolean => {
    const acc = accounts[i];
    return !!acc && acc.adapter_kind === 'local' && acc.id === 'local';
  };

  const tryDelete = useCallback(
    (i: number) => {
      const acc = accounts[i];
      if (!acc) return;
      if (acc.adapter_kind === 'local' && acc.id === 'local') {
        announce(t('dialogs.accounts.localCannotDelete'));
        return;
      }
      setConfirmTarget(acc);
    },
    [accounts, announce, t],
  );

  const startRename = (i: number) => {
    const acc = accounts[i];
    if (!acc) return;
    setFocusIndex(i);
    setEditDraft(acc.display_name);
    editingRef.current = true;
    setEditingId(acc.id);
  };

  // Close the inline editor. `commit` saves the trimmed draft (when it
  // actually changed); either way focus returns to the listbox. The
  // ref guard makes this idempotent so the focus-return blur is a no-op.
  const finishRename = (commit: boolean) => {
    if (!editingRef.current) return;
    editingRef.current = false;
    const id = editingId;
    const name = editDraft.trim();
    const current = id ? accounts.find((a) => a.id === id) : undefined;
    setEditingId(null);
    listRef.current?.focus({ preventScroll: true });
    if (!commit || !id || !name) return;
    if (current && name === current.display_name) return;
    void (async () => {
      try {
        await renameAccount(id, name);
        announce(t('dialogs.accounts.renamed', { name }));
        refresh();
        refreshAccounts();
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      }
    })();
  };

  const handleListKey = (e: React.KeyboardEvent<HTMLUListElement>) => {
    // While the inline editor is open the keydown bubbles up from the
    // <input>; ignore it here so Enter/Delete don't also fire delete.
    if (editingRef.current) return;
    if (e.key === 'F2') {
      e.preventDefault();
      startRename(focusIndex);
      return;
    }
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    if (accounts.length === 0) return;
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setFocusIndex((i) => Math.min(i + 1, accounts.length - 1));
        return;
      case 'ArrowUp':
        e.preventDefault();
        setFocusIndex((i) => Math.max(i - 1, 0));
        return;
      case 'Home':
        e.preventDefault();
        setFocusIndex(0);
        return;
      case 'End':
        e.preventDefault();
        setFocusIndex(accounts.length - 1);
        return;
      case 'Enter':
      case ' ':
      case 'Spacebar':
      case 'Delete':
      case 'Backspace':
        e.preventDefault();
        tryDelete(focusIndex);
        return;
      default:
        return;
    }
  };

  const sectionTabIndex = accounts.length === 0 && !loading ? 0 : -1;

  return (
    <>
      <div className="form">
        {/* The "Loading accounts …" / "5 accounts." live region we used
            to render here was useful when this panel lived in its own
            modal — opening the modal explicitly meant the user wanted
            that summary. Inside the Settings tab it just fires on
            every tab switch, drowning the user in count chatter they
            didn't ask for. The information stays reachable via the
            listbox semantics: NVDA already announces "1 of 5" when
            focus enters the list. Create / delete confirmations go
            through the global Announcer, so the events that actually
            matter still surface. */}

        {error && (
          <p
            ref={errorRef}
            role="alert"
            tabIndex={-1}
            className="form__error"
          >
            {error}
          </p>
        )}

        {/* §19.11 step 8 — banner for accounts whose credentials
            are missing on this device. Shown above the listbox so
            it lives in tab order before the rows and can be
            actioned without arrow-navigating into the list first.
            The "Connect" button opens the existing reconnect
            wizard with the matching subset. */}
        {missingIds.size > 0 && (
          <section
            aria-labelledby={`${headingId}-missing`}
            className="accounts-missing-banner"
            role="status"
          >
            <p id={`${headingId}-missing`} className="form__hint">
              {missingIds.size === 1
                ? t('dialogs.accounts.missingCredentials_one')
                : t('dialogs.accounts.missingCredentials_other', {
                    count: missingIds.size,
                  })}
            </p>
            <button
              type="button"
              className="form__action"
              onClick={openReconnectWizard}
            >
              {t('dialogs.accounts.missingCredentialsConnect')}
            </button>
          </section>
        )}

        <section
          ref={sectionRef}
          tabIndex={sectionTabIndex}
          aria-labelledby={`${headingId}-list`}
          aria-describedby={
            accounts.length === 0 ? `${headingId}-empty` : undefined
          }
          className="accounts-list-section"
        >
          <h3 id={`${headingId}-list`} className="form__label">
            {t('dialogs.accounts.existingHeading')}
          </h3>
          {accounts.length > 0 && (
            <p id={`${headingId}-hint`} className="form__hint">
              {t('dialogs.accounts.manageHint')}
            </p>
          )}
          {accounts.length === 0 && !loading ? (
            <p id={`${headingId}-empty`} className="form__hint">
              {t('dialogs.accounts.empty')}
            </p>
          ) : (
            <ul
              ref={listRef}
              role="listbox"
              tabIndex={0}
              aria-label={t('dialogs.accounts.listLabel')}
              aria-describedby={`${headingId}-hint`}
              aria-activedescendant={
                listHasFocus && accounts.length > 0
                  ? optionId(focusIndex)
                  : undefined
              }
              onFocus={() => setListHasFocus(true)}
              onBlur={() => setListHasFocus(false)}
              onKeyDown={handleListKey}
              className="accounts-list"
            >
              {accounts.map((acc, i) => {
                const isLocal = isLocalAt(i);
                const focused = i === focusIndex;
                const needsConnect = missingIds.has(acc.id);
                // §20.8 — the backend enriches each row with
                // a `plugin_loaded` flag. `false` here means
                // the adapter_kind maps to a plugin id that
                // isn't loaded (uninstalled, disabled, or
                // never installed on this device). Local
                // accounts ship with plugin_loaded=true even
                // though they're host-internal; the missing
                // signal is the broader "the wire payload
                // omitted the field" case which we treat as
                // backwards-compatible "fine".
                const needsPlugin = acc.plugin_loaded === false;
                // Pick the row label template: local rows get the
                // special "can't be deleted" copy; remote rows
                // pick up the "needs credentials" variant when
                // the keychain probe came back empty, so SR users
                // hear the state as they arrow through. The
                // missing-plugin state takes precedence over the
                // missing-credentials state — without the plugin,
                // re-entering credentials wouldn't help.
                const rowLabelKey = isLocal
                  ? 'dialogs.accounts.rowLabelLocal'
                  : needsPlugin
                    ? 'dialogs.accounts.rowLabelMissingPlugin'
                    : needsConnect
                      ? 'dialogs.accounts.rowLabelMissing'
                      : 'dialogs.accounts.rowLabel';
                return (
                  <li
                    key={acc.id}
                    id={optionId(i)}
                    role="option"
                    // Only expose the selection state while the listbox
                    // is focused. With `aria-selected="true"` set on the
                    // first row from the moment the listbox enters the
                    // a11y tree, NVDA reads that row aloud on every tab
                    // switch — even though keyboard focus is still on
                    // the Settings tab. When the user actually tabs
                    // into the list, listHasFocus flips and the right
                    // row gets aria-selected back.
                    aria-selected={listHasFocus ? focused : undefined}
                    aria-label={
                      t(rowLabelKey, {
                        name: acc.display_name,
                        kind: t(`dialogs.accounts.kindName.${acc.adapter_kind}`),
                      }) +
                      (errorsByAccount.has(acc.id)
                        ? ' ' +
                          t(
                            errorsByAccount.get(acc.id)?.auth_suspected
                              ? 'sidebar.tree.refreshErrorAuth'
                              : 'sidebar.tree.refreshError',
                          )
                        : '')
                    }
                    className={
                      'accounts-list__item' +
                      (focused ? ' accounts-list__item--focused' : '') +
                      (needsConnect ? ' accounts-list__item--needs-connect' : '') +
                      (needsPlugin ? ' accounts-list__item--needs-plugin' : '')
                    }
                    onClick={() => {
                      // Ignore row clicks while inline-editing — the
                      // click bubbles from the rename <input>.
                      if (editingRef.current) return;
                      // A click only SELECTS the row (like arrowing to it).
                      // Deleting is a deliberate action — the Delete key (per
                      // the list hint) or the explicit "Delete account" button
                      // in the detail area below — never an accidental click.
                      setFocusIndex(i);
                    }}
                  >
                    {editingId === acc.id ? (
                      <input
                        ref={editInputRef}
                        type="text"
                        className="accounts-list__rename-input"
                        value={editDraft}
                        aria-label={t('dialogs.accounts.renameLabel', {
                          name: acc.display_name,
                        })}
                        onChange={(e) => setEditDraft(e.target.value)}
                        onClick={(e) => e.stopPropagation()}
                        onKeyDown={(e) => {
                          e.stopPropagation();
                          if (e.key === 'Enter') {
                            e.preventDefault();
                            finishRename(true);
                          } else if (e.key === 'Escape') {
                            e.preventDefault();
                            finishRename(false);
                          }
                        }}
                        onBlur={() => finishRename(true)}
                      />
                    ) : (
                      <span className="accounts-list__name">
                        {acc.display_name}
                      </span>
                    )}
                    <span className="accounts-list__kind">
                      {t(`dialogs.accounts.kindName.${acc.adapter_kind}`)}
                    </span>
                    {needsPlugin && (
                      <span
                        className="accounts-list__badge accounts-list__badge--plugin"
                        aria-hidden="true"
                      >
                        {t('dialogs.accounts.missingPluginBadge')}
                      </span>
                    )}
                    {!needsPlugin && needsConnect && (
                      <span
                        className="accounts-list__badge"
                        aria-hidden="true"
                      >
                        {t('dialogs.accounts.missingBadge')}
                      </span>
                    )}
                    {errorsByAccount.has(acc.id) && (
                      <span
                        className="accounts-list__badge accounts-list__badge--refresh-error"
                        aria-hidden="true"
                      >
                        ⚠️ {t('dialogs.accounts.refreshErrors.badge')}
                      </span>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {/* Refresh-error DETAILS for the focused account: which containers
              are failing, the provider's error text, and how stale the data
              the user currently sees is (last successful refresh). This panel
              lives inside the Settings Modal whose body is role="application"
              (NVDA stays in focus mode), so every line must be a FOCUS STOP —
              static markup would be invisible to NVDA here (see
              FocusableNote.tsx). Auth-shaped errors additionally get a real
              re-enter-password button: the reconnect wizard accepts any
              account, but its banner entry point only appears for MISSING
              credentials, so a present-but-wrong password needs this path. */}
          {accounts[focusIndex] &&
            errorsByAccount.has(accounts[focusIndex].id) && (
              <section
                className="accounts-refresh-errors"
                aria-label={t('dialogs.accounts.refreshErrors.heading', {
                  name: accounts[focusIndex].display_name,
                })}
              >
                <h4
                  tabIndex={0}
                  aria-label={t('dialogs.accounts.refreshErrors.heading', {
                    name: accounts[focusIndex].display_name,
                  })}
                >
                  {t('dialogs.accounts.refreshErrors.heading', {
                    name: accounts[focusIndex].display_name,
                  })}
                </h4>
                {errorsByAccount.get(accounts[focusIndex].id)
                  ?.auth_suspected && (
                  <FocusableNote className="accounts-refresh-errors__auth-hint">
                    {t(
                      accounts[focusIndex].adapter_kind === 'google' ||
                        accounts[focusIndex].adapter_kind === 'microsoft_graph'
                        ? 'dialogs.accounts.refreshErrors.authHintOauth'
                        : 'dialogs.accounts.refreshErrors.authHint',
                    )}
                  </FocusableNote>
                )}
                <ul>
                  {errorsByAccount
                    .get(accounts[focusIndex].id)
                    ?.errors.map((err) => {
                      const line = `${t(
                        'dialogs.accounts.refreshErrors.entry',
                        {
                          container:
                            err.container_name ??
                            t(
                              `dialogs.accounts.refreshErrors.scope.${err.scope}`,
                              { defaultValue: err.scope },
                            ),
                          error: clampErrorText(err.error),
                        },
                      )} ${
                        err.last_success_at
                          ? t('dialogs.accounts.refreshErrors.lastSuccess', {
                              time: new Date(
                                err.last_success_at,
                              ).toLocaleString(i18n.language, {
                                dateStyle: 'long',
                                timeStyle: 'short',
                              }),
                            })
                          : t('dialogs.accounts.refreshErrors.neverSucceeded')
                      }`;
                      return (
                        <li
                          key={`${err.scope}:${err.container_id}`}
                          tabIndex={0}
                          aria-label={line}
                        >
                          {line}
                        </li>
                      );
                    })}
                </ul>
                {errorsByAccount.get(accounts[focusIndex].id)
                  ?.auth_suspected && (
                  <button
                    type="button"
                    className="form__action accounts-refresh-errors__reconnect"
                    onClick={() =>
                      openSyncAccountsConnect([accounts[focusIndex]], 'repair')
                    }
                  >
                    {t(
                      accounts[focusIndex].adapter_kind === 'google' ||
                        accounts[focusIndex].adapter_kind === 'microsoft_graph'
                        ? 'dialogs.accounts.refreshErrors.signInAgain'
                        : 'dialogs.accounts.refreshErrors.reenterPassword',
                    )}
                  </button>
                )}
              </section>
            )}
          {/* Per-account recovery: force a full cold re-sync of the FOCUSED
              external account. Clears its delta tokens + cached window so the
              next refresh re-bootstraps the whole collection from the provider —
              fixes a "stuck" cache where a bootstrap cached an incomplete set as
              complete (events that exist on the device but never show here).
              Local accounts have no external cache, so it's offered only for
              externals; credentials are untouched (no re-auth). */}
          {accounts[focusIndex] && !isLocalAt(focusIndex) && (
            <button
              type="button"
              className="form__action accounts-list__resync"
              onClick={() => void onResetSync(accounts[focusIndex])}
              aria-disabled={resyncing || undefined}
            >
              {resyncing
                ? t('dialogs.accounts.resyncing')
                : t('dialogs.accounts.forceResync', {
                    name: accounts[focusIndex].display_name,
                  })}
            </button>
          )}
          {/* Explicit delete affordance for the focused external account — the
              pointer path now that a row click only selects. Keyboard users
              still delete via the Delete key (see the list hint). */}
          {accounts[focusIndex] && !isLocalAt(focusIndex) && (
            <button
              type="button"
              className="form__action form__action--danger accounts-list__delete"
              onClick={() => tryDelete(focusIndex)}
            >
              {t('dialogs.accounts.deleteAccount', {
                name: accounts[focusIndex].display_name,
              })}
            </button>
          )}
        </section>

        <section aria-labelledby={`${headingId}-add`} className="accounts-add-section">
          <h3 id={`${headingId}-add`} className="form__label">
            {t('dialogs.accounts.addHeading')}
          </h3>
          <form onSubmit={onSubmit} className="form">
            <label className="form__field">
              <span className="form__label">
                {t('dialogs.accounts.kindLabel')}
              </span>
              <select
                value={kind}
                onChange={(e) => setKind(e.target.value as AdapterKind)}
              >
                {/* Whatever the host reported. A bundled adapter gets its
                    translated name; anything else falls back to the plugin's
                    own, which beats a missing-key marker. */}
                {availableKinds.map((entry) => (
                  <option key={entry.kind} value={entry.kind}>
                    {t(`dialogs.accounts.kindName.${entry.kind}`, {
                      defaultValue: entry.name,
                    })}
                  </option>
                ))}
              </select>
            </label>

            <label className="form__field">
              <span className="form__label">
                {t('dialogs.accounts.nameLabel')}
              </span>
              <input
                type="text"
                value={displayName}
                onChange={(e) => setDisplayName(e.target.value)}
                placeholder={t('dialogs.accounts.namePlaceholder')}
                autoComplete="off"
                required
              />
            </label>

            {kind === 'caldav' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.serverUrlLabel')}
                  </span>
                  <input
                    type="url"
                    value={caldav.serverUrl}
                    onChange={(e) =>
                      setCaldav((prev) => ({
                        ...prev,
                        serverUrl: e.target.value,
                      }))
                    }
                    placeholder={t('dialogs.accounts.serverUrlPlaceholder')}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.serverUrlHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.usernameLabel')}
                  </span>
                  <input
                    type="text"
                    value={caldav.username}
                    onChange={(e) =>
                      setCaldav((prev) => ({
                        ...prev,
                        username: e.target.value,
                      }))
                    }
                    autoComplete="username"
                    spellCheck={false}
                    required
                  />
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.passwordLabel')}
                  </span>
                  <input
                    type="password"
                    value={caldav.password}
                    onChange={(e) =>
                      setCaldav((prev) => ({
                        ...prev,
                        password: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.passwordHint')}
                  </span>
                </label>
                {testMessage && kind === 'caldav' && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            {kind === 'ical' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.feedUrlLabel')}
                  </span>
                  <input
                    type="url"
                    value={ical.feedUrl}
                    onChange={(e) =>
                      setIcal((prev) => ({
                        ...prev,
                        feedUrl: e.target.value,
                      }))
                    }
                    placeholder={t('dialogs.accounts.feedUrlPlaceholder')}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.feedUrlHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.icalUsernameLabel')}
                  </span>
                  <input
                    type="text"
                    value={ical.username}
                    onChange={(e) =>
                      setIcal((prev) => ({
                        ...prev,
                        username: e.target.value,
                      }))
                    }
                    autoComplete="username"
                    spellCheck={false}
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.icalAuthHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.icalPasswordLabel')}
                  </span>
                  <input
                    type="password"
                    value={ical.password}
                    onChange={(e) =>
                      setIcal((prev) => ({
                        ...prev,
                        password: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                  />
                </label>
                {testMessage && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            {kind === 'google' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.googleClientIdLabel')}
                  </span>
                  <input
                    type="text"
                    value={google.clientId}
                    onChange={(e) =>
                      setGoogle((prev) => ({
                        ...prev,
                        clientId: e.target.value,
                      }))
                    }
                    placeholder={t(
                      'dialogs.accounts.googleClientIdPlaceholder',
                    )}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.googleClientIdHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.googleClientSecretLabel')}
                  </span>
                  <input
                    type="text"
                    value={google.clientSecret}
                    onChange={(e) =>
                      setGoogle((prev) => ({
                        ...prev,
                        clientSecret: e.target.value,
                      }))
                    }
                    placeholder={t(
                      'dialogs.accounts.googleClientSecretPlaceholder',
                    )}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.googleClientSecretHint')}
                  </span>
                </label>
                <FocusableNote className="form__hint accounts-google-flow-hint">
                  {t('dialogs.accounts.googleFlowHint')}
                </FocusableNote>
              </>
            )}

            {/* Adapters that declare their own connect form render it
                straight from the declaration — no branch here, and none needed
                when the next adapter arrives. */}
            {formSpec && (
              <AccountSchemaForm
                spec={formSpec}
                values={formValues}
                onChange={(key, value) =>
                  setFormValues((prev) => ({ ...prev, [key]: value }))
                }
              />
            )}

            {kind === 'microsoft_graph' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.microsoftClientIdLabel')}
                  </span>
                  <input
                    type="text"
                    value={microsoft.clientId}
                    onChange={(e) =>
                      setMicrosoft((prev) => ({
                        ...prev,
                        clientId: e.target.value,
                      }))
                    }
                    placeholder={t(
                      'dialogs.accounts.microsoftClientIdPlaceholder',
                    )}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.microsoftClientIdHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.microsoftAuthorityLabel')}
                  </span>
                  <select
                    value={microsoft.authority}
                    onChange={(e) =>
                      setMicrosoft((prev) => ({
                        ...prev,
                        authority: e.target.value,
                      }))
                    }
                  >
                    <option value="common">
                      {t('dialogs.accounts.microsoftAuthorityCommon')}
                    </option>
                    <option value="consumers">
                      {t('dialogs.accounts.microsoftAuthorityConsumers')}
                    </option>
                    <option value="organizations">
                      {t('dialogs.accounts.microsoftAuthorityOrganizations')}
                    </option>
                  </select>
                  <span className="form__hint">
                    {t('dialogs.accounts.microsoftAuthorityHint')}
                  </span>
                </label>
                <FocusableNote className="form__hint accounts-google-flow-hint">
                  {t('dialogs.accounts.microsoftFlowHint')}
                </FocusableNote>
              </>
            )}

            {kind === 'ews' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.ewsEndpointLabel')}
                  </span>
                  <input
                    type="url"
                    value={ews.endpoint}
                    onChange={(e) =>
                      setEws((prev) => ({
                        ...prev,
                        endpoint: e.target.value,
                      }))
                    }
                    placeholder={t('dialogs.accounts.ewsEndpointPlaceholder')}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.ewsEndpointHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.usernameLabel')}
                  </span>
                  <input
                    type="text"
                    value={ews.username}
                    onChange={(e) =>
                      setEws((prev) => ({
                        ...prev,
                        username: e.target.value,
                      }))
                    }
                    autoComplete="username"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.ewsUsernameHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.passwordLabel')}
                  </span>
                  <input
                    type="password"
                    value={ews.password}
                    onChange={(e) =>
                      setEws((prev) => ({
                        ...prev,
                        password: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.passwordHint')}
                  </span>
                </label>
                <FocusableNote className="form__hint accounts-google-flow-hint">
                  {t('dialogs.accounts.ewsReadOnlyHint')}
                </FocusableNote>
                {testMessage && kind === 'ews' && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            {kind === 'vikunja' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.vikunjaServerUrlLabel')}
                  </span>
                  <input
                    type="url"
                    value={vikunja.serverUrl}
                    onChange={(e) =>
                      setVikunja((prev) => ({
                        ...prev,
                        serverUrl: e.target.value,
                      }))
                    }
                    placeholder={t(
                      'dialogs.accounts.vikunjaServerUrlPlaceholder',
                    )}
                    autoComplete="off"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.vikunjaServerUrlHint')}
                  </span>
                </label>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.vikunjaApiTokenLabel')}
                  </span>
                  <input
                    type="password"
                    value={vikunja.apiToken}
                    onChange={(e) =>
                      setVikunja((prev) => ({
                        ...prev,
                        apiToken: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.vikunjaApiTokenHint')}
                  </span>
                </label>
                {testMessage && kind === 'vikunja' && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            {kind === 'todoist' && !formSpec && (
              <>
                <label className="form__field">
                  <span className="form__label">
                    {t('dialogs.accounts.todoistApiTokenLabel')}
                  </span>
                  <input
                    type="password"
                    value={todoist.apiToken}
                    onChange={(e) =>
                      setTodoist((prev) => ({
                        ...prev,
                        apiToken: e.target.value,
                      }))
                    }
                    autoComplete="new-password"
                    spellCheck={false}
                    required
                  />
                  <span className="form__hint">
                    {t('dialogs.accounts.todoistApiTokenHint')}
                  </span>
                </label>
                {testMessage && kind === 'todoist' && (
                  <p
                    role="status"
                    aria-live="polite"
                    className="form__hint accounts-test-ok"
                  >
                    {testMessage}
                  </p>
                )}
              </>
            )}

            <div className="form__actions">
              {kind === 'ews' && (
                <button
                  type="button"
                  className="form__action"
                  onClick={onDiscover}
                  aria-disabled={
                    discovering || testing || submitting || undefined
                  }
                  aria-describedby={`${headingId}-discover-help`}
                >
                  {discovering
                    ? t('dialogs.accounts.ewsDiscovering')
                    : t('dialogs.accounts.ewsDiscover')}
                </button>
              )}
              {(kind === 'caldav' ||
                kind === 'ical' ||
                kind === 'ews' ||
                kind === 'vikunja' ||
                kind === 'todoist') && (
                <button
                  type="button"
                  className="form__action"
                  onClick={onTestConnection}
                  aria-disabled={
                    testing || submitting || discovering || undefined
                  }
                >
                  {testing
                    ? t('dialogs.accounts.testing')
                    : t('dialogs.accounts.testConnection')}
                </button>
              )}
              <button
                type="submit"
                className="form__action form__action--primary"
                aria-disabled={
                  submitting ||
                  testing ||
                  discovering ||
                  !availableKinds.some((entry) => entry.kind === kind) ||
                  undefined
                }
              >
                {t('dialogs.accounts.add')}
              </button>
            </div>
            {kind === 'ews' && (
              <p
                id={`${headingId}-discover-help`}
                className="sr-only"
              >
                {t('dialogs.accounts.ewsDiscoverSrHint')}
              </p>
            )}
          </form>
        </section>

      </div>

      <ConfirmDialog
        isOpen={confirmTarget !== null}
        onClose={() => setConfirmTarget(null)}
        onConfirm={() => {
          if (confirmTarget) void performDelete(confirmTarget);
        }}
        title={t('dialogs.accounts.deleteTitle')}
        message={t('dialogs.accounts.deleteMessage', {
          name: confirmTarget?.display_name ?? '',
        })}
      />
      <ContactsPrivacyNoticeModal
        isOpen={privacyNoticeFor !== null}
        adapterKind={privacyNoticeFor}
        onAcknowledge={() => {
          // Fire-and-forget the pref write: if it fails the
          // user just sees the modal one more time on the next
          // connect, which is harmless. Closing the modal
          // optimistically keeps the UI responsive without a
          // spinner for a sub-second write.
          void setUserPref(PREF_PRIVACY_NOTICE_ACK, 'true').catch((err) => {
            // eslint-disable-next-line no-console
            console.warn('privacy notice pref write failed', err);
          });
          setPrivacyNoticeFor(null);
        }}
      />
    </>
  );
}
