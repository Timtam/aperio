import { SegmentedControl } from '@expo/ui/community/segmented-control';

export interface NamedSegmentedControlProps {
  /** The field's name. iOS speaks it as focus enters the control
   *  (`NamedSegmentedControl.ios.tsx`); Android's control takes none, so there
   *  the caller's visible legend stays a stop of its own before it. */
  name: string;
  values: string[];
  selectedIndex: number;
  onSelect: (index: number) => void;
}

/**
 * A native segmented control: a Material SingleChoiceSegmentedButtonRow on
 * Android (this file), a UISegmentedControl on iOS (the `.ios.tsx` twin, which
 * also carries the name). Selection is by index.
 */
export function NamedSegmentedControl({
  values,
  selectedIndex,
  onSelect,
}: NamedSegmentedControlProps) {
  return (
    <SegmentedControl
      values={values}
      selectedIndex={selectedIndex}
      onChange={(e) => onSelect(e.nativeEvent.selectedSegmentIndex)}
    />
  );
}
