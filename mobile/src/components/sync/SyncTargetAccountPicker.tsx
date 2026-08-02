import { useFocusEffect, useNavigation } from '@react-navigation/native';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  AccessibilityInfo,
  findNodeHandle,
  Pressable,
  StyleSheet,
  Text,
  View,
} from 'react-native';

import {
  listAccounts,
  listAdapterKinds,
  type Account,
  type AdapterKindInfo,
} from '../../api/accounts';
import {
  previewSyncAccountHostKey,
  selectSyncAccount,
  trustSftpHostKey,
  type HostKeyPreview,
} from '../../api/sync';
import { useThemedStyles, type ThemeColors } from '../../theme';
import { AppDialog } from '../AppDialog';

/**
 * Settings → Sync, the target half: WHICH of the user's accounts holds the
 * dataset.
 *
 * It used to ask a different question — what kind of target, and what are its
 * host, path and password — through
 * [`SyncTargetConfigForm`](./SyncTargetConfigForm.tsx). That form still exists
 * and the first-launch wizard still renders it, because a fresh install has no
 * accounts yet and has to create its first one while deciding whether to join a
 * dataset or start one. Everywhere else the account is already there: added
 * under Settings → Accounts, or carried in by a restore. So this asks the only
 * question that is left, and it asks it about rows rather than about protocols.
 *
 * The desktop twin is `src/components/sync/SyncTargetAccountPicker.tsx`; same
 * behaviour, different markup — a phone has no master/detail listbox, so the
 * detail pane's contents ride on the row instead.
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
 * - Each account is one addressable row whose accessible name is
 *   "{name}, {kind}, {does it hold the dataset}" — the same sentence the
 *   desktop listbox speaks, so swiping through says which row the dataset is on
 *   without the user having to go and look. Both halves are TRANSLATED phrases
 *   about the DATASET, never a platform state word: `selected`/`checked` stay
 *   English on iOS (see `src/a11y/roles.ts`), and on desktop the same summary
 *   rides beside a listbox `aria-selected` that means something else entirely
 *   ("whose detail pane is showing"), so it must not sound like a selection.
 * - The list says nothing until it has looked: an empty list on the first
 *   render is a loading state, and a FAILED load is its own message — not
 *   "none of your accounts can hold a dataset".
 * - Acting on a row is a real button AND a custom accessibility action, the
 *   pattern the accounts screen already uses, so a rotor user never has to hunt
 *   for the affordance.
 * - Focus after a change lands on the status note at the top, which is the one
 *   node that survives every state change here AND whose text is the new state
 *   — the pressed button is not, because "Sync through X" is replaced by "this
 *   account holds the dataset" the moment it succeeds.
 * - A refusal is ANNOUNCED and focus moves onto it, both imperatively in the
 *   handler and both carrying the same sentence. Not a live region: TalkBack
 *   speaks one of those on its own and would then say the refusal twice, and
 *   an effect keyed on the message never re-runs when the same refusal is set
 *   twice, so a second press against the same dead server left the cursor
 *   standing where it was. Same arrangement as the desktop twin.
 * - The START of a probe is announced. `accessibilityState.busy` has no
 *   VoiceOver equivalent and a changed label is not re-read for the element
 *   that already has focus — and on iOS the row is one accessible element, so
 *   the button inside it is not reachable at all and its label is never
 *   spoken. Without the announcement the whole `test_connection` +
 *   `fetch_meta` round trip passed in silence.
 */

export interface SyncTargetAccountPickerProps {
  /** The account this device syncs through, or `null` for none. */
  currentAccountId: string | null;
  /**
   * Whether this device is actually syncing through it — the host's own
   * `configured`, not the stored pointer. `null` while that is still unknown.
   *
   * The two disagree after a start-up restore that refused: a locked keychain,
   * a credential that is gone or an unconfirmed host key all leave the pointer
   * (and therefore the row) exactly where it was while nothing syncs. Without
   * this the picker stated the pointer as fact — "this device syncs through
   * X", "holds the sync dataset" — on a screen whose status line said the
   * opposite, and offered no control on the one row that needed one.
   */
  active: boolean | null;
  /** Re-read the screen's summary + status after the choice changed. */
  onChanged: () => void | Promise<void>;
}

