import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../../a11y/announcerContext';
import { FocusableNote } from '../../a11y/FocusableNote';
import {
  isCommandError,
  listAccounts,
  listAdapterKinds,
  forgetSftpHostKey,
  previewSyncAccountHostKey,
  selectSyncAccount,
  syncAccountHostKeyPin,
  trustSftpHostKey,
  type AdapterKindInfo,
  type HostKeyPinInfo,
  type HostKeyPreview,
} from '../../api/client';
import type { Account } from '../../api/types';
import { useDialogState } from '../../state/dialogStateContext';
import { useSettingsNav } from '../../state/settingsNavContext';
import {
  SettingsSelectorDetail,
  type SettingsSelectorGroup,
} from '../SettingsSelectorDetail';
import { ConfirmDialog } from '../ConfirmDialog';
import { SyncSftpTrustDialog } from '../SyncSftpTrustDialog';
import { useSyncErrorMessage } from './syncErrorMessage';

/**
 * Settings → Synchronisation, the target half: WHICH of the user's accounts
 * holds the dataset.
 *
 * It used to ask a different question — what kind of target, and what are its
 * host, path and password — through a form with a block per backend. The
 * first-launch wizard still asks that, through
 * [`SyncTargetSchemaForm`](./SyncTargetSchemaForm.tsx) and the plugin's own
 * schema, because a fresh instance has no accounts yet and has to create its
 * first one while deciding whether to join a dataset or start one. Everywhere
 * else the account is already there: added under Settings → Accounts, or
 * carried in by a restore. So this asks the only question that is left, and it
 * asks it about rows rather than about protocols.
 *
 * ## The pinned fingerprint
 *
 * An account whose adapter pins host keys (§19.5) carries a decision this
 * device made about a server, and a decision that can be made must be
 * revocable. The detail pane therefore states the confirmed fingerprint and
 * offers to drop it — read WITHOUT touching the network
 * ([`syncAccountHostKeyPin`]), because the moment a user most wants to withdraw
 * trust from a server is the moment that server is behaving strangely.
 *
 * ## Where the list comes from
 *
 * `listAdapterKinds()` → the kinds whose plugin declares the `sync` capability
 * (`can_sync`), intersected with the accounts that exist. Not a list of names
 * here: the host computes `can_sync` off the manifest, so a plugin that starts
 * being able to hold a dataset appears in this list by shipping, and one that
 * is disabled drops out of it.
 *
 * ## Accessibility
 *
 * - The list is the shared master/detail listbox
 *   ([`SettingsSelectorDetail`](../SettingsSelectorDetail.tsx)) — one tab stop,
 *   arrow keys, selection follows focus. Each option's accessible name is
 *   "{name}, {kind}, {does it hold the dataset}", so arrowing says which row
 *   the dataset is on without the user having to go and look. That phrasing is
 *   deliberate: the listbox's own `aria-selected` means "whose detail pane is
 *   showing", and it is spoken on every row the user arrows to. A summary that
 *   said "current sync target" / "not currently used as the target" collided
 *   with it head-on — "not currently used as the target, selected". Stating
 *   what the account does with the DATASET leaves "selected" to mean the one
 *   thing the listbox can make it mean.
 * - The detail pane opens on the account that holds the dataset
 *   (`preferredItemId`), not on the first row, so the pane and the status note
 *   above it never describe two different accounts.
 * - Every line of prose is a [`FocusableNote`]: this panel lives inside the
 *   Settings Modal, whose body is `role="application"`, where NVDA's focus-mode
 *   traversal skips static text entirely. That includes the refusal — it is a
 *   focus stop, not a `role="alert"`, because the announcement it needs comes
 *   from the shared announcer (below) and a second live region would say the
 *   same sentence twice.
 * - Focus after a change lands on the status note at the top, which is the one
 *   node that survives every state change here AND whose text is the new state
 *   — the pressed button is not, because "Sync through X" is replaced by "this
 *   account holds the dataset" the moment it succeeds. Same idea as the
 *   panel's Disconnect, which lands on the section heading; the note is
 *   strictly better because it names the outcome rather than the section.
 * - Failures announce through the shared announcer AND move focus onto the
 *   message, so a screen-reader user hears WHY without hunting for it, and can
 *   re-read it with arrow keys. Both are imperative, in the handler: setting
 *   the SAME refusal string twice makes React bail out of the re-render, so an
 *   effect keyed on it would never re-run and a second press against the same
 *   dead server would be answered with silence.
 * - The START of a probe is announced too. `aria-busy` and a changed button
 *   label are not read out for the element that already has focus — on any of
 *   NVDA, VoiceOver or TalkBack — so pressing "Sync through X" was answered
 *   with silence for the whole of a live `test_connection` + `fetch_meta`
 *   round trip, and then a result out of nowhere.
 */

