import { requireNativeView } from 'expo';
import { type ComponentType, type ReactNode } from 'react';
import { Platform, type ViewProps } from 'react-native';

// A form's content as one ordered VoiceOver container (iOS only): see
// ios/A11yFormModule.swift. Part of the form lab of the diagnostic build;
// FormScrollView renders it only while the lab's switch is on. There is no
// Android module, so the view is looked up on iOS only.

export interface A11yFormContainerProps extends ViewProps {
  ordered: boolean;
  children?: ReactNode;
}

export const A11yFormContainer: ComponentType<A11yFormContainerProps> | null =
  Platform.OS === 'ios' ? requireNativeView('A11yForm') : null;
