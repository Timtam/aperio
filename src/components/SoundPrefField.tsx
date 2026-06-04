import { useSoundPref } from '../state/useSoundPref';
import { SoundPicker } from './SoundPicker';

/**
 * Binds a `user_prefs` sound key to a {@link SoundPicker} (DESIGN.md
 * §14.4). Used for every prefs-backed level: the global default
 * (`sound.global`, `allowInherit={false}`), per-calendar /
 * per-tasklist container defaults, and per-item overrides.
 *
 * Pass `prefKey={null}` to render an inert picker — used by the item
 * dialogs on a not-yet-saved item, where there's no id to key against
 * yet.
 */
export interface SoundPrefFieldProps {
  prefKey: string | null;
  /** Offer the "use default / inherit" option. Defaults to true; the
   *  global picker passes false. */
  allowInherit?: boolean;
  legend?: string;
  compact?: boolean;
}

export function SoundPrefField({
  prefKey,
  allowInherit,
  legend,
  compact,
}: SoundPrefFieldProps) {
  const { value, setValue } = useSoundPref(prefKey);
  return (
    <SoundPicker
      value={value}
      onChange={setValue}
      allowInherit={allowInherit}
      legend={legend}
      compact={compact}
    />
  );
}
