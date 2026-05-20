import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import { useTranslation } from 'react-i18next';

import { useAnnouncer } from '../a11y/Announcer';
import {
  clearContainerNameOverride,
  createCalendar,
  createTaskList,
  deleteCalendar,
  isCommandError,
  renameContainer,
  type ContainerKind,
} from '../api/client';
import { useCalendarStore } from '../state/CalendarStore';
import { useDialogState } from '../state/DialogState';
import {
  accountTriState,
  buildSidebarTree,
  parentTriState,
  type AccountNode,
  type LeafNode,
  type SectionNode,
  type TriState,
} from '../state/sidebarTree';
import { useSidebarExpansion } from '../state/useSidebarExpansion';

/**
 * Sidebar: tree view of accounts → sections (Calendars / Tasks) →
 * individual containers, with multi-select via space and a single
 * tab stop on the container (`aria-activedescendant`).
 *
 * Keyboard model (W3C ARIA APG, treeview multi-select variant):
 *
 *   - ↑/↓     move focus between visible items (skipping collapsed
 *             children)
 *   - ←       on an open parent: collapse; on a leaf or closed parent:
 *             move to parent
 *   - →       on a closed parent: expand; on an open parent: focus
 *             first child
 *   - Home    first visible item
 *   - End     last visible item
 *   - Space   toggle selection on the focused item. For a parent
 *             with a `mixed` or `checked` state this clears every
 *             descendant; for `unchecked` it selects every descendant.
 *   - Enter   alias for Space
 *   - a-z     type-ahead: focus the next visible item whose name
 *             starts with the typed letter
 *
 * The flat list of visible items is recomputed on every render from
 * the tree + expansion state. That's O(n) over a handful of dozen
 * items and saves the alternative book-keeping (DOM walks, refs to
 * every item) which would be brittler with React's render model.
 *
 * Rename, delete, and create are kept as inline actions on the leaf
 * rows — the structural change is purely in the tree wrapping.
 */
