import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { FocusableNote } from '../a11y/FocusableNote';
import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import {
  inspectPluginArchive,
  installPluginArchive,
  isCommandError,
  listFailedPlugins,
  listPlugins,
  listRemotePlugins,
  setPluginEnabled,
  uninstallPlugin,
} from '../api/client';
import type {
  CommandError,
  FailedPluginInfo,
  PluginArchivePreview,
  PluginInfo,
  PluginTypeWire,
  RemotePluginAnnouncement,
} from '../api/types';
import { Modal } from './Modal';
import {
  SettingsSelectorDetail,
  type SettingsSelectorGroup,
} from './SettingsSelectorDetail';

/** Module scope so the memos inside the shared selector stay stable. */
const pluginId = (p: PluginInfo): string => p.id;
const pluginName = (p: PluginInfo): string => p.name;

/** Shape stored per-row in the toggle-error map. We keep the
 *  full envelope so the row can branch on the error code
 *  (active-sync-conflict gets a more actionable message that
 *  points the user at the Sync tab) without re-string-matching
 *  the message body. */
interface ToggleErrorEntry {
  code: CommandError['code'] | 'unknown';
  message: string;
}

/**
 * Settings → Plugins panel (DESIGN.md §20.10).
 *
 * Read-only listing of every plugin the host's `PluginManager`
 * has loaded. v1 surfaces:
 *
 *   - id, name, version, plugin-type
 *   - author + description (from `plugin.json`)
 *   - ABI version + minimum app version
 *   - signature status (manifest-claimed; not verified yet)
 *   - capability badges (calendar / tasks / contacts) plus the
 *     optional named-symbol hooks (interactive_auth, discover,
 *     probe_host_key) so the user can see "this plugin manages
 *     sign-in" / "…has its own service discovery" / "…probes
 *     host keys" at a glance.
 *
 * Enable / disable / install / uninstall (the rest of §20.10's
 * table) need separate infrastructure — runtime gate on the
 * manager for disable, `plugin.uninstalled` event log for
 * uninstall, `.aperio` archive extractor (§20.7) for install.
 * Each is its own future iteration.
 *
 * ## Keyboard shape
 *
 * The installed plugins are ONE `SettingsSelectorDetail` — the accounts
 * list's idiom: a single tab stop, arrow keys, selection follows focus, and
 * only the selected plugin's controls in the tab order.
 *
 * They used to be a focusable card each, grouped under a heading per
 * `plugin_type`. Every card was a focus stop carrying the row's summary, and
 * inside it the switch, the description and the uninstall button were three
 * more — so a dozen plugins put roughly forty stops between the top of the
 * panel and the bottom, with no arrow keys and nothing to skip with. Reaching
 * the last plugin's switch meant Tabbing through everything above it.
 *
 * The type grouping survives as the selector's visual sub-headers, which are
 * presentational: each option's accessible name already carries its type, so
 * arrow keys walk the whole list uninterrupted.
 *
 * The remote-announced and failed sections stay as focusable cards. A listbox
 * earns its keep when a list is long and its rows have controls; those two are
 * usually empty, never long, and carry no actions.
 */