export interface SyncTargetAccountPickerProps {
  /** The account this device syncs through, or `null` for none. */
  currentAccountId: string | null;
  /**
   * Whether this device is actually syncing through it — the engine's own
   * `configured`, not the stored pointer. `null` while that is still unknown.
   *
   * The two disagree after a start-up restore that refused: a locked keychain,
   * a credential that is gone, an unconfirmed host key or a missing plugin all
   * leave the pointer (and therefore the row) exactly where it was while
   * nothing syncs. Without this the picker stated the pointer as fact — "this
   * device syncs through X", "holds the sync dataset" — two headings under a
   * State section reading "No sync adapter configured yet", and offered no
   * control at all on the one row that needed one.
   */
  active: boolean | null;
  /** Re-read the panel's summary + status after the choice changed. */
  onChanged: () => void | Promise<void>;
}

/** Module scope so the memos inside the shared selector stay stable. */
const accountId = (account: Account): string => account.id;
const accountName = (account: Account): string => account.display_name;

export function SyncTargetAccountPicker({
  currentAccountId,
  active,
  onChanged,
}: SyncTargetAccountPickerProps) {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const messageForError = useSyncErrorMessage();
  const { openAccounts, dataVersion } = useDialogState();
  const settingsNav = useSettingsNav();

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [syncKinds, setSyncKinds] = useState<AdapterKindInfo[]>([]);
  /** False until the first load has answered — one way or the other. Without
   *  it the empty list on the first render is stated as a FACT ("none of your
   *  accounts can hold a dataset"), which is a lie the user has no way to
   *  distinguish from the truth. */
  const [loaded, setLoaded] = useState(false);
  /** Set when the load itself failed. A failed load leaves the same empty
   *  list; saying "you have no suitable account" for it would be the same lie
   *  made permanent. */
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Set when the refusal was `encryption_key_mismatch`, so the passphrase
   *  field appears on that account — the only way out of that state. */
  const [keyMismatchId, setKeyMismatchId] = useState<string | null>(null);
  const [passphraseDraft, setPassphraseDraft] = useState('');
  /** Set when the refusal was `host_key_not_trusted`, so the §19.5 trust
   *  gesture can be offered for exactly that account instead of a message the
   *  user has no way to act on. */
  const [untrustedId, setUntrustedId] = useState<string | null>(null);
  const [trustPreview, setTrustPreview] = useState<HostKeyPreview | null>(null);
  /** The selected account's host-key pin, or `null` when its adapter pins none
   *  — read for the SELECTED account only, so switching rows costs one cheap
   *  local read rather than one per account on every load. */
  const [pin, setPin] = useState<HostKeyPinInfo | null>(null);
  const [pinAccountId, setPinAccountId] = useState<string | null>(null);
  const [confirmForget, setConfirmForget] = useState(false);

  // The one node that outlives every state change in here, and whose text IS
  // the state. See the component doc.
  const statusNoteRef = useRef<HTMLParagraphElement>(null);
  const errorRef = useRef<HTMLParagraphElement>(null);

  const reload = useCallback(async () => {
    try {
      const [accs, kinds] = await Promise.all([
        listAccounts(),
        listAdapterKinds(),
      ]);
      setAccounts(accs);
      setSyncKinds(kinds.filter((k) => k.can_sync));
      setLoadError(null);
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('loading the sync-target accounts failed', err);
      // Through the shared mapping, like every other error path here: a Tauri
      // command rejects with a plain `{code, message}` object and NOT with an
      // `Error`, so the obvious ternary renders "[object Object]".
      setLoadError(messageForError(err));
    } finally {
      setLoaded(true);
    }
  }, [messageForError]);

  // Re-read on mount, on `dataVersion` — adding an account (the "no accounts
  // yet" route below sends the user off to do exactly that) and enabling or
  // disabling a plugin both change the answer, and both bump it — and whenever
  // the chosen target MOVES. That last one is not redundant: Disconnect
  // DELETES the account row it pointed at, and it happens on this panel, which
  // bumps neither `dataVersion` nor anything else this component watches.
  // Without it the list would go on offering a row that is not there any more,
  // with a live "Sync through …" button that resolves to `not_found`. The
  // mobile twin reloads on the same signal, for the same reason.
  useEffect(() => {
    void reload();
  }, [currentAccountId, dataVersion, reload]);

  /** After a disconnect, land on the sentence that says so.
   *
   *  The panel used to announce "Sync target disconnected" and, a frame later,
   *  move focus to its own heading — which reads "Sync target". Two channels
   *  carrying two different sentences, the second interrupting the first, and
   *  the user left on a landmark that says nothing about what just happened.
   *
   *  This note's text has by then become "This device syncs through no
   *  account", which is the outcome. So the component that owns the node owns
   *  the focus, and the panel does neither. */
  const hadAccount = useRef(currentAccountId !== null);
  useEffect(() => {
    const has = currentAccountId !== null;
    // Only the transition, and only downwards: opening the panel with no
    // account chosen is not an event, and must not steal focus from wherever
    // the user actually is.
    if (hadAccount.current && !has) {
      requestAnimationFrame(() => {
        statusNoteRef.current?.focus({ preventScroll: true });
      });
    }
    hadAccount.current = has;
  }, [currentAccountId]);

  /** Park focus on the refusal, which both speaks it and leaves it
   *  re-readable.
   *
   *  Deliberately NOT announced as well. The node focus lands on carries the
   *  message as its accessible name, so announcing the same string first meant
   *  hearing every refusal twice — once from the live region, once from the
   *  element. One channel, and the one that leaves the user somewhere useful.
   *
   *  Imperative rather than an effect keyed on `error`: an identical message
   *  twice in a row is a no-op re-render, so an effect would not fire and the
   *  second press would be answered with silence. */
  const showError = useCallback((message: string) => {
    setError(message);
    requestAnimationFrame(() => {
      errorRef.current?.focus({ preventScroll: true });
    });
  }, []);

  /** One group per adapter kind that can hold a dataset, in the host's order,
   *  keeping only the kinds the user actually has an account for.
   *
   *  The group label rides into every option's accessible name through
   *  `optionLabel`, so the kind is spoken without a second column — and it is
   *  the plugin's own name (translated when this build ships a translation for
   *  it), never a table in here. */
  const groups: SettingsSelectorGroup<Account>[] = useMemo(
    () =>
      syncKinds
        .map((kind) => ({
          id: kind.kind,
          label: t(`dialogs.accounts.kindName.${kind.kind}`, {
            defaultValue: kind.name,
          }),
          items: accounts.filter(
            (account) => account.adapter_kind === kind.kind,
          ),
        }))
        .filter((group) => group.items.length > 0),
    [accounts, syncKinds, t],
  );

  const runSelect = useCallback(
    async (account: Account, passphrase?: string) => {
      setError(null);
      setUntrustedId(null);
      setKeyMismatchId(null);
      setBusyId(account.id);
      // Say that the probe STARTED. The button's name changes to the same
      // sentence, but a screen reader does not re-read the element that
      // already has focus, so without this the press was answered with
      // silence until the round trip came back. Polite: it is progress, and
      // an assertive one would cut off whatever the press itself spoke.
      announce(
        t('dialogs.settings.sync.targetUseBusy', {
          name: account.display_name,
        }),
        'polite',
      );
      try {
        await selectSyncAccount(account.id, passphrase);
        await onChanged();
        announce(
          t('dialogs.settings.sync.targetSelected', {
            name: account.display_name,
          }),
        );
        // The pressed button is gone — it said "sync through this" and the
        // detail now says "this one holds it". Land on the status note, whose
        // text has just become the answer.
        requestAnimationFrame(() => {
          statusNoteRef.current?.focus({ preventScroll: true });
        });
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('select_sync_account failed', err);
        if (isCommandError(err) && err.code === 'host_key_not_trusted') {
          // Not a dead end: the account connect flow never probes, so this is
          // the ordinary state of an SFTP account, and the §19.5 gesture is
          // offered right here.
          setUntrustedId(account.id);
          showError(t('dialogs.settings.sync.targetHostKeyUntrusted'));
          return;
        }
        if (
          isCommandError(err) &&
          (err.code === 'encryption_key_mismatch' ||
            // A passphrase was offered and did not open the dataset either.
            // Same gesture, so the field stays rather than disappearing under
            // the user with only a sentence left behind.
            (err.code === 'decryption_failed' && passphrase))
        ) {
          setKeyMismatchId(account.id);
          setPassphraseDraft('');
          showError(messageForError(err));
          return;
        }
        if (isCommandError(err) && err.code === 'plugin_missing') {
          showError(t('dialogs.settings.sync.targetPluginMissing'));
          return;
        }
        showError(
          `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        );
      } finally {
        setBusyId(null);
      }
    },
    [announce, messageForError, onChanged, showError, t],
  );

  /** Probe the server this account points at and hand the fingerprint to the
   *  existing trust dialog. Which fields hold the host and the port is the
   *  adapter's own declaration — the command resolves them, not this file. */
  const onCheckHostKey = useCallback(
    async (account: Account) => {
      setError(null);
      setBusyId(account.id);
      // Same live round trip, same silence without this — see `runSelect`.
      announce(
        t('dialogs.settings.sync.targetUseBusy', {
          name: account.display_name,
        }),
        'polite',
      );
      try {
        const preview = await previewSyncAccountHostKey(account.id);
        if (preview.status.kind === 'unchanged') {
          // Already pinned; whatever refused was not this. Retry and let the
          // real reason surface.
          await runSelect(account);
          return;
        }
        setTrustPreview(preview);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('preview_sync_account_host_key failed', err);
        showError(
          `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        );
      } finally {
        setBusyId(null);
      }
    },
    [announce, messageForError, runSelect, showError, t],
  );

  /** Re-read the selected account's pin. No network: it is this device's own
   *  decision, and a screen that could only show it while the server answers
   *  would hide it exactly when it matters. */
  const reloadPin = useCallback(async () => {
    const id = pinAccountId;
    if (!id) {
      setPin(null);
      return;
    }
    try {
      const info = await syncAccountHostKeyPin(id);
      // Ignore an answer for an account the user has already left.
      setPin((prev) => (pinAccountId === id ? info : prev));
    } catch {
      // A pin that cannot be read is not a failure of this screen: the account
      // still works, and the section simply does not appear. Reporting it here
      // would put an error over a picker whose real job succeeded.
      setPin(null);
    }
  }, [pinAccountId]);

  useEffect(() => {
    void reloadPin();
  }, [reloadPin]);

  const onTrustAccept = useCallback(
    async (fingerprint: string) => {
      const preview = trustPreview;
      const account = accounts.find((a) => a.id === untrustedId);
      setTrustPreview(null);
      if (!preview || !account) return;
      try {
        await trustSftpHostKey(preview.host_port, fingerprint);
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('trust_sftp_host_key failed', err);
        showError(
          `${t('dialogs.settings.sync.errorPrefix')}: ${messageForError(err)}`,
        );
        return;
      }
      // The pin the user just confirmed is the one this pane offers to drop —
      // read it again before the select, so the section appears whether or not
      // the select that follows succeeds.
      await reloadPin();
      await runSelect(account);
    },
    [
      accounts,
      messageForError,
      reloadPin,
      runSelect,
      showError,
      t,
      trustPreview,
      untrustedId,
    ],
  );

  // Declining the fingerprint means the account still cannot hold the dataset.
  // `onCheckHostKey` cleared the refusal on its way into the dialog, and the
  // dialog that carried the story is now gone — so put the sentence back,
  // where it stays re-readable next to the button that retries.
  //
  // ONE sentence, carrying both halves, spoken and focused. It used to
  // announce "Cancelled, the pinned host key was left unchanged" and one frame
  // later move focus onto a note reading "this server's fingerprint has not
  // been confirmed" — two different sentences racing each other, and on iOS
  // the focus move reliably wins, so the user never heard that the cancel took
  // effect. `showError` says and focuses the same text, in that order.
  const onTrustCancel = useCallback(() => {
    setTrustPreview(null);
    showError(
      `${t('dialogs.settings.sync.sftpTrustCancelled')} ${t(
        'dialogs.settings.sync.targetHostKeyUntrusted',
      )}`,
    );
  }, [showError, t]);

  // A refusal belongs to the account it was raised for, and its repair button
  // lives on that account's detail pane. Arrowing to another option takes the
  // button away; the message has to go with it — and so does the fingerprint
  // button itself, which is rendered off `untrustedId` and outlived the
  // sentence that explained why it was there.
  const onSelectionChange = useCallback((id: string | null) => {
    setError(null);
    setUntrustedId(null);
    setPin(null);
    setPinAccountId(id);
  }, []);

  /** Drop the pin, then say so and land on the sentence that says it.
   *
   *  The button that was pressed is gone — the whole section it lived in is —
   *  so focus goes to the status note, the one node that outlives this. */
  const runForget = useCallback(async () => {
    const hostPort = pin?.host_port;
    setConfirmForget(false);
    if (!hostPort) return;
    try {
      await forgetSftpHostKey(hostPort);
      await reloadPin();
      announce(t('dialogs.settings.sync.sftpForgetPinDone'), 'polite');
      requestAnimationFrame(() => {
        statusNoteRef.current?.focus({ preventScroll: true });
      });
    } catch (err) {
      showError(messageForError(err));
    }
  }, [announce, messageForError, pin, reloadPin, showError, t]);

  // Sync → Accounts WITHOUT a second dialog frame. `openSettings('accounts')`
  // reconciles a new Settings frame into the same tree position, unmounting
  // the pressed button while the dialog stays open and no focus handler
  // re-runs — focus lands on <body>, outside the modal's role="application".
  // See `settingsNavContext.ts`. The fallback is for a host that renders this
  // outside the Settings dialog; there, pushing a frame IS the right move.
  const goToAccounts = useCallback(() => {
    if (settingsNav) settingsNav.goToTab('accounts');
    else openAccounts();
  }, [openAccounts, settingsNav]);

  const currentAccount = accounts.find((a) => a.id === currentAccountId) ?? null;
  // Chosen, and demonstrably not working. `active === null` is "not answered
  // yet" and claims nothing — the status arrives on its own round trip, and a
  // note that accused the target for one render would be worse than late.
  const currentBroken = currentAccount !== null && active === false;

  return (
    <div className="sync-panel__target">
      <FocusableNote ref={statusNoteRef} className="sync-panel__hint">
        {currentBroken && currentAccount
          ? t('dialogs.settings.sync.targetStatusBroken', {
              name: currentAccount.display_name,
            })
          : currentAccount
            ? t('dialogs.settings.sync.targetStatusCurrent', {
                name: currentAccount.display_name,
              })
            : t('dialogs.settings.sync.targetStatusNone')}
      </FocusableNote>
      <FocusableNote className="sync-panel__hint">
        {t('dialogs.settings.sync.targetIntro')}
      </FocusableNote>

      {error && (
        <FocusableNote
          ref={errorRef}
          className="sync-panel__error form__error"
        >
          {error}
        </FocusableNote>
      )}

      {!loaded ? (
        <FocusableNote className="sync-panel__hint">
          {t('dialogs.accounts.loading')}
        </FocusableNote>
      ) : loadError ? (
        <FocusableNote className="sync-panel__error form__error">
          {t('dialogs.settings.sync.targetLoadFailed', { message: loadError })}
        </FocusableNote>
      ) : groups.length === 0 ? (
        <FocusableNote className="sync-panel__hint">
          {t('dialogs.settings.sync.targetEmpty')}
        </FocusableNote>
      ) : (
        <SettingsSelectorDetail<Account>
          groups={groups}
          getItemId={accountId}
          getItemName={accountName}
          getItemSummary={(account) =>
            account.id !== currentAccountId
              ? t('dialogs.settings.sync.targetOptionAvailable')
              : currentBroken
                ? t('dialogs.settings.sync.targetOptionBroken')
                : t('dialogs.settings.sync.targetOptionCurrent')
          }
          selectorLabel={t('dialogs.settings.sync.targetSelectorLabel')}
          optionLabel={({ account: kind, name, summary }) =>
            t('dialogs.settings.sync.targetOptionLabel', {
              name,
              kind,
              summary,
            })
          }
          detailHeading={({ account: kind, name }) =>
            t('dialogs.settings.sync.targetDetailHeading', { name, kind })
          }
          // The panel's own "Sync-Ziel" is an <h3>; a second <h3> in the same
          // section reads as its sibling rather than as the selected row's
          // editor.
          detailHeadingLevel={4}
          // Open on the account that holds the dataset, not on whichever row
          // happens to sort first — otherwise the status note says "syncs
          // through Nextcloud NAS" and the pane under it says "Dropbox Backup
          // does not hold the dataset".
          preferredItemId={currentAccountId}
          onSelectionChange={onSelectionChange}
          renderDetail={(account) => {
            const isCurrent = account.id === currentAccountId;
            const broken = isCurrent && currentBroken;
            const busy = busyId === account.id;
            // Gate on ANY select being in flight, not just this row's.
            // Selection follows focus, so arrowing to another account during a
            // slow probe and pressing ITS button starts a second
            // `select_sync_account`; the two race the orchestrator against the
            // persisted pointer.
            const blocked = busyId !== null;
            return (
              <>
                <FocusableNote className="sync-panel__hint">
                  {broken
                    ? t('dialogs.settings.sync.targetBrokenNote', {
                        name: account.display_name,
                      })
                    : isCurrent
                      ? t('dialogs.settings.sync.targetCurrentNote', {
                          name: account.display_name,
                        })
                      : t('dialogs.settings.sync.targetOtherNote', {
                          name: account.display_name,
                        })}
                </FocusableNote>
                {/* The row the dataset is already on carries no button — with
                    one exception: a chosen target the engine did not come up
                    on. That state used to have no control anywhere (this
                    button was hidden, the fingerprint one needs a refusal
                    that only a press produces, and the panel's Disconnect was
                    gated on the engine being configured), so the one account
                    that needed acting on was the one the user could not act
                    on. Pressing runs the same select, which either repairs
                    the state or SAYS what is wrong with it. */}
                {(!isCurrent || broken) && (
                  <div className="sync-panel__actions">
                    <button
                      type="button"
                      // aria-disabled, not native `disabled`: a button that
                      // disables itself mid-round-trip loses focus to
                      // <body>, which drops NVDA out of the dialog for the
                      // whole probe. Same reasoning as the connect form's
                      // primary action.
                      aria-disabled={blocked}
                      aria-busy={busy}
                      onClick={() => {
                        if (!blocked) void runSelect(account);
                      }}
                    >
                      {busy
                        ? t('dialogs.settings.sync.targetUseBusy', {
                            name: account.display_name,
                          })
                        : broken
                          ? t('dialogs.settings.sync.targetRetry', {
                              name: account.display_name,
                            })
                          : t('dialogs.settings.sync.targetUse', {
                              name: account.display_name,
                            })}
                    </button>
                  </div>
                )}
                {keyMismatchId === account.id && (
                  <div className="sync-panel__field">
                    <label>
                      {t('dialogs.settings.sync.targetKeyPassphraseLabel')}
                      <input
                        type="password"
                        value={passphraseDraft}
                        onChange={(e) => setPassphraseDraft(e.target.value)}
                        autoComplete="current-password"
                      />
                    </label>
                    <div className="sync-panel__actions">
                      <button
                        type="button"
                        aria-disabled={blocked || !passphraseDraft.trim()}
                        aria-busy={busy}
                        onClick={() => {
                          if (blocked || !passphraseDraft.trim()) return;
                          void runSelect(account, passphraseDraft.trim());
                        }}
                      >
                        {t('dialogs.settings.sync.targetKeyPassphraseUse', {
                          name: account.display_name,
                        })}
                      </button>
                    </div>
                  </div>
                )}
                {/* §19.5 — what this device confirmed about this account's
                    server, and the way back out of it. Only on the account
                    whose pin was read, and only once there IS one: an adapter
                    that pins nothing has nothing to say here. */}
                {pinAccountId === account.id && pin?.fingerprint && (
                  <div className="sync-panel__field">
                    <FocusableNote className="sync-panel__hint">
                      {t('dialogs.settings.sync.sftpPinCurrentWithValue', {
                        fingerprint: pin.fingerprint,
                      })}
                    </FocusableNote>
                    <FocusableNote className="sync-panel__hint">
                      {t('dialogs.settings.sync.sftpPinHint')}
                    </FocusableNote>
                    <div className="sync-panel__actions">
                      <button
                        type="button"
                        onClick={() => setConfirmForget(true)}
                      >
                        {t('dialogs.settings.sync.sftpForgetPin')}
                      </button>
                    </div>
                  </div>
                )}
                {untrustedId === account.id && (
                  <div className="sync-panel__actions">
                    <button
                      type="button"
                      aria-disabled={blocked}
                      aria-busy={busy}
                      onClick={() => {
                        if (!blocked) void onCheckHostKey(account);
                      }}
                    >
                      {t('dialogs.settings.sync.targetHostKeyCheck', {
                        name: account.display_name,
                      })}
                    </button>
                  </div>
                )}
              </>
            );
          }}
        />
      )}

      <div className="sync-panel__actions">
        <button type="button" onClick={goToAccounts}>
          {t('dialogs.settings.sync.targetAddAccount')}
        </button>
      </div>

      <SyncSftpTrustDialog
        isOpen={trustPreview !== null}
        preview={trustPreview}
        onAccept={(fp) => void onTrustAccept(fp)}
        onCancel={onTrustCancel}
      />

      <ConfirmDialog
        isOpen={confirmForget}
        onClose={() => setConfirmForget(false)}
        onConfirm={() => void runForget()}
        title={t('dialogs.settings.sync.sftpForgetPin')}
        message={t('dialogs.settings.sync.sftpForgetPinConfirm', {
          hostPort: pin?.host_port ?? '',
        })}
        confirmLabel={t('dialogs.settings.sync.sftpForgetPin')}
      />
    </div>
  );
}
