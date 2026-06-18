// Make vitest's global APIs (describe/it/expect/vi …) and the jest-dom matcher
// augmentations visible to `tsc --noEmit`, which typechecks the *.test.tsx
// files alongside the app. vite.config.ts sets `test.globals: true`, so the
// tests use these without importing them — this reference makes that type-safe.
/// <reference types="vitest/globals" />
/// <reference types="@testing-library/jest-dom" />
