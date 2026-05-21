import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { AccountsPanel } from './AccountsPanel';
import { ColorLabelsPanel } from './ColorLabelsPanel';
import { Modal } from './Modal';

/**
 * Settings dialog — single entry point for global preferences.
 *
 * Replaces the standalone `AccountsDialog` and `ColorLabelDialog` of
 * earlier phases. The two halves now live as `AccountsPanel` and
 * `ColorLabelsPanel` and render inside a `role="tabpanel"`.
 *
 * Keyboard model (W3C APG vertical-tabs pattern):
 *   - Tablist sits on the left, vertical orientation.
 *   - Arrow Up / Down: move the focused tab. Wraps at ends.
 *   - Home / End: first / last tab.
 *   - Tab: leaves the tablist and enters the panel content.
 *   - Activation: automatic on focus (manual activation isn't worth
 *     the extra keystroke for a 2-tab dialog; panels are cheap to
 *     mount).
 *
 * The active tab uses `tabIndex=0`; the others use `tabIndex=-1` —
 * the roving tabindex pattern that the W3C example uses for trees
 * and tablists alike. Focus is moved imperatively when the arrow
 * keys change the active tab.
 */

export type SettingsTabId = 'accounts' | 'colorLabels';

const TAB_ORDER: SettingsTabId[] = ['accounts', 'colorLabels'];

export interface SettingsDialogProps {
  isOpen: boolean;
  onClose: () => void;
  initialTab?: SettingsTabId;
}

export function SettingsDialog({
  isOpen,
  onClose,
  initialTab,
}: SettingsDialogProps) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<SettingsTabId>(
    initialTab ?? TAB_ORDER[0],
  );

  // When the dialog re-opens with a different initialTab, honour it.
  useEffect(() => {
    if (isOpen && initialTab) setActiveTab(initialTab);
  }, [isOpen, initialTab]);

  const idPrefix = useId();
  const tabId = useCallback(
    (id: SettingsTabId) => `${idPrefix}-tab-${id}`,
    [idPrefix],
  );
  const panelId = useCallback(
    (id: SettingsTabId) => `${idPrefix}-panel-${id}`,
    [idPrefix],
  );

  // One ref per tab so arrow-key activation can move DOM focus to
  // the newly-active tab (the roving-tabindex contract requires it).
  const tabRefs = useRef<Record<SettingsTabId, HTMLButtonElement | null>>({
    accounts: null,
    colorLabels: null,
  });

  const focusTab = useCallback(
    (id: SettingsTabId) => {
      // Move focus to the destination tab BEFORE updating activeTab.
      //
      // The previous order was: state update first, then RAF-deferred
      // focus. That meant the still-focused old tab caught its own
      // aria-selected flipping from true to false, and NVDA fired a
      // state-change announcement on it — the user heard the tab name
      // a second time as a "plain text via aria live" pulse on top of
      // the normal focus event on the new tab.
      //
      // Focusing first peels DOM focus off the old element before its
      // selection state changes, so the state-change announcement has
      // no focused element to attach to. The setActiveTab call then
      // updates aria-selected / tabIndex in the same render; the new
      // tab is already focused when those land. Calling .focus() on
      // an element with tabIndex=-1 is fine — tabIndex only gates
      // Tab-key navigation, not programmatic focus.
      tabRefs.current[id]?.focus({ preventScroll: true });
      setActiveTab(id);
    },
    [],
  );

  const onTablistKey = useCallback(
    (e: ReactKeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const idx = TAB_ORDER.indexOf(activeTab);
      switch (e.key) {
        case 'ArrowDown':
        case 'ArrowRight':
          e.preventDefault();
          focusTab(TAB_ORDER[(idx + 1) % TAB_ORDER.length]);
          return;
        case 'ArrowUp':
        case 'ArrowLeft':
          e.preventDefault();
          focusTab(TAB_ORDER[(idx - 1 + TAB_ORDER.length) % TAB_ORDER.length]);
          return;
        case 'Home':
          e.preventDefault();
          focusTab(TAB_ORDER[0]);
          return;
        case 'End':
          e.preventDefault();
          focusTab(TAB_ORDER[TAB_ORDER.length - 1]);
          return;
      }
    },
    [activeTab, focusTab],
  );

  const labels = useMemo(
    () =>
      ({
        accounts: t('dialogs.settings.tabs.accounts'),
        colorLabels: t('dialogs.settings.tabs.colorLabels'),
      }) as Record<SettingsTabId, string>,
    [t],
  );

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={t('dialogs.settings.title')}
      className="modal--settings"
    >
      <div className="settings">
        <div
          role="tablist"
          aria-label={t('dialogs.settings.tablistAria')}
          aria-orientation="vertical"
          onKeyDown={onTablistKey}
          className="settings__tablist"
        >
          {TAB_ORDER.map((id) => {
            const selected = id === activeTab;
            return (
              <button
                key={id}
                ref={(el) => {
                  tabRefs.current[id] = el;
                }}
                id={tabId(id)}
                role="tab"
                type="button"
                aria-selected={selected}
                // `aria-controls` is intentionally OMITTED on every
                // tab. The W3C APG recommends it, but it makes NVDA
                // follow the relationship on focus and read whatever
                // the first interactive control in the panel sounds
                // like — for our two panels those happen to be a
                // listbox labelled "Verbundene Konten" / "Vorhandene
                // Farb-Labels" and a kind-picker combobox that NVDA
                // announces as "Lokal, 1 of 8". Both contain the tab
                // label as a substring, so the user hears the tab
                // name a second time as plain text via aria-live.
                // Dropping aria-controls breaks no functionality:
                // the visual / DOM relationship is obvious, the role
                // pair (tab + tabpanel) still carries the semantic
                // meaning, and keyboard navigation lands in the
                // panel anyway when the user Tabs past the tablist.
                tabIndex={selected ? 0 : -1}
                className={
                  'settings__tab' +
                  (selected ? ' settings__tab--active' : '')
                }
                // Mouse / click activation: switch tab. Focus stays
                // with the click target; the roving-tabindex update
                // happens via the same state.
                onClick={() => setActiveTab(id)}
              >
                {labels[id]}
              </button>
            );
          })}
        </div>

        {/* No `tabIndex` on the tabpanel: both panels host plenty of
            focusable controls (the W3C APG carve-out), and adding the
            panel itself as a tab stop made NVDA pause on it as an
            "Eigenschaftsfeld" with no interaction — a confusing extra
            beat between the active tab and the first real control.
            We also intentionally OMIT `aria-labelledby` here. The
            APG pattern recommends pointing it back at the active tab,
            but combined with `aria-controls` on the tab that creates
            a round-trip the screen reader walks on focus: the tab
            name gets read first as the focused control and then again
            as the controlled panel's accessible name. Dropping the
            label-by leaves a nameless tabpanel — NVDA still places it
            in its landmark list as "tabpanel" — and the focused tab
            gets announced exactly once, the way the user expects. */}
        <div
          role="tabpanel"
          id={panelId(activeTab)}
          className="settings__panel"
        >
          {activeTab === 'accounts' && <AccountsPanel />}
          {activeTab === 'colorLabels' && <ColorLabelsPanel />}
        </div>
      </div>

      <div className="form__actions settings__footer">
        <button
          type="button"
          onClick={onClose}
          className="form__action form__action--primary"
        >
          {t('dialogs.close')}
        </button>
      </div>
    </Modal>
  );
}
