import { registerWebModule, NativeModule } from 'expo';

// CalFfiModule is not available on the web platform.
class CalFfiModule extends NativeModule<Record<never, never>> {}

export default registerWebModule(CalFfiModule, 'CalFfiModule');
