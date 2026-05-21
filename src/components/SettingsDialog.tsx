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
      setActiveTab(id);
      // Defer to the next frame so React commits the new tabIndex=0
      // before we call focus(). Otherwise the focused element still
      // has tabIndex=-1 momentarily and screen readers can get
      // confused about whether the tab is in the tab order.
      requestAnimationFrame(() => {
        tabRefs.current[id]?.focus({ preventScroll: true });
      });
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
                aria-controls={panelId(id)}
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
            beat between the active tab and the first real control. */}
        <div
          role="tabpanel"
          id={panelId(activeTab)}
          aria-labelledby={tabId(activeTab)}
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
