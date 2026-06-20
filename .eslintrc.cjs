/* eslint-env node */
module.exports = {
  root: true,
  env: { browser: true, es2022: true, node: true },
  extends: [
    'eslint:recommended',
    'plugin:@typescript-eslint/recommended',
    'plugin:react-hooks/recommended',
  ],
  parser: '@typescript-eslint/parser',
  parserOptions: {
    ecmaVersion: 2022,
    sourceType: 'module',
    ecmaFeatures: { jsx: true },
  },
  settings: { react: { version: '18.3' } },
  plugins: ['@typescript-eslint', 'react-refresh'],
  ignorePatterns: ['dist', 'node_modules', 'src-tauri', 'target', 'crates'],
  overrides: [
    {
      // CommonJS build/config tooling (Metro/Babel configs, Expo config plugins)
      // legitimately use require() + module.exports — they run in Node's CJS
      // context at build time, not as ES modules. Exempt them from the
      // ESM-only require rule rather than littering eslint-disable comments.
      files: [
        '**/*.config.js',
        '**/metro.config.js',
        '**/babel.config.js',
        '**/plugins/**/*.js',
      ],
      parserOptions: { sourceType: 'script' },
      rules: {
        '@typescript-eslint/no-var-requires': 'off',
        '@typescript-eslint/no-require-imports': 'off',
      },
    },
  ],
  rules: {
    'react-refresh/only-export-components': [
      'warn',
      { allowConstantExport: true },
    ],
    '@typescript-eslint/no-unused-vars': [
      'error',
      { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
    ],
  },
};