function errorMessage(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

export function SyncTargetAccountPicker({
  currentAccountId,
  active,
  onChanged,
}: SyncTargetAccountPickerProps) {
  const { t } = useTranslation();
  const navigation = useNavigation();
  const styles = useThemedStyles(makeStyles);

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [syncKinds, setSyncKinds] = useState<AdapterKindInfo[]>([]);
  /** False until the first load has answered — one way or the other. Without
   *  it the empty list on the first render is stated as a FACT ("none of your
   *  accounts can hold a dataset"), which is a lie the user has no way to
   *  distinguish from the truth. */
  const [loaded, setLoaded] = useState(false);
  /** Set when the load itself failed. A failed load leaves the same empty
   *  list; saying "you have no suitable account" for it would be the same lie
   *  made permanent — and it used to be swallowed into `[]`. */
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The account whose host key is being confirmed, with the freshly-probed
   *  fingerprint — the §19.5 gesture, offered for exactly the account that was
   *  refused instead of a message the user has no way to act on. */
  const [trustFor, setTrustFor] = useState<Account | null>(null);
  const [trustPreview, setTrustPreview] = useState<HostKeyPreview | null>(null);

  // The one node that outlives every state change in here, and whose text IS
  // the state. See the component doc.
  const statusRef = useRef<Text>(null);
  const errorRef = useRef<Text>(null);

  const announce = useCallback(
    (message: string) => AccessibilityInfo.announceForAccessibility(message),
    [],
  );

  const focusOn = useCallback((node: Text | null) => {
    const tag = node ? findNodeHandle(node) : null;
    if (tag != null) AccessibilityInfo.setAccessibilityFocus(tag);
  }, []);

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
      setLoadError(errorMessage(err));
    } finally {
      setLoaded(true);
    }
  }, []);

  // Re-read on every focus, not just on mount: the "no accounts yet" route
  // below sends the user to the accounts screen to add one, and coming back is
  // exactly the moment the answer changed.
  useFocusEffect(
    useCallback(() => {
      void reload();
    }, [reload]),
  );

  // And whenever the chosen target moves. Disconnect DELETES the account row it
  // pointed at, and it happens on this screen — which is already focused, so
  // the effect above never fires for it. Without this the list would go on
  // offering a row that is not there any more.
  useEffect(() => {
    void reload();
  }, [currentAccountId, reload]);

  /** Say a refusal once, out loud, and park focus on it so it stays
   *  re-readable. Both imperative, in the handler: setting the SAME refusal
   *  twice makes React bail out of the re-render, so an effect keyed on it
   *  never re-runs and a second press against the same dead server left the
   *  cursor standing where it was. One frame later, so the note has committed
   *  (and any dialog above it has gone) before focus moves. */
  const showError = useCallback(
    (message: string) => {
      setError(message);
      announce(message);
      requestAnimationFrame(() => focusOn(errorRef.current));
    },
    [announce, focusOn],
  );

  /** One group per adapter kind that can hold a dataset, in the host's order,
   *  keeping only the kinds the user actually has an account for.
   *
   *  The group label also rides into every row's accessible name, so the kind
   *  is spoken without a second stop — and it is the plugin's own name
   *  (translated when this build ships a translation for it), never a table in
   *  here. */
  const groups = useMemo(
    () =>
      syncKinds
        .map((kind) => ({
          id: kind.kind,
          label: t(`dialogs.accounts.kindName.${kind.kind}`, {
            defaultValue: kind.name,
          }),
          items: accounts.filter((a) => a.adapter_kind === kind.kind),
        }))
        .filter((group) => group.items.length > 0),
    [accounts, syncKinds, t],
  );

  const currentAccount =
    accounts.find((a) => a.id === currentAccountId) ?? null;
  // Chosen, and demonstrably not working. `active === null` is "not answered
  // yet" and claims nothing — the status arrives on its own round trip, and a
  // note that accused the target for one render would be worse than late.
  const currentBroken = currentAccount != null && active === false;

  const runSelect = useCallback(
    async (account: Account) => {
      setError(null);
      setBusyId(account.id);
      // Say that the probe STARTED — see the component doc. On iOS this is
      // the ONLY signal there is: the row is one accessible element, so the
      // button's changed label is not spoken to anyone.
      announce(
        t('dialogs.settings.sync.targetUseBusy', {
          name: account.display_name,
        }),
      );
      try {
        await selectSyncAccount(account.id);
        await onChanged();
        announce(
          t('dialogs.settings.sync.targetSelected', {
            name: account.display_name,
          }),
        );
        // The pressed button is gone — it said "sync through this" and the row
        // now says "this one holds it". Land on the status note, whose text has
        // just become the answer. One frame later, so the re-render the line
        // above triggered has committed and the note reads out its NEW text.
        requestAnimationFrame(() => focusOn(statusRef.current));
      } catch (err) {
        // No error CODE crosses this boundary — a `StoreError` arrives as a
        // sentence. So the one refusal whose repair is a GESTURE is identified
        // by ASKING: an account whose adapter pins host keys and whose
        // fingerprint is not confirmed on this device is the §19.5 case, and
        // the probe answers `null` without touching the network for every
        // adapter that cannot produce it.
        const preview = await previewSyncAccountHostKey(account.id).catch(
          () => null,
        );
        if (preview != null && preview.status.kind !== 'unchanged') {
          // No error text here on purpose: the dialog IS the message, it takes
          // screen-reader focus onto its own title, and a second assertive
          // announcement fired underneath it would talk over that. The
          // sentence appears if the user declines — see `cancelTrust`.
          setTrustFor(account);
          setTrustPreview(preview);
          return;
        }
        showError(t('mobile.error', { message: errorMessage(err) }));
      } finally {
        setBusyId(null);
      }
    },
    [announce, focusOn, onChanged, showError, t],
  );

  const acceptTrust = useCallback(async () => {
    const preview = trustPreview;
    const account = trustFor;
    setTrustPreview(null);
    setTrustFor(null);
    if (preview == null || account == null) return;
    try {
      await trustSftpHostKey(preview.host_port, preview.fingerprint);
    } catch (err) {
      showError(t('mobile.error', { message: errorMessage(err) }));
      return;
    }
    await runSelect(account);
  }, [runSelect, showError, t, trustFor, trustPreview]);

  const cancelTrust = useCallback(() => {
    setTrustPreview(null);
    setTrustFor(null);
    // Declining the fingerprint means the account still cannot hold the
    // dataset, and the dialog that said so is gone — so say it on the screen,
    // where it stays re-readable and next to the button that retries.
    //
    // ONE sentence, carrying both halves. It used to announce "Cancelled, the
    // pinned host key was left unchanged" and then move focus onto a note
    // reading "this server's fingerprint has not been confirmed" — two
    // different sentences racing, and the focus move reliably wins on iOS, so
    // the user never heard that the cancel took effect.
    showError(
      `${t('dialogs.settings.sync.sftpTrustCancelled')} ${t(
        'dialogs.settings.sync.targetHostKeyUntrusted',
      )}`,
    );
  }, [showError, t]);

  // The accounts screen is a sibling route on the SAME settings stack as this
  // one, so the user lands where they add a target and comes straight back —
  // rather than this screen growing a second copy of the connect form.
  const openAccounts = useCallback(() => {
    navigation.navigate('Accounts');
  }, [navigation]);

  return (
    <>
      {/* Deliberately NOT a live region. Every change to this line is already
          spoken by an explicit announcement (the select below, the screen's
          Disconnect) and the screen's own status line above it is the live
          region this surface has. A second one would announce the same change
          twice, and would announce the empty→loaded transition on entry as if
          the target had just gone away. */}
      <Text ref={statusRef} style={styles.status} accessibilityRole="text">
        {currentBroken && currentAccount != null
          ? t('dialogs.settings.sync.targetStatusBroken', {
              name: currentAccount.display_name,
            })
          : currentAccount != null
            ? t('dialogs.settings.sync.targetStatusCurrent', {
                name: currentAccount.display_name,
              })
            : t('dialogs.settings.sync.targetStatusNone')}
      </Text>
      <Text style={styles.hint} accessibilityRole="text">
        {t('dialogs.settings.sync.targetIntro')}
      </Text>

      {/* Deliberately NOT a live region, exactly like the desktop twin: every
          refusal here is already announced imperatively by `showError`, which
          then lands focus on this node, and TalkBack would otherwise read the
          same sentence a second time. */}
      {error != null && (
        <Text ref={errorRef} style={styles.error} accessibilityRole="text">
          {error}
        </Text>
      )}

      {!loaded ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.accounts.loading')}
        </Text>
      ) : loadError != null ? (
        <Text style={styles.error} accessibilityRole="text">
          {t('dialogs.settings.sync.targetLoadFailed', { message: loadError })}
        </Text>
      ) : groups.length === 0 ? (
        <Text style={styles.hint} accessibilityRole="text">
          {t('dialogs.settings.sync.targetEmpty')}
        </Text>
      ) : (
        <View
          accessibilityRole="list"
          accessibilityLabel={t('dialogs.settings.sync.targetSelectorLabel')}
          style={styles.list}
        >
          {groups.map((group) => (
            <View key={group.id} style={styles.group}>
              <Text style={styles.groupLabel} accessibilityRole="header">
                {group.label}
              </Text>
              {group.items.map((account) => {
                const isCurrent = account.id === currentAccountId;
                const broken = isCurrent && currentBroken;
                const busy = busyId === account.id;
                // Any select in flight blocks every row, not just this one —
                // see the rotor action below.
                const blocked = busyId != null;
                const summary = !isCurrent
                  ? t('dialogs.settings.sync.targetOptionAvailable')
                  : broken
                    ? t('dialogs.settings.sync.targetOptionBroken')
                    : t('dialogs.settings.sync.targetOptionCurrent');
                // The row the dataset is already on carries no action — with
                // one exception: a chosen target the host did not come up on.
                // Pressing runs the same select, which either repairs the
                // state or SAYS what is wrong with it.
                const actionable = !isCurrent || broken;
                const useLabel = broken
                  ? t('dialogs.settings.sync.targetRetry', {
                      name: account.display_name,
                    })
                  : t('dialogs.settings.sync.targetUse', {
                      name: account.display_name,
                    });
                // The name the button carries WHILE the probe runs. It has to
                // change with the visible text: `accessibilityState.busy` has
                // no VoiceOver equivalent, so a static label left a
                // VoiceOver/TalkBack user hearing "Sync through Nextcloud NAS"
                // for the whole network round-trip and then a result out of
                // nowhere.
                const busyLabel = t('dialogs.settings.sync.targetUseBusy', {
                  name: account.display_name,
                });
                return (
                  <View
                    key={account.id}
                    accessible
                    accessibilityRole="text"
                    // While a probe runs, the ROW says so. On iOS it is the
                    // only element a VoiceOver user can reach — the button
                    // inside it is not one — so leaving the row saying "does
                    // not hold the sync dataset" meant swiping back to it
                    // during a live network round trip and being told the
                    // state it is in the middle of leaving.
                    accessibilityLabel={
                      busy
                        ? busyLabel
                        : t('dialogs.settings.sync.targetOptionLabel', {
                            name: account.display_name,
                            kind: group.label,
                            summary,
                          })
                    }
                    // Withdrawn while anything is in flight, so the rotor
                    // does not offer an action that is about to be ignored.
                    accessibilityActions={
                      actionable && !blocked
                        ? [{ name: 'use', label: useLabel }]
                        : undefined
                    }
                    // Gate on ANY select being in flight, not just this row's,
                    // exactly like the `Pressable` below and the desktop
                    // twin. On iOS the row is one accessible element, so the
                    // button inside it is NOT a VoiceOver element and this
                    // rotor action is the only way a VoiceOver user can
                    // activate anything here — a per-row gate left the two
                    // concurrent `select_sync_account` calls that guard
                    // exists to prevent reachable by exactly those users.
                    onAccessibilityAction={(e) => {
                      if (e.nativeEvent.actionName === 'use' && !blocked) {
                        void runSelect(account);
                      }
                    }}
                    style={[styles.row, isCurrent && styles.rowCurrent]}
                  >
                    <View style={styles.rowText}>
                      <Text style={styles.accountName}>
                        {account.display_name}
                      </Text>
                      <Text style={styles.accountKind}>{group.label}</Text>
                      {/* The state marker a sighted user needs, and only on
                          the row that has something to say: every other row
                          already carries "Sync through …" next to it. Visible
                          only — the row's own accessible name ends in this
                          same state, and saying it twice is noise. */}
                      {isCurrent && (
                        <Text
                          style={styles.rowNote}
                          importantForAccessibility="no"
                        >
                          {t(
                            broken
                              ? 'dialogs.settings.sync.targetBrokenNote'
                              : 'dialogs.settings.sync.targetCurrentNote',
                            { name: account.display_name },
                          )}
                        </Text>
                      )}
                    </View>
                    {actionable && (
                      <Pressable
                        accessibilityRole="button"
                        accessibilityState={{ disabled: blocked, busy }}
                        accessibilityLabel={busy ? busyLabel : useLabel}
                        disabled={blocked}
                        onPress={() => void runSelect(account)}
                        style={({ pressed }) => [
                          styles.smallButton,
                          pressed && styles.pressed,
                        ]}
                      >
                        <Text style={styles.smallButtonText}>
                          {busy ? busyLabel : useLabel}
                        </Text>
                      </Pressable>
                    )}
                  </View>
                );
              })}
            </View>
          ))}
        </View>
      )}

      <Pressable
        accessibilityRole="button"
        accessibilityLabel={t('dialogs.settings.sync.targetAddAccount')}
        onPress={openAccounts}
        style={({ pressed }) => [styles.ghostButton, pressed && styles.pressed]}
      >
        <Text style={styles.ghostButtonText}>
          {t('dialogs.settings.sync.targetAddAccount')}
        </Text>
      </Pressable>

      {/* §19.5 — the fingerprint decision, in the app's focus-trapping popup so
          the numbers cannot be confirmed by a stray tap on the screen behind.
          Same pin store as the connect form's trust panel, keyed by host:port,
          so a fingerprint confirmed on either path is confirmed for both. */}
      <AppDialog
        visible={trustPreview != null}
        title={
          trustPreview?.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustChangedTitle')
            : t('dialogs.settings.sync.sftpTrustNewTitle')
        }
        message={
          trustPreview?.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustChangedBody')
            : t('dialogs.settings.sync.sftpTrustNewBody')
        }
        confirmLabel={
          trustPreview?.status.kind === 'changed'
            ? t('dialogs.settings.sync.sftpTrustAcceptChanged')
            : t('dialogs.settings.sync.sftpTrustAcceptNew')
        }
        cancelLabel={t('dialogs.settings.sync.sftpTrustCancel')}
        destructive={trustPreview?.status.kind === 'changed'}
        onConfirm={() => void acceptTrust()}
        onCancel={cancelTrust}
      >
        <Text style={styles.trustField}>
          {t('dialogs.settings.sync.sftpTrustHostLabel')}:{' '}
          {trustPreview?.host_port ?? ''}
        </Text>
        {trustPreview?.status.kind === 'changed' && (
          <Text style={styles.trustField}>
            {t('dialogs.settings.sync.sftpTrustStoredLabel')}:{' '}
            {trustPreview.status.stored}
          </Text>
        )}
        <Text style={styles.trustField}>
          {t('dialogs.settings.sync.sftpTrustPresentedLabel')}:{' '}
          {trustPreview?.fingerprint ?? ''}
        </Text>
        <Text style={styles.hint}>
          {t('dialogs.settings.sync.sftpTrustVerifyHint')}
        </Text>
      </AppDialog>
    </>
  );
}

