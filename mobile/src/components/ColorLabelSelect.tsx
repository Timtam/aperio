import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import type { ColorLabel } from '@aperio/shared';

import { SelectFieldButton } from './SelectFieldButton';

/**
 * Colour-label picker: ONE focus stop (a button carrying the current label's
 * name), opening "No colour" + every named label in a dialog. Serves both
 * audiences — sighted users see a real colour SWATCH on the button and per
 * option, screen-reader users hear the label NAME. `value` is the bound
 * `color_label` id, `''` = none; `onChange('')` clears it. Ad-hoc one-off
 * colours are excluded (they're not user-named). Formerly an inline radio
 * group; collapsed so the editors it sits in stay walkable (see
 * SelectFieldButton).
 */
export function ColorLabelSelect({
  value,
  labels,
  onChange,
  disabled,
}: {
  value: string;
  labels: ColorLabel[];
  onChange: (id: string) => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const named = useMemo(() => labels.filter((l) => !l.ad_hoc), [labels]);
  const options = useMemo(
    () => [
      { value: '', label: t('dialogs.task.section.noColor') },
      ...named.map((l) => ({ value: l.id, label: l.name })),
    ],
    [named, t],
  );
  const hexById = useMemo(
    () => new Map(named.map((l) => [l.id, l.hex])),
    [named],
  );

  return (
    <SelectFieldButton
      label={t('dialogs.colorLabels.fields.color')}
      value={value}
      options={options}
      onChange={onChange}
      disabled={disabled}
      swatchFor={(id) => hexById.get(id) ?? null}
    />
  );
}
