import { Host, Picker, Text } from '@expo/ui/swift-ui';
import {
  accessibilityElement,
  accessibilityLabel,
  pickerStyle,
  tag,
} from '@expo/ui/swift-ui/modifiers';

import type { NamedSegmentedControlProps } from './NamedSegmentedControl';

/**
 * iOS: a native segmented control (SwiftUI Picker, `.segmented`, backed by a
 * UISegmentedControl) that VoiceOver names.
 *
 * A bare `.accessibilityLabel` on a segmented Picker is not spoken: it lands on
 * the control's own node, a container VoiceOver never focuses, and VoiceOver
 * reads only the segments ("Klein, 1 von 3"; Toni's iPhone, build 74, with
 * @expo/ui 58's community SegmentedControl, which does exactly that). Marked
 * `.accessibilityElement(children: .contain)` first, the control becomes a
 * named container: VoiceOver speaks the name once, as focus enters it, and the
 * segments stay elements of their own (the pattern CVS Health's iOS
 * accessibility guide gives for segmented Pickers). So the visible legend can
 * be hidden from VoiceOver and the field costs one stop.
 */
export function NamedSegmentedControl({
  name,
  values,
  selectedIndex,
  onSelect,
}: NamedSegmentedControlProps) {
  return (
    <Host matchContents={{ vertical: true }} ignoreSafeArea="all">
      <Picker
        label=""
        selection={selectedIndex}
        onSelectionChange={(index: number) => onSelect(index)}
        modifiers={[
          pickerStyle('segmented'),
          accessibilityElement('contain'),
          accessibilityLabel(name),
        ]}
      >
        {values.map((value, index) => (
          <Text key={index} modifiers={[tag(index)]}>
            {value}
          </Text>
        ))}
      </Picker>
    </Host>
  );
}
