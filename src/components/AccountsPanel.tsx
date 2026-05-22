import {
  useCallback,
  useEffect,
  useId,
  useRef,
  useState,
  type FormEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  connectGoogleAccount,
  connectMicrosoftAccount,
  createAccount,
  deleteAccount,
  discoverEwsEndpoint,
  isCommandError,
  listAccounts,
  testCaldavConnection,
  testEwsConnection,
  testIcalFeed,
  testTodoistConnection,
  testVikunjaConnection,
} from '../api/client';
import type { Account, AdapterKind } from '../api/types';
import { useCalendarStore } from '../state/CalendarStore';
import { ConfirmDialog } from './ConfirmDialog';

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

/** Order of the kind picker. Local first because it's the only
 *  enabled choice in Phase 6a; the others follow the rough release
 *  order announced in the spec. */
const KIND_ORDER: AdapterKind[] = [
  'local',
  'caldav',
  'ical',
  'google',
  'microsoft_graph',
  'ews',
  'vikunja',
  'todoist',
];

const ENABLED_KINDS: ReadonlySet<AdapterKind> = new Set([
  'local',
  'caldav',
  'ical',
  'google',
  'microsoft_graph',
  'ews',
  'vikunja',
  'todoist',
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
  const { t } = useTranslation();
  const announce = useAnnouncer();
  // The store owns the calendar / task-list catalog the sidebar
  // renders from. We have to nudge it manually after creating or
  // deleting an account — otherwise the new account's calendars
  // wouldn't show up until something else triggers a store refresh.
  const { refreshCalendars, refreshTaskLists, refreshAccounts } =
    useCalendarStore();

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [confirmTarget, setConfirmTarget] = useState<Account | null>(null);

  const [kind, setKind] = useState<AdapterKind>('local');
  const [displayName, setDisplayName] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [testing, setTesting] = useState(false);
  const [discovering, setDiscovering] = useState(false);
  const [testMessage, setTestMessage] = useState<string | null>(null);
  const [caldav, setCaldav] = useState<CaldavFields>(EMPTY_CALDAV);
  const [ical, setIcal] = useState<IcalFields>(EMPTY_ICAL);
  const [google, setGoogle] = useState<GoogleFields>(EMPTY_GOOGLE);
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
  }, []);

  // The panel mounts when the user lands on its tab, so "on mount" is
  // the right moment to fetch — no need to gate on an `isOpen` flag any
  // more (the host SettingsDialog handles open/close).
  useEffect(() => {
    refresh();
  }, [refresh]);

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
      if (!ENABLED_KINDS.has(kind)) {
        // Defence in depth — UI should already block disabled kinds.
        setError(t('dialogs.accounts.kindUnavailable'));
        return;
      }
      if (kind === 'caldav') {
        const v = validateCaldav();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'ical') {
        const v = validateIcal();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'google') {
        const v = validateGoogle();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'microsoft_graph') {
        const v = validateMicrosoft();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'ews') {
        const v = validateEws();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'vikunja') {
        const v = validateVikunja();
        if (v) {
          setError(v);
          return;
        }
      }
      if (kind === 'todoist') {
        const v = validateTodoist();
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
          created = await connectGoogleAccount(
            google.clientId.trim(),
            google.clientSecret.trim(),
            name,
          );
        } else if (kind === 'microsoft_graph') {
          // Same OAuth-then-persist flow as Google, minus the
          // client_secret (Microsoft honours PKCE for public clients).
          created = await connectMicrosoftAccount(
            microsoft.clientId.trim(),
            name,
            microsoft.authority.trim() || undefined,
          );
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
        setDisplayName('');
        setCaldav(EMPTY_CALDAV);
        setIcal(EMPTY_ICAL);
        setGoogle(EMPTY_GOOGLE);
        setMicrosoft(EMPTY_MICROSOFT);
        setEws(EMPTY_EWS);
        setVikunja(EMPTY_VIKUNJA);
        setTodoist(EMPTY_TODOIST);
        refresh();
        // Re-fetch the calendar / task-list catalog so the sidebar
        // picks up the new account's containers without the user
        // having to open another dialog (or restart the app). Errors
        // here are non-fatal — the account row was already
        // persisted; a stale sidebar fixes itself on the next normal
        // store refresh.
        void Promise.allSettled([
          refreshAccounts(),
          refreshCalendars(),
          refreshTaskLists(),
        ]);
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      } finally {
        setSubmitting(false);
      }
    },
    [
      displayName,
      kind,
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
      t,
    ],
  );

  const onTestConnection = useCallback(async () => {
    setTestMessage(null);
    setError(null);
    setTesting(true);
    try {
      if (kind === 'caldav') {
        const v = validateCaldav();
        if (v) {
          setError(v);
          return;
        }
        await testCaldavConnection(
          caldav.serverUrl.trim(),
          caldav.username.trim(),
          caldav.password,
        );
      } else if (kind === 'ical') {
        const v = validateIcal();
        if (v) {
          setError(v);
          return;
        }
        await testIcalFeed(
          ical.feedUrl.trim(),
          ical.username.trim() || null,
          ical.password || null,
        );
      } else if (kind === 'ews') {
        const v = validateEws();
        if (v) {
          setError(v);
          return;
        }
        await testEwsConnection(
          ews.endpoint.trim(),
          ews.username.trim(),
          ews.password,
        );
      } else if (kind === 'vikunja') {
        const v = validateVikunja();
        if (v) {
          setError(v);
          return;
        }
        await testVikunjaConnection(
          vikunja.serverUrl.trim(),
          vikunja.apiToken.trim(),
        );
      } else if (kind === 'todoist') {
        const v = validateTodoist();
        if (v) {
          setError(v);
          return;
        }
        await testTodoistConnection(todoist.apiToken.trim());
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
    if (!ews.username.trim()) {
      setError(t('dialogs.accounts.ewsDiscoverNeedsEmail'));
      return;
    }
    if (!ews.password) {
      setError(t('dialogs.accounts.ewsDiscoverNeedsPassword'));
      return;
    }
    setDiscovering(true);
    try {
      const result = await discoverEwsEndpoint(
        ews.username.trim(),
        ews.password,
      );
      setEws((prev) => ({
        ...prev,
        endpoint: result.ews_url,
        // If autodiscover took us through a RedirectAddr step, the
        // canonical login is the one we actually authenticated as —
        // surface that to the user so the password they typed
        // matches the username we stored.
        username: result.account_email,
      }));
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
  }, [ews, announce, t]);

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
        // account's calendars disappear from the sidebar.
        void Promise.allSettled([
          refreshAccounts(),
          refreshCalendars(),
          refreshTaskLists(),
        ]);
      } catch (err) {
        if (isCommandError(err)) setError(`${err.code}: ${err.message}`);
        else setError(String(err));
      }
    },
    [announce, refresh, refreshCalendars, refreshTaskLists, t],
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
  // Track whether the listbox currently owns DOM focus. Used to gate
  // `aria-activedescendant` so it only points at an option while the
  // listbox is actually focused. With the attribute set unconditionally
  // some screen readers (NVDA in particular) treat the listbox as the
  // current focus target the moment it enters the accessibility tree
  // and start reading the topmost option, even though keyboard focus
  // is still on the Settings tab. The gating turns that into a no-op.
  const [listHasFocus, setListHasFocus] = useState(false);

  useEffect(() => {
    if (focusIndex >= accounts.length) {
      setFocusIndex(Math.max(0, accounts.length - 1));
    }
  }, [accounts.length, focusIndex]);

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

  const handleListKey = (e: React.KeyboardEvent<HTMLUListElement>) => {
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
                    aria-label={t(
                      isLocal
                        ? 'dialogs.accounts.rowLabelLocal'
                        : 'dialogs.accounts.rowLabel',
                      {
                        name: acc.display_name,
                        kind: t(`dialogs.accounts.kindName.${acc.adapter_kind}`),
                      },
                    )}
                    className={
                      'accounts-list__item' +
                      (focused ? ' accounts-list__item--focused' : '')
                    }
                    onClick={() => {
                      setFocusIndex(i);
                      if (!isLocal) setConfirmTarget(acc);
                    }}
                  >
                    <span className="accounts-list__name">
                      {acc.display_name}
                    </span>
                    <span className="accounts-list__kind">
                      {t(`dialogs.accounts.kindName.${acc.adapter_kind}`)}
                    </span>
                  </li>
                );
              })}
            </ul>
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
                {KIND_ORDER.map((k) => (
                  <option
                    key={k}
                    value={k}
                    disabled={!ENABLED_KINDS.has(k)}
                  >
                    {t(`dialogs.accounts.kindName.${k}`)}
                    {!ENABLED_KINDS.has(k)
                      ? ` — ${t('dialogs.accounts.comingSoon')}`
                      : ''}
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

            {kind === 'caldav' && (
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

            {kind === 'ical' && (
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

            {kind === 'google' && (
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
                <p className="form__hint accounts-google-flow-hint">
                  {t('dialogs.accounts.googleFlowHint')}
                </p>
              </>
            )}

            {kind === 'microsoft_graph' && (
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
                <p className="form__hint accounts-google-flow-hint">
                  {t('dialogs.accounts.microsoftFlowHint')}
                </p>
              </>
            )}

            {kind === 'ews' && (
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
                <p className="form__hint accounts-google-flow-hint">
                  {t('dialogs.accounts.ewsReadOnlyHint')}
                </p>
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

            {kind === 'vikunja' && (
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

            {kind === 'todoist' && (
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
                  !ENABLED_KINDS.has(kind) ||
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
    </>
  );
}
