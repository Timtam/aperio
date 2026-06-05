import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { ColorLabel } from '../api/types';
import { ColorPickerModal } from './ColorPickerModal';

/**
 * A color-label `<select>` with a live swatch showing the currently
 * selected color, so the user sees their choice at a glance instead of
 * having to read the option name. Used by the event + task dialogs.
 *
 * Native `<option>`s can't render a reliable cross-platform color chip,
 * so the indicator is a swatch rendered alongside the select. The empty
 * ("no color label") choice shows an outlined, slashed placeholder so the
 * indicator stays visible even when nothing is picked.
 *
 * The dropdown lists only *named* labels; a "custom color…" button opens
 * the {@link ColorPickerModal} to compose an arbitrary color on the fly.
 * If the current value is a hidden ad-hoc color, it's injected as a
 * one-off option so it still shows as selected.
 */
export function ColorLabelSelect({
  value,
  onChange,
  labels,
  noneLabel,
}: {
  value: string | null;
  onChange: (next: string | null) => void;
  /** All color labels (including hidden ad-hoc ones — used for the swatch
   *  + current-value lookup; the dropdown filters them out). */
  labels: ColorLabel[];
  /** Localized text for the empty "no color label" option. */
  noneLabel: string;
}) {
  const { t } = useTranslation();
  const [pickerOpen, setPickerOpen] = useState(false);
  const selected = labels.find((l) => l.id === value) ?? null;
  const namedLabels = labels.filter((l) => !l.ad_hoc);

  return (
    <span className="color-label-select">
      <span
        className={
          'color-label-select__swatch' +
          (selected ? '' : ' color-label-select__swatch--empty')
        }
        aria-hidden="true"
        style={selected ? { background: selected.hex } : undefined}
      />
      <select
        className="color-label-select__select"
        value={value ?? ''}
        onChange={(e) => onChange(e.target.value ? e.target.value : null)}
      >
        <option value="">{noneLabel}</option>
        {namedLabels.map((label) => (
          <option key={label.id} value={label.id}>
            {label.name}
          </option>
        ))}
        {selected?.ad_hoc && (
          <option value={selected.id}>
            {t('dialogs.colorPicker.customOption', { hex: selected.hex })}
          </option>
        )}
      </select>
      <button
        type="button"
        className="color-label-select__custom"
        onClick={() => setPickerOpen(true)}
      >
        {t('dialogs.colorPicker.openAction')}
      </button>
      <ColorPickerModal
        isOpen={pickerOpen}
        onClose={() => setPickerOpen(false)}
        initialHex={selected?.hex}
        onResolve={(label) => onChange(label.id)}
      />
    </span>
  );
}
