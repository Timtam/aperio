import { useCallback, useEffect, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { open as openFileDialog } from '@tauri-apps/plugin-dialog';

import {
  deleteCustomSound,
  importSound,
  isCommandError,
  listCustomSounds,
  previewSound,
  type ImportedSound,
} from '../api/client';
import type { SoundConfig } from '../api/types';

/**
 * Reusable notification-sound picker (DESIGN.md §14.4).
 *
 * Renders an accessible radiogroup (native `<input type="radio">`, so
 * arrow-key navigation and labelling come for free) over the sound
 * options, plus — when "Custom" is chosen — a dropdown of imported
 * sounds, an import button, a Test button, and a remove button.
 *
 * Volume is intentionally NOT exposed: the OS mixer owns per-app
 * volume on every platform we target. We still round-trip the model's
 * `volume` field so a value set on another device (or a future
 * release) isn't clobbered.
 */
export interface SoundPickerProps {
  /** Current value. `null` means "inherit from the next level up"
   *  (only offered when `allowInherit` is true). */
  value: SoundConfig | null;
  onChange: (next: SoundConfig | null) => void;
  /** Offer an "Use default / inherit" option mapping to `null`. The
   *  global picker passes `false` (nothing to inherit from). */
  allowInherit?: boolean;
  /** Tightens spacing for embedding inside a reminder row. */
  compact?: boolean;
  /** Overrides the group legend; defaults to "Notification sound". */
  legend?: string;
}

type Choice = 'inherit' | 'system' | 'silent' | 'custom';

const DEFAULT_VOLUME = 80;

/** Build a `SoundConfig` for a non-custom source, preserving any
 *  existing volume so a cross-device value survives the edit. */
function simpleConfig(type: 'system' | 'silent', prev: SoundConfig | null): SoundConfig {
  return { source: { type }, volume: prev?.volume ?? DEFAULT_VOLUME };
}

function customConfig(sha256: string, prev: SoundConfig | null): SoundConfig {
  return {
    source: { type: 'custom', sha256 },
    volume: prev?.volume ?? DEFAULT_VOLUME,
  };
}

function choiceOf(value: SoundConfig | null): Choice {
  if (value === null) return 'inherit';
  return value.source.type;
}

export function SoundPicker({
  value,
  onChange,
  allowInherit = true,
  compact = false,
  legend,
}: SoundPickerProps) {
  const { t } = useTranslation();
  const groupName = useId();
  const [sounds, setSounds] = useState<ImportedSound[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setSounds(await listCustomSounds());
    } catch {
      // A listing failure just leaves the dropdown empty; the user can
      // still import. No need to surface it loudly.
      setSounds([]);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // When inheritance isn't offered (the global picker), an unset value
  // renders as the effective default — System — rather than leaving no
  // radio checked.
  const choice: Choice =
    value === null && !allowInherit ? 'system' : choiceOf(value);
  const currentHash =
    value?.source.type === 'custom' ? value.source.sha256 : '';

  const doImport = useCallback(async (): Promise<string | null> => {
    setError(null);
    try {
      const selected = await openFileDialog({
        multiple: false,
        directory: false,
        title: t('reminders.sound.dialogTitle'),
        filters: [
          {
            name: t('reminders.sound.fileFilter'),
            extensions: ['mp3', 'ogg', 'wav', 'm4a', 'aac', 'flac'],
          },
        ],
      });
      if (typeof selected !== 'string' || selected.length === 0) {
        return null;
      }
      const imported = await importSound(selected);
      await refresh();
      return imported.sha256;
    } catch (err) {
      setError(
        isCommandError(err)
          ? err.message
          : t('reminders.sound.importError'),
      );
      return null;
    }
  }, [refresh, t]);

  const select = useCallback(
    async (next: Choice) => {
      setError(null);
      switch (next) {
        case 'inherit':
          onChange(null);
          return;
        case 'system':
          onChange(simpleConfig('system', value));
          return;
        case 'silent':
          onChange(simpleConfig('silent', value));
          return;
        case 'custom': {
          // Keep an existing valid hash; otherwise adopt the first
          // stored sound, or fall back to importing one. If the import
          // is cancelled and nothing is available, leave the prior
          // selection untouched rather than committing an empty hash.
          if (currentHash) return;
          if (sounds.length > 0) {
            onChange(customConfig(sounds[0].sha256, value));
            return;
          }
          const hash = await doImport();
          if (hash) onChange(customConfig(hash, value));
          return;
        }
      }
    },
    [onChange, value, currentHash, sounds, doImport],
  );

  const onImportClick = useCallback(async () => {
    const hash = await doImport();
    if (hash) onChange(customConfig(hash, value));
  }, [doImport, onChange, value]);

  const onRemove = useCallback(async () => {
    if (!currentHash) return;
    setError(null);
    try {
      await deleteCustomSound(currentHash);
    } catch {
      // Best-effort: even if the file delete fails, drop the reference
      // so the picker doesn't keep pointing at a broken sound.
    }
    const remaining = sounds.filter((s) => s.sha256 !== currentHash);
    setSounds(remaining);
    if (remaining.length > 0) {
      onChange(customConfig(remaining[0].sha256, value));
    } else {
      onChange(allowInherit ? null : simpleConfig('system', value));
    }
  }, [currentHash, sounds, onChange, value, allowInherit]);

  const onTest = useCallback(() => {
    if (value) void previewSound(value);
  }, [value]);

  const radios: { id: Choice; label: string }[] = [
    ...(allowInherit
      ? [{ id: 'inherit' as const, label: t('reminders.sound.inherit') }]
      : []),
    { id: 'system', label: t('reminders.sound.system') },
    { id: 'silent', label: t('reminders.sound.silent') },
    { id: 'custom', label: t('reminders.sound.custom') },
  ];

  return (
    <fieldset
      className={`form__field sound-picker${compact ? ' sound-picker--compact' : ''}`}
    >
      <legend className="form__label">
        {legend ?? t('reminders.sound.label')}
      </legend>

      <div className="sound-picker__choices" role="radiogroup">
        {radios.map((r) => (
          <label key={r.id} className="sound-picker__choice">
            <input
              type="radio"
              name={groupName}
              checked={choice === r.id}
              onChange={() => void select(r.id)}
            />
            <span>{r.label}</span>
          </label>
        ))}
      </div>

      {choice === 'custom' && (
        <div className="sound-picker__custom">
          {sounds.length > 0 ? (
            <label className="form__field">
              <span className="form__label">
                {t('reminders.sound.selectLabel')}
              </span>
              <select
                value={currentHash}
                onChange={(e) => onChange(customConfig(e.target.value, value))}
              >
                {sounds.map((s) => (
                  <option key={s.sha256} value={s.sha256}>
                    {t('reminders.sound.customItem', {
                      ext: s.ext.toUpperCase(),
                      short: s.sha256.slice(0, 8),
                    })}
                  </option>
                ))}
              </select>
            </label>
          ) : (
            <p className="form__hint">{t('reminders.sound.none')}</p>
          )}

          <div className="sound-picker__actions">
            <button
              type="button"
              className="form__action"
              onClick={() => void onImportClick()}
            >
              {t('reminders.sound.import')}
            </button>
            {currentHash && (
              <>
                <button
                  type="button"
                  className="form__action"
                  onClick={onTest}
                >
                  {t('reminders.sound.test')}
                </button>
                <button
                  type="button"
                  className="form__action form__action--danger"
                  onClick={() => void onRemove()}
                >
                  {t('reminders.sound.remove')}
                </button>
              </>
            )}
          </div>
        </div>
      )}

      {error && (
        <p className="form__error" role="alert">
          {error}
        </p>
      )}
    </fieldset>
  );
}
