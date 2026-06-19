import { useTranslation } from 'react-i18next';

import type { SoundConfig } from '@aperio/shared';

import { RadioGroup } from './RadioGroup';

// Accessible notification-sound picker (DESIGN §14.4) — the mobile twin of the
// desktop SoundPicker, scoped to what mobile can do: System / Silent / (for a
// container) "Use default" = inherit. Custom sounds aren't pickable on mobile
// (no asset store yet); a custom value synced in from desktop is shown as a
// read-back option so the user sees it's active (picking another replaces it).
// Presentational — the caller (useSoundPref) owns load/save. Built on RadioGroup
// so every option is its own screen-reader focus stop announcing selected state.

type SoundChoice = 'inherit' | 'system' | 'silent' | 'custom';

// OS notifications play at the system volume, so per-config volume is N/A on
// mobile; this keeps the wire SoundConfig well-formed (matches cal_core
// SoundConfig::default().volume) and round-trips a desktop-set volume untouched.
const DEFAULT_VOLUME = 80;

function toChoice(value: SoundConfig | null, allowInherit: boolean): SoundChoice {
  if (value == null) return allowInherit ? 'inherit' : 'system';
  return value.source.type;
}

export function SoundSelect({
  label,
  value,
  allowInherit,
  onChange,
  disabled,
}: {
  label: string;
  value: SoundConfig | null;
  /** Container pickers offer "Use default" (inherit → clear the key); the global
   *  root does not (it IS the default, falling through to System). */
  allowInherit: boolean;
  onChange: (next: SoundConfig | null) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const choice = toChoice(value, allowInherit);

  const options: { value: SoundChoice; label: string }[] = [];
  if (allowInherit) options.push({ value: 'inherit', label: t('reminders.sound.inherit') });
  options.push({ value: 'system', label: t('reminders.sound.system') });
  options.push({ value: 'silent', label: t('reminders.sound.silent') });
  if (choice === 'custom') {
    // Only present as the read-back of an existing (desktop-set) custom sound.
    options.push({ value: 'custom', label: t('reminders.sound.custom') });
  }

  const handle = (next: SoundChoice) => {
    const volume = value?.volume ?? DEFAULT_VOLUME;
    if (next === 'inherit') onChange(null);
    else if (next === 'system') onChange({ source: { type: 'system' }, volume });
    else if (next === 'silent') onChange({ source: { type: 'silent' }, volume });
    // 'custom' is not a newly-pickable choice → no-op.
  };

  return (
    <RadioGroup<SoundChoice>
      label={label}
      value={choice}
      options={options}
      onChange={handle}
      disabled={disabled}
    />
  );
}
