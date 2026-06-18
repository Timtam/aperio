import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import type { ColorLabel } from '@aperio/shared';

import { RadioGroup } from './RadioGroup';

/**
 * Accessible colour-label picker: a radio group over "No colour" + every named
 * label. For a screen-reader user the label's NAME (e.g. "Work") is the
 * meaningful content — the colour itself is visual — so options read by name;
 * ad-hoc one-off colours are excluded (they're not user-named). `value` is the
 * bound `color_label` id, `''` meaning none; `onChange('')` clears it.
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
  const options = useMemo(
    () => [
      { value: '', label: t('dialogs.task.section.noColor') },
      ...labels
        .filter((l) => !l.ad_hoc)
        .map((l) => ({ value: l.id, label: l.name })),
    ],
    [labels, t],
  );
  return (
    <RadioGroup<string>
      label={t('dialogs.colorLabels.fields.color')}
      value={value}
      options={options}
      onChange={onChange}
      disabled={disabled}
    />
  );
}
