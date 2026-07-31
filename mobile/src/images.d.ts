// Metro turns a static image import into an asset id — a plain number, which
// is exactly React Native's `ImageRequireSource` and therefore assignable to
// `ImageSourcePropType`.
//
// TypeScript has to be told: neither Expo's `global.d.ts` (which declares CSS
// modules and nothing else) nor React Native's own types cover image imports,
// so without this every `import icon from './icon.png'` is TS2307. The
// alternative is `require()` at the use site, which the repo's ESLint config
// rejects outright — and a declaration is the better answer anyway, because it
// keeps the import statically analysable.

declare module '*.png' {
  const asset: number;
  export default asset;
}

declare module '*.jpg' {
  const asset: number;
  export default asset;
}

declare module '*.jpeg' {
  const asset: number;
  export default asset;
}

declare module '*.gif' {
  const asset: number;
  export default asset;
}

declare module '*.webp' {
  const asset: number;
  export default asset;
}

declare module '*.svg' {
  const asset: number;
  export default asset;
}
