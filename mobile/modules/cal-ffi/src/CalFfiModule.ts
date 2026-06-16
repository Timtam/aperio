import { NativeModule, requireNativeModule } from 'expo';

import { ParsedAttendee } from './CalFfi.types';

declare class CalFfiModule extends NativeModule<{}> {
  /**
   * Parse an attendee entry (`"Name <email>"` or a bare address) by calling
   * the shared Rust `cal-core` parser through UniFFI.
   */
  parseAttendee(entry: string): ParsedAttendee;
}

export default requireNativeModule<CalFfiModule>('CalFfi');
