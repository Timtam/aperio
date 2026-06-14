---
title: "Contributing"
---

## Branching

Short-lived topic branches off `main`, named by intent:

| Prefix | For |
|---|---|
| `feat/` | a new feature |
| `fix/` | a bug fix |
| `docs/` | documentation only |
| `chore/`, `refactor/`, `perf/` | housekeeping, restructuring, performance |

Never commit straight to `main`; open a pull request.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org/) are
recommended:

```text
feat(tasks): add assignee picker to the task dialog
fix(month): uniform day-cell height
docs(dev): document the plugin ABI
```

- Subject in the imperative, present tense.
- Body explains *why*, not just *what*, when the change isn't obvious.
- Dev-facing text (commits, code comments, READMEs, this book) is in
  **English**. User-facing UI strings are localised through the app's i18n
  files (`src/locales/de` + `src/locales/en`) — never hard-code a visible
  string.

## Pull requests

A PR should:

1. Be focused — one logical change.
2. Pass the full local gate (see below) before review.
3. Update documentation for any user-visible or developer-visible change
   (the PR template carries a docs checklist — `DESIGN.md` § 24.6).
4. Describe the change and how it was tested.

## Accessibility is a gate, not a nice-to-have

Aperio's reason to exist is being usable with a screen reader. Every PR
that touches the UI must keep that promise:

- Colour is never the **only** signal (WCAG 1.4.1) — pair it with text or
  an accessible label. (E.g. events carry the container name in their
  aria-label even though they're also colour-coded.)
- Everything is operable from the keyboard (WCAG 2.1.1) — including custom
  controls like the colour picker.
- Custom widgets expose correct ARIA roles/states and a sensible focus
  order.
- The app runs in `role="application"` so screen-reader users never have to
  switch to browse mode; new interactive surfaces must keep that working.

If you can't verify it with a screen reader yourself, say so in the PR so a
reviewer who can will check it.

## Code style — fix warnings immediately, technically

The project keeps a zero-warning bar and fixes warnings the moment they
appear, with a real fix rather than a suppression.

- **Rust:** `cargo fmt` formats; `cargo clippy --workspace --all-targets --
  -D warnings` must be clean. Don't reach for `#[allow(...)]` to silence a
  lint unless there's a concrete, documented reason a technical fix isn't
  viable.
- **TypeScript/React:** `tsc` must pass; ESLint must be clean — no
  `// eslint-disable` or `@ts-ignore` to paper over a warning. Common ones
  have standard fixes:
  - `react-refresh/only-export-components` → move the non-component
    export (hook/helper/context) into its own file.
  - `react-hooks/exhaustive-deps` → add the missing dependency, or
    stabilise it with `useCallback`/`useMemo`.

## Before you push

Run exactly what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npx tsc --noEmit && npm run lint && npm run test
```

A green local run here means a green CI run.

> **Line endings.** The repo is mostly LF with a few CRLF files. When a
> script edits files, write in binary mode (or use an editor that
> preserves EOL) so you don't flip line endings. Cargo tooling occasionally
> renormalises `src-tauri/Cargo.toml` — revert any such spurious EOL flip
> before committing.