export function PluginsPanel() {
  const { t } = useTranslation();
  const [plugins, setPlugins] = useState<PluginInfo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Per-plugin error envelopes for the toggle. Cleared on
  // successful re-flip; not persisted across re-fetches.
  const [toggleErrors, setToggleErrors] = useState<
    Record<string, ToggleErrorEntry>
  >({});

  // Install dialog state. `pendingInstall` holds the
  // archive-path + preview pair the §20.7 confirmation modal
  // renders against; it stays `null` while no install is
  // in-flight. `installing` gates the dialog buttons during
  // the actual extract + load call. `installError` surfaces
  // any inspect / install failure inline in the modal.
  const [pendingInstall, setPendingInstall] = useState<{
    archivePath: string;
    preview: PluginArchivePreview;
  } | null>(null);
  const [installing, setInstalling] = useState(false);
  const [installError, setInstallError] = useState<ToggleErrorEntry | null>(
    null,
  );

  // Uninstall confirmation state. Stores the plugin pending
  // removal so the modal can render its name + id; the
  // `uninstalling` flag gates the dialog buttons during the
  // actual command call; per-plugin error entries surface
  // inline so the user sees why a removal didn't take.
  const [pendingUninstall, setPendingUninstall] = useState<PluginInfo | null>(
    null,
  );
  const [uninstalling, setUninstalling] = useState(false);
  const [uninstallError, setUninstallError] = useState<ToggleErrorEntry | null>(
    null,
  );

  // §20.8 — plugins other devices have announced that we
  // don't have installed locally. Initial fetch lives in a
  // sibling effect to `listPlugins` so the two views stay
  // independent (a failed remote-plugin fetch shouldn't tank
  // the main list).
  const [remotePlugins, setRemotePlugins] = useState<
    RemotePluginAnnouncement[]
  >([]);

  // Plugin directories the manager refused to load at
  // startup. Most commonly an ABI mismatch after an Aperio
  // update where the user has stale community plugins —
  // without this section those plugins would silently
  // vanish from the loaded list.
  const [failedPlugins, setFailedPlugins] = useState<FailedPluginInfo[]>([]);

  useEffect(() => {
    let cancelled = false;
    listPlugins()
      .then((list) => {
        if (cancelled) return;
        setPlugins(list);
        setLoadError(null);
      })
      .catch((err) => {
        if (cancelled) return;
        const msg = isCommandError(err)
          ? err.message
          : err instanceof Error
            ? err.message
            : String(err);
        setLoadError(msg);
        setPlugins([]);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    listRemotePlugins()
      .then((list) => {
        if (cancelled) return;
        setRemotePlugins(list);
      })
      .catch(() => {
        // Quiet failure — the section just doesn't render.
        // The main plugin list still works, and the next
        // sync round will repopulate.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    listFailedPlugins()
      .then((list) => {
        if (cancelled) return;
        setFailedPlugins(list);
      })
      .catch(() => {
        // Same posture as the remote-plugins fetch — quiet
        // failure, panel without the section.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const onToggle = useCallback(
    async (pluginId: string, enabled: boolean) => {
      // Optimistic update — flip the local row immediately so
      // the toggle feels instant. Revert on error.
      setPlugins((prev) =>
        prev
          ? prev.map((p) => (p.id === pluginId ? { ...p, enabled } : p))
          : prev,
      );
      setToggleErrors((prev) => {
        if (!(pluginId in prev)) return prev;
        const next = { ...prev };
        delete next[pluginId];
        return next;
      });
      try {
        await setPluginEnabled({ plugin_id: pluginId, enabled });
      } catch (err) {
        const entry: ToggleErrorEntry = isCommandError(err)
          ? { code: err.code, message: err.message }
          : {
              code: 'unknown',
              message:
                err instanceof Error ? err.message : String(err),
            };
        // Revert the optimistic flip + surface the error
        // inline so the user can see why the toggle didn't
        // take.
        setPlugins((prev) =>
          prev
            ? prev.map((p) =>
                p.id === pluginId ? { ...p, enabled: !enabled } : p,
              )
            : prev,
        );
        setToggleErrors((prev) => ({ ...prev, [pluginId]: entry }));
      }
    },
    [],
  );

  const onClickInstall = useCallback(async () => {
    setInstallError(null);
    // Native file picker — the dialog plugin is already
    // initialised by the host (see lib.rs's invoke_handler
    // setup). Single-file selection; user can cancel.
    let picked: string | null;
    try {
      picked = (await openFileDialog({
        multiple: false,
        directory: false,
        filters: [
          {
            name: t('dialogs.settings.plugins.install.filterName'),
            extensions: ['aperio'],
          },
        ],
      })) as string | null;
    } catch (err) {
      setInstallError({
        code: 'unknown',
        message: err instanceof Error ? err.message : String(err),
      });
      return;
    }
    if (!picked) return;
    try {
      const preview = await inspectPluginArchive({ archive_path: picked });
      setPendingInstall({ archivePath: picked, preview });
    } catch (err) {
      const entry: ToggleErrorEntry = isCommandError(err)
        ? { code: err.code, message: err.message }
        : {
            code: 'unknown',
            message: err instanceof Error ? err.message : String(err),
          };
      setInstallError(entry);
    }
  }, [t]);

  const onConfirmInstall = useCallback(async () => {
    if (!pendingInstall) return;
    setInstalling(true);
    setInstallError(null);
    try {
      const installed = await installPluginArchive({
        archive_path: pendingInstall.archivePath,
      });
      // Splice the freshly-installed plugin into the list so
      // the user sees it appear without a full refresh. Sort
      // by id to match the backend's stable order.
      setPlugins((prev) => {
        if (!prev) return [installed];
        const without = prev.filter((p) => p.id !== installed.id);
        return [...without, installed].sort((a, b) =>
          a.id.localeCompare(b.id),
        );
      });
      // Drop the matching remote announcement (if any) so
      // the "Plugin benötigt" section updates immediately —
      // the backend also drops it from remote_plugins, but
      // the frontend doesn't get an event for that.
      setRemotePlugins((prev) =>
        prev.filter((r) => r.id !== installed.id),
      );
      // Same for the failed-loads section: if the freshly
      // installed plugin replaces a stale incompatible one,
      // drop its failure row locally.
      setFailedPlugins((prev) =>
        prev.filter((f) => f.id !== installed.id),
      );
      setPendingInstall(null);
    } catch (err) {
      const entry: ToggleErrorEntry = isCommandError(err)
        ? { code: err.code, message: err.message }
        : {
            code: 'unknown',
            message: err instanceof Error ? err.message : String(err),
          };
      setInstallError(entry);
    } finally {
      setInstalling(false);
    }
  }, [pendingInstall]);

  const onCancelInstall = useCallback(() => {
    if (installing) return;
    setPendingInstall(null);
    setInstallError(null);
  }, [installing]);

  const onClickUninstall = useCallback((plugin: PluginInfo) => {
    setUninstallError(null);
    setPendingUninstall(plugin);
  }, []);

  const onConfirmUninstall = useCallback(async () => {
    if (!pendingUninstall) return;
    setUninstalling(true);
    setUninstallError(null);
    try {
      await uninstallPlugin({ plugin_id: pendingUninstall.id });
      setPlugins((prev) =>
        prev ? prev.filter((p) => p.id !== pendingUninstall.id) : prev,
      );
      setPendingUninstall(null);
    } catch (err) {
      const entry: ToggleErrorEntry = isCommandError(err)
        ? { code: err.code, message: err.message }
        : {
            code: 'unknown',
            message: err instanceof Error ? err.message : String(err),
          };
      setUninstallError(entry);
    } finally {
      setUninstalling(false);
    }
  }, [pendingUninstall]);

  const onCancelUninstall = useCallback(() => {
    if (uninstalling) return;
    setPendingUninstall(null);
    setUninstallError(null);
  }, [uninstalling]);

  // Group by plugin_type. Order: calendar-adapter, sync-adapter,
  // videoconference-adapter, notification, then anything else
  // (forward-compat tags) alphabetised. Within each group plugins
  // stay in the backend's id-sorted order.
  const groups = useMemo(() => groupByType(plugins ?? []), [plugins]);

  /** The same grouping, in the shared selector's shape. The group label is the
   *  plugin TYPE, which the selector folds into every option's accessible name
   *  — so arrowing says "Google, Kalender-Adapter, Version 1.2.0, …" without a
   *  heading interrupting the walk. */
  const selectorGroups: SettingsSelectorGroup<PluginInfo>[] = useMemo(
    () =>
      groups.map((g) => ({
        id: g.type,
        label: typeLabel(t, g.type),
        items: g.plugins,
      })),
    [groups, t],
  );

  /** Everything needed to judge a plugin without opening it: version, where it
   *  came from, and whether it is switched on. */
  const pluginSummary = useCallback(
    (p: PluginInfo) =>
      [
        t('dialogs.settings.plugins.version', { version: p.version }),
        p.source === 'bundled'
          ? t('dialogs.settings.plugins.source.bundled')
          : t('dialogs.settings.plugins.source.user'),
        p.enabled
          ? t('dialogs.settings.plugins.toggle.enabled')
          : t('dialogs.settings.plugins.toggle.disabled'),
      ].join(', '),
    [t],
  );

  /** The visible trailing text on each option. `aria-hidden` in the selector,
   *  because the accessible name above already carries the same facts.
   *
   *  It carries the OFF state as well, and that is not decoration: the old
   *  cards dimmed when a plugin was disabled, and moving to a list of options
   *  took that cue away. A screen-reader user hears "deaktiviert" in the
   *  option's name; without this a sighted user would have to select each row
   *  in turn to find out the same thing. */
  const pluginBadge = useCallback(
    (p: PluginInfo) =>
      p.enabled
        ? t('dialogs.settings.plugins.version', { version: p.version })
        : `${t('dialogs.settings.plugins.version', {
            version: p.version,
          })} · ${t('dialogs.settings.plugins.toggle.disabled')}`,
    [t],
  );

  const installedHeadingId = useId();

  if (plugins === null) {
    return (
      <div className="settings-panel plugins-panel">
        <p className="form__hint" aria-live="polite">
          {t('views.loading')}
        </p>
      </div>
    );
  }

  return (
    <div className="settings-panel plugins-panel">
      <FocusableNote className="form__hint">{t('dialogs.settings.plugins.hint')}</FocusableNote>

      <div className="plugins-panel__actions">
        <button
          type="button"
          className="form__action"
          onClick={onClickInstall}
        >
          {t('dialogs.settings.plugins.install.button')}
        </button>
      </div>

      {/* When no install is in flight, surface inspect / picker
          errors here at the top of the panel so the user sees
          why their file pick didn't open the dialog. The
          modal carries its own error region for install-time
          failures (see ConfirmInstallModal). */}
      {!pendingInstall && installError && (
        <p className="form__error" role="alert">
          {t('dialogs.settings.plugins.install.error', {
            error: installError.message,
          })}
        </p>
      )}

      {loadError && (
        <p className="form__error" role="alert">
          {t('dialogs.settings.plugins.loadError', { error: loadError })}
        </p>
      )}

      {failedPlugins.length > 0 && (
        <FailedPluginsSection failed={failedPlugins} />
      )}

      {plugins.length === 0 && !loadError && (
        <FocusableNote className="form__hint">{t('dialogs.settings.plugins.empty')}</FocusableNote>
      )}

      {selectorGroups.length > 0 && (
        <section aria-labelledby={installedHeadingId}>
          <h3
            id={installedHeadingId}
            className="plugins-panel__type-heading"
          >
            {t('dialogs.settings.plugins.installedHeading')}
          </h3>
          {/* One listbox for every installed plugin, grouped by type — the
              accounts list's idiom, and the reason this replaced a stack of
              focusable cards. Each card was a focus stop carrying the row
              summary, and then its switch, its description and its uninstall
              button were three more; twelve plugins meant roughly forty Tab
              presses to cross the panel, with no way to skip and no arrow
              keys. Now the whole list is ONE stop, arrows walk it, and only
              the selected plugin's controls are in the tab order. */}
          <SettingsSelectorDetail<PluginInfo>
            groups={selectorGroups}
            getItemId={pluginId}
            getItemName={pluginName}
            getItemSummary={pluginSummary}
            getItemBadge={pluginBadge}
            selectorLabel={t('dialogs.settings.plugins.selectorLabel')}
            optionLabel={({ account: type, name, summary }) =>
              t('dialogs.settings.plugins.optionLabel', {
                name,
                type,
                summary,
              })
            }
            detailHeading={({ name }) =>
              t('dialogs.settings.plugins.detailHeading', { name })
            }
            // The panel's own heading above is an <h3>; a second one in the
            // same section would read as its sibling rather than as the
            // selected plugin's detail.
            detailHeadingLevel={4}
            renderDetail={(plugin) => (
              <PluginDetail
                plugin={plugin}
                onToggle={onToggle}
                toggleError={toggleErrors[plugin.id] ?? null}
                onUninstall={onClickUninstall}
              />
            )}
          />
        </section>
      )}

      {remotePlugins.length > 0 && (
        <section
          className="plugins-panel__group plugins-panel__group--remote"
          aria-label={t('dialogs.settings.plugins.remote.sectionAria')}
        >
          <h3 className="plugins-panel__type-heading">
            {t('dialogs.settings.plugins.remote.heading')}
          </h3>
          <FocusableNote className="form__hint">
            {t('dialogs.settings.plugins.remote.hint')}
          </FocusableNote>
          <ul className="plugins-panel__list" role="list">
            {remotePlugins.map((r) => {
              // Same focusable-card pattern: tabindex=0 +
              // composed aria-label so the per-row info
              // (name, version, announcing device) is
              // reachable in focus mode.
              const displayName = r.name ?? r.id;
              const deviceLabel =
                r.announced_by_device_name ?? r.announced_by_device;
              const ariaLabel = [
                displayName,
                t('dialogs.settings.plugins.version', {
                  version: r.version,
                }),
                t('dialogs.settings.plugins.remote.announcedBy', {
                  device: deviceLabel,
                }),
              ].join(', ');
              return (
                <li
                  key={r.id}
                  tabIndex={0}
                  className="plugins-panel__row"
                  aria-label={ariaLabel}
                >
                  <div className="plugins-panel__row-header">
                    <span className="plugins-panel__name">{displayName}</span>
                    <span className="plugins-panel__version">
                      {t('dialogs.settings.plugins.version', {
                        version: r.version,
                      })}
                    </span>
                  </div>
                  {r.name && (
                    <div className="plugins-panel__id" aria-hidden="true">
                      {r.id}
                    </div>
                  )}
                  {/* Plain, not a `FocusableNote`: this exact sentence is
                      already the end of the row's own accessible name, so
                      making it focusable spoke it twice and doubled the stops
                      in this section for nothing. */}
                  <p className="form__hint">
                    {t('dialogs.settings.plugins.remote.announcedBy', {
                      device: deviceLabel,
                    })}
                  </p>
                </li>
              );
            })}
          </ul>
        </section>
      )}

      {pendingInstall && (
        <ConfirmInstallModal
          preview={pendingInstall.preview}
          installing={installing}
          error={installError}
          onConfirm={onConfirmInstall}
          onCancel={onCancelInstall}
        />
      )}
      {pendingUninstall && (
        <ConfirmUninstallModal
          plugin={pendingUninstall}
          uninstalling={uninstalling}
          error={uninstallError}
          onConfirm={onConfirmUninstall}
          onCancel={onCancelUninstall}
        />
      )}
    </div>
  );
}

interface ConfirmInstallModalProps {
  preview: PluginArchivePreview;
  installing: boolean;
  error: ToggleErrorEntry | null;
  onConfirm: () => void;
  onCancel: () => void;
}

function ConfirmInstallModal({
  preview,
  installing,
  error,
  onConfirm,
  onCancel,
}: ConfirmInstallModalProps) {
  const { t } = useTranslation();
  const title = preview.already_installed
    ? t('dialogs.settings.plugins.install.titleUpdate')
    : t('dialogs.settings.plugins.install.titleNew');
  return (
    <Modal isOpen={true} onClose={onCancel} title={title}>
      <dl className="plugins-panel__meta">
        <dt>{t('dialogs.settings.plugins.install.name')}</dt>
        <dd>{preview.name}</dd>
        {preview.author && (
          <>
            <dt>{t('dialogs.settings.plugins.author')}</dt>
            <dd>{preview.author}</dd>
          </>
        )}
        <dt>{t('dialogs.settings.plugins.install.version')}</dt>
        <dd>
          {preview.already_installed && preview.installed_version
            ? t('dialogs.settings.plugins.install.versionUpgrade', {
                from: preview.installed_version,
                to: preview.version,
              })
            : preview.version}
        </dd>
        <dt>{t('dialogs.settings.plugins.install.type')}</dt>
        <dd>{typeLabel(t, preview.plugin_type)}</dd>
      </dl>
      {preview.description && (
        <FocusableNote className="plugins-panel__description">{preview.description}</FocusableNote>
      )}
      <p className="form__hint" role="note">
        {t('dialogs.settings.plugins.install.unsignedWarning')}
      </p>
      {error && (
        <p className="form__error" role="alert">
          {error.code === 'restart_required'
            ? t('dialogs.settings.plugins.install.restartRequired')
            : t('dialogs.settings.plugins.install.error', {
                error: error.message,
              })}
        </p>
      )}
      <div className="form__actions">
        <button
          type="button"
          className="form__action form__action--primary"
          onClick={onConfirm}
          disabled={installing}
        >
          {installing
            ? t('dialogs.settings.plugins.install.installing')
            : t('dialogs.settings.plugins.install.confirm')}
        </button>
        <button
          type="button"
          className="form__action"
          onClick={onCancel}
          disabled={installing}
        >
          {t('dialogs.settings.plugins.install.cancel')}
        </button>
      </div>
    </Modal>
  );
}

interface FailedPluginsSectionProps {
  failed: FailedPluginInfo[];
}

/** "Konnten nicht geladen werden"-section. Renders at the
 *  top of the panel (above the per-type groups) because a
 *  user who just updated Aperio + has stale community
 *  plugins should see this prominently. Each row carries an
 *  actionable hint derived from the failure reason. */
function FailedPluginsSection({ failed }: FailedPluginsSectionProps) {
  const { t } = useTranslation();
  return (
    <section
      className="plugins-panel__group plugins-panel__group--failed"
      aria-label={t('dialogs.settings.plugins.failed.sectionAria')}
    >
      <h3 className="plugins-panel__type-heading">
        {t('dialogs.settings.plugins.failed.heading')}
      </h3>
      <FocusableNote className="form__hint">{t('dialogs.settings.plugins.failed.hint')}</FocusableNote>
      <ul className="plugins-panel__list" role="list">
        {failed.map((f) => {
          // Still the focusable-card pattern the installed list has moved off:
          // tabindex=0 + a full aria-label, so the per-row reason is reachable
          // in focus mode without switching to NVDA's browse mode.
          //
          // Kept deliberately. A listbox earns its keep when the list is long
          // and every row has controls; this one is usually empty, never long,
          // and each row carries one disclosure and no actions. Converting it
          // would add a level of indirection to reach strictly less.
          const displayName = f.name ?? f.id ?? basename(f.plugin_dir);
          const ariaLabel = [
            t('dialogs.settings.plugins.failed.rowAria', {
              name: displayName,
            }),
            f.version
              ? t('dialogs.settings.plugins.version', { version: f.version })
              : null,
            reasonHint(t, f.reason),
          ]
            .filter(Boolean)
            .join(', ');
          return (
            <li
              key={f.plugin_dir}
              tabIndex={0}
              className="plugins-panel__row plugins-panel__row--failed"
              aria-label={ariaLabel}
            >
            <div className="plugins-panel__row-header">
              <span className="plugins-panel__name">
                {f.name ?? f.id ?? basename(f.plugin_dir)}
              </span>
              {f.version && (
                <span className="plugins-panel__version">
                  {t('dialogs.settings.plugins.version', {
                    version: f.version,
                  })}
                </span>
              )}
            </div>
            <p className="form__error" role="alert">
              {reasonHint(t, f.reason)}
            </p>
            <details className="plugins-panel__failed-details">
              <summary>
                {t('dialogs.settings.plugins.failed.detailsSummary')}
              </summary>
              <dl className="plugins-panel__meta">
                {f.id && (
                  <>
                    <dt>{t('dialogs.settings.plugins.failed.idLabel')}</dt>
                    <dd>{f.id}</dd>
                  </>
                )}
                <dt>{t('dialogs.settings.plugins.failed.dirLabel')}</dt>
                <dd>
                  <code>{f.plugin_dir}</code>
                </dd>
                <dt>{t('dialogs.settings.plugins.failed.errorLabel')}</dt>
                <dd>
                  <code>{f.error_message}</code>
                </dd>
              </dl>
            </details>
          </li>
          );
        })}
      </ul>
    </section>
  );
}

function basename(path: string): string {
  // Last segment of either / or \ separated path. Used as a
  // last-ditch label when the failure was so early the
  // manifest didn't parse.
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function reasonHint(
  t: (k: string, opts?: Record<string, unknown>) => string,
  reason: FailedPluginInfo['reason'],
): string {
  switch (reason.kind) {
    case 'abi_mismatch':
      // host > plugin → plugin is outdated; host < plugin →
      // user needs to update Aperio. Both cases get distinct
      // copy so the user knows which side to act on.
      return reason.plugin < reason.host
        ? t('dialogs.settings.plugins.failed.reason.abiMismatchOlderPlugin', {
            host: reason.host,
            plugin: reason.plugin,
          })
        : t('dialogs.settings.plugins.failed.reason.abiMismatchNewerPlugin', {
            host: reason.host,
            plugin: reason.plugin,
          });
    case 'app_too_old':
      return t('dialogs.settings.plugins.failed.reason.appTooOld', {
        required: reason.required,
        running: reason.running,
      });
    case 'manifest_invalid':
      return t('dialogs.settings.plugins.failed.reason.manifestInvalid');
    case 'library_load':
      return t('dialogs.settings.plugins.failed.reason.libraryLoad');
    case 'other':
    default:
      return t('dialogs.settings.plugins.failed.reason.other');
  }
}

interface PluginDetailProps {
  plugin: PluginInfo;
  onToggle: (pluginId: string, enabled: boolean) => void;
  toggleError: ToggleErrorEntry | null;
  onUninstall: (plugin: PluginInfo) => void;
}

/**
 * The selected plugin's controls and detail — the right-hand half of the
 * master/detail selector.
 *
 * ## The focus order, and why the switch comes first
 *
 * This used to be a focusable card per plugin, with the switch, the
 * description and the uninstall button as further stops inside it. So reaching
 * the twelfth plugin's switch meant Tabbing past eleven plugins' worth of
 * everything, and there were no arrow keys to skip with.
 *
 * Now only the SELECTED plugin's controls are in the tab order, and they are
 * ordered by what a user came here to do:
 *
 * 1. the enable switch — the reason this panel exists;
 * 2. Uninstall, for user-installed plugins;
 * 3. the description;
 * 4. the technical metadata, behind a disclosure, because author / ABI /
 *    minimum version / signature are looked up once in a blue moon and would
 *    otherwise be four more stops between the list and the switch.
 *
 * Name, version, type, source and on/off are NOT repeated as focus stops here:
 * they are already the selected option's accessible name, so a screen reader
 * has just spoken them. They still render, for the same reason they always
 * did — a sighted user is looking at this pane, not at the option's label.
 */
function PluginDetail({
  plugin,
  onToggle,
  toggleError,
  onUninstall,
}: PluginDetailProps) {
  const { t } = useTranslation();
  const toggleId = `plugin-toggle-${plugin.id}`;
  return (
    <div
      className={
        'plugins-panel__detail' +
        (plugin.enabled ? '' : ' plugins-panel__row--disabled')
      }
    >
      <div className="plugins-panel__toggle">
        <label htmlFor={toggleId} className="plugins-panel__toggle-label">
          {plugin.enabled
            ? t('dialogs.settings.plugins.toggle.enabled')
            : t('dialogs.settings.plugins.toggle.disabled')}
        </label>
        <input
          id={toggleId}
          type="checkbox"
          role="switch"
          checked={plugin.enabled}
          aria-checked={plugin.enabled}
          onChange={(e) => onToggle(plugin.id, e.target.checked)}
        />
      </div>
      {toggleError && (
        <p className="form__error" role="alert">
          {toggleError.code === 'active_sync_conflict'
            ? t('dialogs.settings.plugins.toggle.activeSyncConflict')
            : t('dialogs.settings.plugins.toggle.error', {
                error: toggleError.message,
              })}
        </p>
      )}
      {plugin.source === 'user' && (
        <div className="plugins-panel__row-actions">
          <button
            type="button"
            className="form__action form__action--danger"
            onClick={() => onUninstall(plugin)}
          >
            {t('dialogs.settings.plugins.uninstall.button')}
          </button>
        </div>
      )}
      {plugin.description && (
        <FocusableNote className="plugins-panel__description">
          {plugin.description}
        </FocusableNote>
      )}
      <PluginBadges plugin={plugin} />
      {/* One focusable line, not a `<details>` around a `<dl>`.

          The disclosure looked right and was unreadable. This panel lives
          inside the Settings modal, whose body is `role="application"`, where
          NVDA stays in focus mode and stops ONLY on focusable elements — so
          `<dt>`/`<dd>` are invisible to it. Expanding the disclosure revealed
          markup a screen-reader user could not get into, which is worse than
          not offering it: the affordance says there is something there and
          then there is no way to reach it.

          `FocusableNote` carries the text as the element's accessible name,
          which is the pattern the rest of the settings panels use for exactly
          this reason. One stop, always readable, and the sighted user reads
          the same string. See FocusableNote.tsx. */}
      <FocusableNote className="plugins-panel__technical">
        {technicalLine(t, plugin)}
      </FocusableNote>
    </div>
  );
}

/** The technical facts as one readable line.
 *
 *  Joined with " · " rather than rendered as a definition list: the list was
 *  prettier and could not be read by a screen reader in this dialog (see the
 *  call site). Each fact keeps its own label so the line stays parseable by
 *  ear — "Kennung: com.aperio.… · ABI-Version: 3 · …". */
function technicalLine(
  t: ReturnType<typeof useTranslation>['t'],
  plugin: PluginInfo,
): string {
  return [
    `${t('dialogs.settings.plugins.failed.idLabel')}: ${plugin.id}`,
    plugin.author
      ? `${t('dialogs.settings.plugins.author')}: ${plugin.author}`
      : null,
    `${t('dialogs.settings.plugins.abiVersion')}: ${plugin.abi_version}`,
    `${t('dialogs.settings.plugins.minAppVersion')}: ${plugin.min_app_version}`,
    `${t('dialogs.settings.plugins.signed.label')}: ${
      plugin.signed
        ? t('dialogs.settings.plugins.signed.yes')
        : t('dialogs.settings.plugins.signed.no')
    }`,
    `${t('dialogs.settings.plugins.source.label')}: ${
      plugin.source === 'bundled'
        ? t('dialogs.settings.plugins.source.bundled')
        : t('dialogs.settings.plugins.source.user')
    }`,
  ]
    .filter(Boolean)
    .join(' · ');
}

interface ConfirmUninstallModalProps {
  plugin: PluginInfo;
  uninstalling: boolean;
  error: ToggleErrorEntry | null;
  onConfirm: () => void;
  onCancel: () => void;
}

function ConfirmUninstallModal({
  plugin,
  uninstalling,
  error,
  onConfirm,
  onCancel,
}: ConfirmUninstallModalProps) {
  const { t } = useTranslation();
  return (
    <Modal
      isOpen={true}
      onClose={onCancel}
      title={t('dialogs.settings.plugins.uninstall.title')}
    >
      <FocusableNote>
        {t('dialogs.settings.plugins.uninstall.body', {
          name: plugin.name,
          version: plugin.version,
        })}
      </FocusableNote>
      <FocusableNote className="form__hint">
        {t('dialogs.settings.plugins.uninstall.warning')}
      </FocusableNote>
      {error && (
        <p className="form__error" role="alert">
          {error.code === 'active_sync_conflict'
            ? t('dialogs.settings.plugins.toggle.activeSyncConflict')
            : error.code === 'restart_required'
              ? t('dialogs.settings.plugins.uninstall.restartRequired', {
                  error: error.message,
                })
              : t('dialogs.settings.plugins.uninstall.error', {
                  error: error.message,
                })}
        </p>
      )}
      <div className="form__actions">
        <button
          type="button"
          className="form__action form__action--danger"
          onClick={onConfirm}
          disabled={uninstalling}
        >
          {uninstalling
            ? t('dialogs.settings.plugins.uninstall.uninstalling')
            : t('dialogs.settings.plugins.uninstall.confirm')}
        </button>
        <button
          type="button"
          className="form__action"
          onClick={onCancel}
          disabled={uninstalling}
        >
          {t('dialogs.settings.plugins.uninstall.cancel')}
        </button>
      </div>
    </Modal>
  );
}

/** Render the per-plugin badge row. Each badge is rendered with
 *  a tooltip-style title attribute so screen-reader users get
 *  the full description without the visual chip needing to
 *  expand. */
function PluginBadges({ plugin }: { plugin: PluginInfo }) {
  const { t } = useTranslation();
  const badges: { key: string; label: string; title: string }[] = [];

  for (const cap of plugin.capabilities) {
    badges.push({
      key: `cap-${cap}`,
      label: t(`dialogs.settings.plugins.capability.${cap}`, {
        defaultValue: cap,
      }),
      title: t('dialogs.settings.plugins.capability.title', {
        defaultValue: 'Capability: {{value}}',
        value: cap,
      }),
    });
  }
  if (plugin.has_interactive_auth) {
    badges.push({
      key: 'interactive_auth',
      label: t('dialogs.settings.plugins.hook.interactiveAuth.label'),
      title: t('dialogs.settings.plugins.hook.interactiveAuth.title'),
    });
  }
  if (plugin.has_discover) {
    badges.push({
      key: 'discover',
      label: t('dialogs.settings.plugins.hook.discover.label'),
      title: t('dialogs.settings.plugins.hook.discover.title'),
    });
  }
  if (plugin.has_probe_host_key) {
    badges.push({
      key: 'probe_host_key',
      label: t('dialogs.settings.plugins.hook.probeHostKey.label'),
      title: t('dialogs.settings.plugins.hook.probeHostKey.title'),
    });
  }

  if (badges.length === 0) return null;

  return (
    <ul className="plugins-panel__badges" role="list">
      {badges.map((b) => (
        <li key={b.key} className="plugins-panel__badge" title={b.title}>
          {b.label}
        </li>
      ))}
    </ul>
  );
}

// ── Helpers ────────────────────────────────────────────────────────

interface PluginGroup {
  type: PluginTypeWire;
  plugins: PluginInfo[];
}

const TYPE_ORDER: PluginTypeWire[] = [
  'calendar-adapter',
  'sync-adapter',
  'videoconference-adapter',
  'notification',
];

function groupByType(plugins: PluginInfo[]): PluginGroup[] {
  const byType = new Map<PluginTypeWire, PluginInfo[]>();
  for (const p of plugins) {
    const list = byType.get(p.plugin_type) ?? [];
    list.push(p);
    byType.set(p.plugin_type, list);
  }
  const ordered: PluginGroup[] = [];
  // Known tags first, in canonical order.
  for (const type of TYPE_ORDER) {
    const list = byType.get(type);
    if (list && list.length > 0) {
      ordered.push({ type, plugins: list });
      byType.delete(type);
    }
  }
  // Anything else (forward-compat) alphabetised.
  const remaining = [...byType.entries()].sort(([a], [b]) =>
    a.localeCompare(b),
  );
  for (const [type, list] of remaining) {
    ordered.push({ type, plugins: list });
  }
  return ordered;
}

function typeLabel(
  t: (k: string, opts?: Record<string, unknown>) => string,
  type: PluginTypeWire,
): string {
  // Map wire string → i18n key. Unknown future tags fall back
  // to the literal wire string so the user at least sees what
  // the plugin's manifest claims.
  switch (type) {
    case 'calendar-adapter':
      return t('dialogs.settings.plugins.type.calendarAdapter');
    case 'sync-adapter':
      return t('dialogs.settings.plugins.type.syncAdapter');
    case 'videoconference-adapter':
      return t('dialogs.settings.plugins.type.videoconferenceAdapter');
    case 'notification':
      return t('dialogs.settings.plugins.type.notification');
    default:
      return type;
  }
}
