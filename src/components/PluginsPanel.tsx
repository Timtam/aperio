import { useCallback, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { isCommandError, listPlugins, setPluginEnabled } from '../api/client';
import type { PluginInfo, PluginTypeWire } from '../api/types';

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
 * The panel groups plugins by their `plugin_type` so calendar,
 * sync, and videoconference adapters render under separate
 * headings — matches the §20.2 mental model.
 */
export function PluginsPanel() {
  const { t } = useTranslation();
  const [plugins, setPlugins] = useState<PluginInfo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // Per-plugin error envelopes for the toggle. Cleared on
  // successful re-flip; not persisted across re-fetches.
  const [toggleErrors, setToggleErrors] = useState<Record<string, string>>({});

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
        const msg = isCommandError(err)
          ? err.message
          : err instanceof Error
            ? err.message
            : String(err);
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
        setToggleErrors((prev) => ({ ...prev, [pluginId]: msg }));
      }
    },
    [],
  );

  // Group by plugin_type. Order: calendar-adapter, sync-adapter,
  // videoconference-adapter, notification, then anything else
  // (forward-compat tags) alphabetised. Within each group plugins
  // stay in the backend's id-sorted order.
  const groups = useMemo(() => groupByType(plugins ?? []), [plugins]);

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
      <p className="form__hint">{t('dialogs.settings.plugins.hint')}</p>

      {loadError && (
        <p className="form__error" role="alert">
          {t('dialogs.settings.plugins.loadError', { error: loadError })}
        </p>
      )}

      {plugins.length === 0 && !loadError && (
        <p className="form__hint">{t('dialogs.settings.plugins.empty')}</p>
      )}

      {groups.map((group) => (
        <section
          key={group.type}
          className="plugins-panel__group"
          aria-label={t('dialogs.settings.plugins.groupAria', {
            type: typeLabel(t, group.type),
          })}
        >
          <h3 className="plugins-panel__type-heading">
            {typeLabel(t, group.type)}
          </h3>
          <ul className="plugins-panel__list" role="list">
            {group.plugins.map((p) => (
              <PluginRow
                key={p.id}
                plugin={p}
                onToggle={onToggle}
                toggleError={toggleErrors[p.id] ?? null}
              />
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}

interface PluginRowProps {
  plugin: PluginInfo;
  onToggle: (pluginId: string, enabled: boolean) => void;
  toggleError: string | null;
}

function PluginRow({ plugin, onToggle, toggleError }: PluginRowProps) {
  const { t } = useTranslation();
  const toggleId = `plugin-toggle-${plugin.id}`;
  return (
    <li
      className={
        'plugins-panel__row' +
        (plugin.enabled ? '' : ' plugins-panel__row--disabled')
      }
      aria-label={t('dialogs.settings.plugins.rowAria', {
        name: plugin.name,
        version: plugin.version,
      })}
    >
      <div className="plugins-panel__row-header">
        <span className="plugins-panel__name">{plugin.name}</span>
        <span className="plugins-panel__version">
          {t('dialogs.settings.plugins.version', { version: plugin.version })}
        </span>
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
      </div>
      <div className="plugins-panel__id" aria-hidden="true">
        {plugin.id}
      </div>
      {plugin.description && (
        <p className="plugins-panel__description">{plugin.description}</p>
      )}
      {toggleError && (
        <p className="form__error" role="alert">
          {t('dialogs.settings.plugins.toggle.error', { error: toggleError })}
        </p>
      )}
      <dl className="plugins-panel__meta">
        {plugin.author && (
          <>
            <dt>{t('dialogs.settings.plugins.author')}</dt>
            <dd>{plugin.author}</dd>
          </>
        )}
        <dt>{t('dialogs.settings.plugins.abiVersion')}</dt>
        <dd>{plugin.abi_version}</dd>
        <dt>{t('dialogs.settings.plugins.minAppVersion')}</dt>
        <dd>{plugin.min_app_version}</dd>
        <dt>{t('dialogs.settings.plugins.signed.label')}</dt>
        <dd>
          {plugin.signed
            ? t('dialogs.settings.plugins.signed.yes')
            : t('dialogs.settings.plugins.signed.no')}
        </dd>
      </dl>
      <PluginBadges plugin={plugin} />
    </li>
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