export function Sidebar() {
  const { t } = useTranslation();
  const announce = useAnnouncer();
  const {
    accounts,
    calendars,
    selectedCalendarIds,
    toggleCalendar,
    refreshCalendars,
    taskLists,
    selectedTaskListIds,
    toggleTaskList,
    refreshTaskLists,
  } = useCalendarStore();
  const { openColorLabels, openAccounts } = useDialogState();
  const expansion = useSidebarExpansion();

  const tree = useMemo(
    () =>
      buildSidebarTree({
        accounts,
        calendars,
        taskLists,
        selectedCalendarIds,
        selectedTaskListIds,
      }),
    [accounts, calendars, taskLists, selectedCalendarIds, selectedTaskListIds],
  );

  // Flatten the tree to the list the user can navigate through. Each
  // entry carries enough context to render itself and to handle a
  // toggle/expand/collapse action without re-walking the tree. Hidden
  // children (those of a collapsed parent) are omitted entirely.
  const visible = useMemo<VisibleItem[]>(
    () => flattenTree(tree, expansion.isExpanded),
    [tree, expansion],
  );

  // ── Active descendant + keyboard navigation ───────────────────────
  const treeId = useId();
  const itemId = useCallback(
    (key: string) => `${treeId}-node-${key}`,
    [treeId],
  );

  const [focusedKey, setFocusedKey] = useState<string | null>(null);

  // Once the tree first populates, focus the first item so arrow-key
  // navigation has a starting point. Don't override an existing
  // focused key (the user may have already moved around).
  useEffect(() => {
    if (focusedKey === null && visible.length > 0) {
      setFocusedKey(visible[0].key);
    }
    // If the focused row disappears (account deleted, section
    // collapsed), fall back to the closest survivor.
    if (focusedKey !== null && !visible.some((v) => v.key === focusedKey)) {
      setFocusedKey(visible[0]?.key ?? null);
    }
  }, [visible, focusedKey]);

  // ── Inline rename plumbing ────────────────────────────────────────
  // Identifies the leaf currently in edit mode (`null` = no edit).
  const [editing, setEditing] = useState<{
    kind: ContainerKind;
    id: string;
  } | null>(null);
  const [draft, setDraft] = useState('');
  const [restoreTarget, setRestoreTarget] = useState<{
    kind: ContainerKind;
    id: string;
  } | null>(null);

  useEffect(() => {
    if (!restoreTarget) return;
    const sel =
      `[data-rename-target-id="${CSS.escape(restoreTarget.id)}"]` +
      `[data-rename-target-kind="${restoreTarget.kind}"]`;
    const btn = document.querySelector(sel);
    if (btn instanceof HTMLElement) {
      btn.focus({ preventScroll: true });
    }
    setRestoreTarget(null);
  }, [restoreTarget]);

  const startEdit = useCallback(
    (kind: ContainerKind, id: string, currentName: string) => {
      setEditing({ kind, id });
      setDraft(currentName);
    },
    [],
  );

  const cancelEdit = useCallback(
    (restoreFocus: boolean) => {
      if (!editing) return;
      const target = restoreFocus ? { ...editing } : null;
      setEditing(null);
      setDraft('');
      if (target) setRestoreTarget(target);
    },
    [editing],
  );

  const commitEdit = useCallback(
    async (restoreFocus: boolean) => {
      if (!editing) return;
      const { kind, id } = editing;
      const trimmed = draft.trim();
      const target = restoreFocus ? { kind, id } : null;
      try {
        if (trimmed === '') {
          await clearContainerNameOverride(id, kind);
          announce(t('sidebar.renameCleared'));
        } else {
          const outcome = await renameContainer(id, kind, trimmed);
          announce(
            t(
              outcome.synced_to_source
                ? 'sidebar.renamedSynced'
                : 'sidebar.renamedLocalOnly',
              { name: trimmed },
            ),
          );
        }
        if (kind === 'calendar') {
          await refreshCalendars();
        } else {
          await refreshTaskLists();
        }
      } catch (err) {
        if (isCommandError(err)) {
          announce(`${err.code}: ${err.message}`);
        } else {
          announce(String(err));
        }
      } finally {
        setEditing(null);
        setDraft('');
        if (target) setRestoreTarget(target);
      }
    },
    [editing, draft, refreshCalendars, refreshTaskLists, announce, t],
  );

  const onEditKey = useCallback(
    (e: KeyboardEvent<HTMLInputElement>) => {
      if (e.key === 'Enter') {
        e.preventDefault();
        void commitEdit(true);
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        cancelEdit(true);
      }
    },
    [commitEdit, cancelEdit],
  );

  // ── Create / delete actions ──────────────────────────────────────
  const onCreateCalendar = useCallback(async () => {
    try {
      const cal = await createCalendar({
        name: t('sidebar.newCalendarName', { n: calendars.length + 1 }),
        color_hex: '#1e88e5',
      });
      await refreshCalendars();
      announce(t('sidebar.calendarCreated', { name: cal.name }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('create_calendar failed', err);
    }
  }, [calendars.length, refreshCalendars, announce, t]);

  const onDeleteCalendar = useCallback(
    async (id: string, name: string) => {
      try {
        await deleteCalendar(id);
        await refreshCalendars();
        announce(t('sidebar.calendarDeleted', { name }));
      } catch (err) {
        // eslint-disable-next-line no-console
        console.warn('delete_calendar failed', err);
      }
    },
    [refreshCalendars, announce, t],
  );

  const onCreateTaskList = useCallback(async () => {
    try {
      const list = await createTaskList({
        name: t('sidebar.newTaskListName', { n: taskLists.length + 1 }),
        embedded_in_calendar: null,
      });
      await refreshTaskLists();
      announce(t('sidebar.taskListCreated', { name: list.name }));
    } catch (err) {
      // eslint-disable-next-line no-console
      console.warn('create_task_list failed', err);
    }
  }, [taskLists.length, refreshTaskLists, announce, t]);

  // ── Multi-toggle: flip every descendant of a parent on/off ───────
  const setManyCalendars = useCallback(
    (ids: string[], next: boolean) => {
      // Toggle reaches into the store one id at a time — the store
      // batches the state-update internally because each toggle is
      // a setState callback.
      for (const id of ids) {
        const isOn = selectedCalendarIds.has(id);
        if (isOn !== next) toggleCalendar(id);
      }
    },
    [selectedCalendarIds, toggleCalendar],
  );
  const setManyTaskLists = useCallback(
    (ids: string[], next: boolean) => {
      for (const id of ids) {
        const isOn = selectedTaskListIds.has(id);
        if (isOn !== next) toggleTaskList(id);
      }
    },
    [selectedTaskListIds, toggleTaskList],
  );

  // ── Keyboard handler on the tree container ───────────────────────
  // Type-ahead buffer: collected single-character keypresses within
  // a 700ms window become a prefix search.
  const typeBuffer = useRef('');
  const typeTimer = useRef<number | null>(null);

  const focusByIndex = useCallback(
    (idx: number) => {
      if (idx < 0 || idx >= visible.length) return;
      setFocusedKey(visible[idx].key);
    },
    [visible],
  );

  const onTreeKey = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      if (focusedKey === null || visible.length === 0) return;
      const idx = visible.findIndex((v) => v.key === focusedKey);
      if (idx < 0) return;
      const item = visible[idx];

      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          focusByIndex(Math.min(idx + 1, visible.length - 1));
          return;
        case 'ArrowUp':
          e.preventDefault();
          focusByIndex(Math.max(idx - 1, 0));
          return;
        case 'Home':
          e.preventDefault();
          focusByIndex(0);
          return;
        case 'End':
          e.preventDefault();
          focusByIndex(visible.length - 1);
          return;
        case 'ArrowRight':
          e.preventDefault();
          if (item.kind === 'leaf') return;
          if (expansion.isExpanded(item.key)) {
            // Already open → move to first child if any.
            const first = visible.find(
              (v, i) => i > idx && v.parentKey === item.key,
            );
            if (first) setFocusedKey(first.key);
          } else {
            expansion.setExpanded(item.key, true);
          }
          return;
        case 'ArrowLeft':
          e.preventDefault();
          if (item.kind !== 'leaf' && expansion.isExpanded(item.key)) {
            expansion.setExpanded(item.key, false);
          } else if (item.parentKey) {
            setFocusedKey(item.parentKey);
          }
          return;
        case ' ':
        case 'Spacebar':
        case 'Enter':
          e.preventDefault();
          toggleItem(item);
          return;
      }

      // Type-ahead: collect printable single chars (no modifiers
      // beyond shift), reset on 700ms idle, jump to the next item
      // whose name starts with the buffer.
      if (
        e.key.length === 1 &&
        !e.ctrlKey &&
        !e.metaKey &&
        !e.altKey &&
        /\S/.test(e.key)
      ) {
        typeBuffer.current = (typeBuffer.current + e.key).toLowerCase();
        if (typeTimer.current !== null) {
          window.clearTimeout(typeTimer.current);
        }
        typeTimer.current = window.setTimeout(() => {
          typeBuffer.current = '';
          typeTimer.current = null;
        }, 700);
        // Search from the next item, wrapping around. This matches
        // the "press 'c' twice to skip past the first c-row" pattern
        // most file managers use.
        const buf = typeBuffer.current;
        const ring = [...visible.slice(idx + 1), ...visible.slice(0, idx + 1)];
        const hit = ring.find((v) =>
          v.label.toLowerCase().startsWith(buf),
        );
        if (hit) setFocusedKey(hit.key);
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [focusedKey, visible, expansion, focusByIndex],
  );

  // Toggle for both leaves (simple selection) and parents (apply to
  // all descendants).
  const toggleItem = useCallback(
    (item: VisibleItem) => {
      if (item.kind === 'leaf') {
        if (item.sectionKind === 'calendars') {
          toggleCalendar(item.containerId);
        } else {
          toggleTaskList(item.containerId);
        }
        return;
      }
      // Parent: collect the leaves and flip them as a batch.
      const leaves = collectLeaves(item, tree);
      const state = leaves.every((l) => l.selected)
        ? 'checked'
        : leaves.some((l) => l.selected)
          ? 'mixed'
          : 'unchecked';
      // mixed / checked → turn everything off; unchecked → on.
      const next = state === 'unchecked';
      const calIds = leaves
        .filter((l) => l.kind === 'calendars')
        .map((l) => l.containerId);
      const tlIds = leaves
        .filter((l) => l.kind === 'tasks')
        .map((l) => l.containerId);
      if (calIds.length) setManyCalendars(calIds, next);
      if (tlIds.length) setManyTaskLists(tlIds, next);
    },
    [tree, toggleCalendar, toggleTaskList, setManyCalendars, setManyTaskLists],
  );

  // ── Render ───────────────────────────────────────────────────────
  return (
    <aside
      className="sidebar"
      aria-label={t('sidebar.label')}
      data-region="sidebar"
    >
      <h2 className="sidebar__heading">{t('sidebar.containersHeading')}</h2>
      {/* The tree itself owns the tab stop. aria-activedescendant
          tells assistive tech which row is "focused" inside the
          tree without React having to focus DOM nodes individually. */}
      <div
        role="tree"
        aria-label={t('sidebar.treeLabel')}
        aria-multiselectable="true"
        tabIndex={0}
        onKeyDown={onTreeKey}
        aria-activedescendant={
          focusedKey ? itemId(focusedKey) : undefined
        }
        className="sidebar__tree"
      >
        {tree.map((account) => (
          <AccountSubtree
            key={account.key}
            account={account}
            expansion={expansion}
            focusedKey={focusedKey}
            onFocusKey={setFocusedKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={setDraft}
            onStartEdit={startEdit}
            onCancelEdit={cancelEdit}
            onCommitEdit={commitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={(leaf) => {
              if (leaf.kind === 'calendars') {
                toggleCalendar(leaf.containerId);
              } else {
                toggleTaskList(leaf.containerId);
              }
            }}
            onToggleSection={(section) => {
              const leaves = section.children;
              const state = parentTriState(leaves);
              const next = state === 'unchecked';
              const ids = leaves.map((l) => l.containerId);
              if (section.kind === 'calendars') {
                setManyCalendars(ids, next);
              } else {
                setManyTaskLists(ids, next);
              }
            }}
            onToggleAccount={(node) => {
              const leaves = node.children.flatMap((s) => s.children);
              const state = accountTriState(node);
              const next = state === 'unchecked';
              const calIds = leaves
                .filter((l) => l.kind === 'calendars')
                .map((l) => l.containerId);
              const tlIds = leaves
                .filter((l) => l.kind === 'tasks')
                .map((l) => l.containerId);
              if (calIds.length) setManyCalendars(calIds, next);
              if (tlIds.length) setManyTaskLists(tlIds, next);
            }}
            onDeleteCalendar={onDeleteCalendar}
          />
        ))}
      </div>

      <div className="sidebar__add-row">
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateCalendar}
        >
          + {t('sidebar.newCalendar')}
        </button>
        <button
          type="button"
          className="sidebar__add"
          onClick={onCreateTaskList}
        >
          + {t('sidebar.newTaskList')}
        </button>
      </div>

      <section className="sidebar__section">
        <button
          type="button"
          className="sidebar__add"
          onClick={() => openColorLabels()}
        >
          {t('sidebar.manageColorLabels')}
        </button>
        <button
          type="button"
          className="sidebar__add"
          onClick={() => openAccounts()}
        >
          {t('sidebar.manageAccounts')}
        </button>
      </section>
    </aside>
  );
}

// ────────────────────────────────────────────────────────────────────────
// Flattened-tree model used for keyboard navigation
// ────────────────────────────────────────────────────────────────────────

type VisibleItem =
  | {
      kind: 'account' | 'section';
      key: string;
      label: string;
      parentKey: string | null;
      level: 1 | 2;
      sectionKind?: undefined;
      containerId?: undefined;
    }
  | {
      kind: 'leaf';
      key: string;
      label: string;
      parentKey: string;
      level: 3;
      sectionKind: 'calendars' | 'tasks';
      containerId: string;
    };

function flattenTree(
  tree: AccountNode[],
  isExpanded: (key: string) => boolean,
): VisibleItem[] {
  const out: VisibleItem[] = [];
  for (const account of tree) {
    out.push({
      kind: 'account',
      key: account.key,
      label: account.displayName,
      parentKey: null,
      level: 1,
    });
    if (!isExpanded(account.key)) continue;
    for (const section of account.children) {
      out.push({
        kind: 'section',
        key: section.key,
        // Label uses the section's own label key; the real
        // localised string is resolved at render time.
        label: section.labelKey,
        parentKey: account.key,
        level: 2,
      });
      if (!isExpanded(section.key)) continue;
      for (const leaf of section.children) {
        out.push({
          kind: 'leaf',
          key: leaf.key,
          label: leaf.name,
          parentKey: section.key,
          level: 3,
          sectionKind: leaf.kind,
          containerId: leaf.containerId,
        });
      }
    }
  }
  return out;
}

function collectLeaves(
  parent: VisibleItem,
  tree: AccountNode[],
): LeafNode[] {
  if (parent.kind === 'leaf') return [];
  // Locate the originating account / section node in the tree by key.
  for (const account of tree) {
    if (account.key === parent.key) {
      return account.children.flatMap((s) => s.children);
    }
    for (const section of account.children) {
      if (section.key === parent.key) {
        return section.children;
      }
    }
  }
  return [];
}

// ────────────────────────────────────────────────────────────────────────
// Subtree rendering
// ────────────────────────────────────────────────────────────────────────

interface AccountSubtreeProps {
  account: AccountNode;
  expansion: ReturnType<typeof useSidebarExpansion>;
  focusedKey: string | null;
  onFocusKey: (key: string) => void;
  itemId: (key: string) => string;
  editing: { kind: ContainerKind; id: string } | null;
  draft: string;
  onDraftChange: (v: string) => void;
  onStartEdit: (kind: ContainerKind, id: string, name: string) => void;
  onCancelEdit: (restoreFocus: boolean) => void;
  onCommitEdit: (restoreFocus: boolean) => Promise<void>;
  onEditKey: (e: KeyboardEvent<HTMLInputElement>) => void;
  onToggleLeaf: (leaf: LeafNode) => void;
  onToggleSection: (section: SectionNode) => void;
  onToggleAccount: (account: AccountNode) => void;
  onDeleteCalendar: (id: string, name: string) => void;
}

function AccountSubtree({
  account,
  expansion,
  focusedKey,
  onFocusKey,
  itemId,
  editing,
  draft,
  onDraftChange,
  onStartEdit,
  onCancelEdit,
  onCommitEdit,
  onEditKey,
  onToggleLeaf,
  onToggleSection,
  onToggleAccount,
  onDeleteCalendar,
}: AccountSubtreeProps) {
  const { t } = useTranslation();
  const isOpen = expansion.isExpanded(account.key);
  const state = accountTriState(account);

  return (
    <div role="group" className="sidebar__account-group">
      <TreeRow
        id={itemId(account.key)}
        level={1}
        expanded={account.children.length > 0 ? isOpen : undefined}
        ariaChecked={triStateToAria(state)}
        focused={focusedKey === account.key}
        onPointerSelect={() => onFocusKey(account.key)}
        onToggleSelect={() => onToggleAccount(account)}
        onToggleExpand={() =>
          expansion.setExpanded(account.key, !isOpen)
        }
        ariaLabel={t('sidebar.tree.accountLabel', {
          name: account.displayName,
          kind: t(`dialogs.accounts.kindName.${account.adapterKind}`),
        })}
        className="sidebar__row sidebar__row--account"
      >
        <span className="sidebar__chevron" aria-hidden="true">
          {account.children.length === 0 ? '·' : isOpen ? '▾' : '▸'}
        </span>
        <span className="sidebar__name">{account.displayName}</span>
        <span className="sidebar__account-kind" aria-hidden="true">
          {t(`dialogs.accounts.kindName.${account.adapterKind}`)}
        </span>
      </TreeRow>
      {account.isEmpty && isOpen && (
        <div role="presentation" className="sidebar__empty-hint">
          {t('sidebar.tree.empty')}
        </div>
      )}
      {isOpen &&
        account.children.map((section) => (
          <SectionSubtree
            key={section.key}
            section={section}
            expansion={expansion}
            focusedKey={focusedKey}
            onFocusKey={onFocusKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={onDraftChange}
            onStartEdit={onStartEdit}
            onCancelEdit={onCancelEdit}
            onCommitEdit={onCommitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={onToggleLeaf}
            onToggleSection={onToggleSection}
            onDeleteCalendar={onDeleteCalendar}
            accountIsLocal={account.accountId === 'local'}
          />
        ))}
    </div>
  );
}

interface SectionSubtreeProps {
  section: SectionNode;
  expansion: ReturnType<typeof useSidebarExpansion>;
  focusedKey: string | null;
  onFocusKey: (key: string) => void;
  itemId: (key: string) => string;
  editing: { kind: ContainerKind; id: string } | null;
  draft: string;
  onDraftChange: (v: string) => void;
  onStartEdit: (kind: ContainerKind, id: string, name: string) => void;
  onCancelEdit: (restoreFocus: boolean) => void;
  onCommitEdit: (restoreFocus: boolean) => Promise<void>;
  onEditKey: (e: KeyboardEvent<HTMLInputElement>) => void;
  onToggleLeaf: (leaf: LeafNode) => void;
  onToggleSection: (section: SectionNode) => void;
  onDeleteCalendar: (id: string, name: string) => void;
  accountIsLocal: boolean;
}

function SectionSubtree({
  section,
  expansion,
  focusedKey,
  onFocusKey,
  itemId,
  editing,
  draft,
  onDraftChange,
  onStartEdit,
  onCancelEdit,
  onCommitEdit,
  onEditKey,
  onToggleLeaf,
  onToggleSection,
  onDeleteCalendar,
  accountIsLocal,
}: SectionSubtreeProps) {
  const { t } = useTranslation();
  const isOpen = expansion.isExpanded(section.key);
  const state = parentTriState(section.children);
  const sectionLabel =
    section.labelKey === 'calendars'
      ? t('sidebar.calendars')
      : t('sidebar.taskLists');

  return (
    <div role="group" className="sidebar__section-group">
      <TreeRow
        id={itemId(section.key)}
        level={2}
        expanded={section.children.length > 0 ? isOpen : undefined}
        ariaChecked={triStateToAria(state)}
        focused={focusedKey === section.key}
        onPointerSelect={() => onFocusKey(section.key)}
        onToggleSelect={() => onToggleSection(section)}
        onToggleExpand={() =>
          expansion.setExpanded(section.key, !isOpen)
        }
        ariaLabel={t('sidebar.tree.sectionLabel', { name: sectionLabel })}
        className="sidebar__row sidebar__row--section"
      >
        <span className="sidebar__chevron" aria-hidden="true">
          {section.children.length === 0 ? '·' : isOpen ? '▾' : '▸'}
        </span>
        <span className="sidebar__name">{sectionLabel}</span>
      </TreeRow>
      {isOpen &&
        section.children.map((leaf) => (
          <LeafRow
            key={leaf.key}
            leaf={leaf}
            focusedKey={focusedKey}
            onFocusKey={onFocusKey}
            itemId={itemId}
            editing={editing}
            draft={draft}
            onDraftChange={onDraftChange}
            onStartEdit={onStartEdit}
            onCancelEdit={onCancelEdit}
            onCommitEdit={onCommitEdit}
            onEditKey={onEditKey}
            onToggleLeaf={onToggleLeaf}
            onDeleteCalendar={onDeleteCalendar}
            // Delete is only ever offered on local calendars — every
            // other source's deletes are still a per-adapter
            // operation we haven't fully wired.
            allowDelete={accountIsLocal && leaf.kind === 'calendars'}
          />
        ))}
    </div>
  );
}

interface LeafRowProps {
  leaf: LeafNode;
  focusedKey: string | null;
  onFocusKey: (key: string) => void;
  itemId: (key: string) => string;
  editing: { kind: ContainerKind; id: string } | null;
  draft: string;
  onDraftChange: (v: string) => void;
  onStartEdit: (kind: ContainerKind, id: string, name: string) => void;
  onCancelEdit: (restoreFocus: boolean) => void;
  onCommitEdit: (restoreFocus: boolean) => Promise<void>;
  onEditKey: (e: KeyboardEvent<HTMLInputElement>) => void;
  onToggleLeaf: (leaf: LeafNode) => void;
  onDeleteCalendar: (id: string, name: string) => void;
  allowDelete: boolean;
}

function LeafRow({
  leaf,
  focusedKey,
  onFocusKey,
  itemId,
  editing,
  draft,
  onDraftChange,
  onStartEdit,
  onCancelEdit,
  onCommitEdit,
  onEditKey,
  onToggleLeaf,
  onDeleteCalendar,
  allowDelete,
}: LeafRowProps) {
  const { t } = useTranslation();
  const kind: ContainerKind = leaf.kind === 'calendars' ? 'calendar' : 'task_list';
  const isEditing = editing?.kind === kind && editing.id === leaf.containerId;

  return (
    <TreeRow
      id={itemId(leaf.key)}
      level={3}
      ariaChecked={leaf.selected ? 'true' : 'false'}
      focused={focusedKey === leaf.key}
      onPointerSelect={() => onFocusKey(leaf.key)}
      onToggleSelect={() => onToggleLeaf(leaf)}
      className="sidebar__row sidebar__row--leaf"
      style={
        leaf.colorHex
          ? ({ '--container-color': leaf.colorHex } as React.CSSProperties)
          : undefined
      }
    >
      {isEditing ? (
        <RenameField
          value={draft}
          onChange={onDraftChange}
          onCommit={onCommitEdit}
          onCancel={onCancelEdit}
          onKeyDown={onEditKey}
          ariaLabel={t('sidebar.renameInputLabel', { name: leaf.name })}
          hint={t('sidebar.renameHint')}
        />
      ) : (
        <>
          <span className="sidebar__swatch" aria-hidden="true" />
          <span className="sidebar__name">{leaf.name}</span>
          <button
            type="button"
            className="sidebar__edit"
            data-rename-target-id={leaf.containerId}
            data-rename-target-kind={kind}
            // Stop the click from bubbling up to the row's
            // onPointerSelect handler (which would set focusedKey but
            // also un-focus the edit button we're about to click).
            onClick={(e) => {
              e.stopPropagation();
              onStartEdit(kind, leaf.containerId, leaf.name);
            }}
            aria-label={t('sidebar.renameButton', { name: leaf.name })}
            title={t('sidebar.renameButtonShort')}
          >
            ✎
          </button>
          {allowDelete && (
            <button
              type="button"
              className="sidebar__delete"
              onClick={(e) => {
                e.stopPropagation();
                onDeleteCalendar(leaf.containerId, leaf.name);
              }}
              aria-label={t('sidebar.deleteCalendar', { name: leaf.name })}
            >
              ✕
            </button>
          )}
        </>
      )}
    </TreeRow>
  );
}

interface TreeRowProps {
  id: string;
  level: 1 | 2 | 3;
  /** Undefined ⇒ no twisty; row has no expand/collapse semantic. */
  expanded?: boolean;
  ariaChecked: 'true' | 'false' | 'mixed';
  focused: boolean;
  onPointerSelect: () => void;
  onToggleSelect: () => void;
  onToggleExpand?: () => void;
  ariaLabel?: string;
  className?: string;
  style?: React.CSSProperties;
  children: React.ReactNode;
}

/**
 * One row in the tree. Renders an element with `role="treeitem"`,
 * the right `aria-level`, `aria-expanded` (where applicable), and
 * `aria-checked` for the tristate selection.
 *
 * Mouse interactions:
 *
 *   - clicking the chevron area (`.sidebar__chevron`) expands /
 *     collapses; clicking the rest toggles the checkbox.
 *
 * We don't put a `tabIndex` on the row — the parent tree owns the
 * single tab stop. Setting `data-focused="true"` lets the stylesheet
 * draw the visible focus ring on the right row.
 */
function TreeRow({
  id,
  level,
  expanded,
  ariaChecked,
  focused,
  onPointerSelect,
  onToggleSelect,
  onToggleExpand,
  ariaLabel,
  className,
  style,
  children,
}: TreeRowProps) {
  return (
    <div
      id={id}
      role="treeitem"
      aria-level={level}
      aria-expanded={expanded}
      aria-checked={ariaChecked}
      aria-label={ariaLabel}
      data-focused={focused || undefined}
      className={className}
      style={style}
      onClick={(e) => {
        // Buttons inside the row handle their own clicks (rename /
        // delete / chevron-area). The fall-through reaches a generic
        // toggle.
        const target = e.target as HTMLElement;
        if (target.closest('button, input')) return;
        onPointerSelect();
        if (
          onToggleExpand &&
          target.closest('.sidebar__chevron')
        ) {
          onToggleExpand();
          return;
        }
        onToggleSelect();
      }}
    >
      {children}
    </div>
  );
}

function triStateToAria(state: TriState): 'true' | 'false' | 'mixed' {
  switch (state) {
    case 'checked':
      return 'true';
    case 'mixed':
      return 'mixed';
    case 'unchecked':
      return 'false';
  }
}

// ────────────────────────────────────────────────────────────────────────
// Inline rename field
// ────────────────────────────────────────────────────────────────────────

function RenameField({
  value,
  onChange,
  onCommit,
  onCancel,
  onKeyDown,
  ariaLabel,
  hint,
}: {
  value: string;
  onChange: (v: string) => void;
  onCommit: (restoreFocus: boolean) => void;
  onCancel: (restoreFocus: boolean) => void;
  onKeyDown: (e: KeyboardEvent<HTMLInputElement>) => void;
  ariaLabel: string;
  hint: string;
}) {
  return (
    <div className="sidebar__rename">
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={onKeyDown}
        onBlur={() => onCommit(false)}
        aria-label={ariaLabel}
        autoFocus
        className="sidebar__rename-input"
      />
      <span className="sidebar__rename-hint" aria-hidden="true">
        {hint}
      </span>
      <button
        type="button"
        className="sidebar__rename-action"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onCommit(true)}
        aria-label={ariaLabel}
      >
        ✓
      </button>
      <button
        type="button"
        className="sidebar__rename-action"
        onMouseDown={(e) => e.preventDefault()}
        onClick={() => onCancel(true)}
        aria-label={ariaLabel}
      >
        ✕
      </button>
    </div>
  );
}
