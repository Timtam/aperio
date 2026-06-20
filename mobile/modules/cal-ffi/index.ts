// Re-export the native module. On web, it will be resolved to CalFfiModule.web.ts
// and on native platforms to CalFfiModule.ts
export { default } from './src/CalFfiModule';
export * from './src/CalFfi.types';