const makeStyles = (c: ThemeColors) =>
  StyleSheet.create({
    status: { fontSize: 16, color: c.textPrimary, fontWeight: '600' },
    hint: { fontSize: 13, color: c.textSecondary },
    list: { gap: 14 },
    group: { gap: 8 },
    groupLabel: { fontSize: 15, fontWeight: '700', color: c.textLabel },
    // A column, not the accounts screen's row: the action here is named after
    // the account ("Sync through Nextcloud NAS"), and a long name beside the
    // text would squeeze the button into a wrapping sliver on a phone.
    row: {
      gap: 10,
      padding: 16,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
    },
    rowCurrent: { borderColor: c.accent },
    rowText: { gap: 2 },
    accountName: { fontSize: 18, color: c.textPrimary, fontWeight: '600' },
    accountKind: { fontSize: 14, color: c.textSecondary },
    rowNote: { fontSize: 13, color: c.textSecondary },
    smallButton: {
      paddingVertical: 12,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surface,
      alignItems: 'center',
    },
    smallButtonText: { fontSize: 15, fontWeight: '600', color: c.accent },
    ghostButton: {
      paddingVertical: 12,
      paddingHorizontal: 18,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: c.border,
      backgroundColor: c.surfaceAlt,
      alignItems: 'center',
    },
    ghostButtonText: { fontSize: 16, fontWeight: '600', color: c.link },
    trustField: { fontSize: 14, color: c.textPrimary, fontFamily: 'monospace' },
    pressed: { opacity: 0.7 },
    error: { fontSize: 15, fontWeight: '600', color: c.danger },
  });
