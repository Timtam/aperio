import type { ColorLabel } from '../api/types';

/**
 * A color-label `<select>` with a live swatch showing the currently
 * selected color, so the user sees their choice at a glance instead of
 * having to read the option name. Used by the event + task dialogs.
 *
 * Native `<option>`s can't render a reliable cross-platform color chip,
 * so the indicator is a swatch rendered alongside the select. The empty
 * ("no color label") choice shows an outlined, slashed placeholder so the
 * indicator stays visible even when nothing is picked.
 */
export function ColorLabelSelect({
  value,
  onChange,
  labels,
  noneLabel,
}: {
  value: string | null;
  onChange: (next: string | null) => void;
  labels: ColorLabel[];
  /** Localized text for the empty "no color label" option. */
  noneLabel: string;
}) {
  const selected = labels.find((l) => l.id === value) ?? null;
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
        {labels.map((label) => (
          <option key={label.id} value={label.id}>
            {label.name}
          </option>
        ))}
      </select>
    </span>
  );
}
