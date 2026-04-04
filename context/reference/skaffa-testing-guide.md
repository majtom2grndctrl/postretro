# Testing Guide - for Skaffa, not PostRetro

> **Read this when:** writing new tests, deciding what to test, or setting up test infrastructure.
> **Key invariant:** tests document Skaffa-specific behavior and cross-boundary interactions — not language or framework basics.
> **Related:** [Development Guide](./skaffa_development_guide.md), [IPC Boundaries](./skaffa_ipc_boundaries_and_sequences.md) (cross-process flows)

---

## 1. Test Infrastructure

Vitest with two workspace projects (separate `@/` alias resolution):

| Project | Scope | Path alias `@/` |
|---------|-------|-----------------|
| `skaffa` | Core: main, renderer, extension-host, shared, extensions, packages, scripts | `apps/electron/` |
| `demo` | Demo app tests | `apps/demos/shadcn/frontend/src/` |

Config: `vitest.config.ts` (root). Setup: `tests/setup/vitest.setup.ts`. Helpers: `tests/helpers/react-testing.tsx`.

Default environment is `node`. Component tests that need a DOM use `@vitest-environment jsdom` (file-level docblock).

E2E tests use Playwright (`tests/e2e/`). Require a built app (`pnpm build` before `pnpm test:e2e`).

---

## 2. What to Test

### Priority targets

| Category | Examples |
|----------|----------|
| Cross-process IPC | Renderer-to-main flows, subscription lifecycle, teardown verification |
| Registry-driven behavior | Exposure kinds shaping Inspector UI, prop grouping and ordering |
| User workflows | Action → state change → IPC → effect chains, error states that impact workflow |
| Domain logic | Override conflict detection, graph/registry transforms, module loading boundaries |
| Boundary validation | IPC payload validation, schema parsing, Zod contract enforcement |

### Decision criteria

Test it if **all** of these hold:
- Skaffa-specific behavior (not a language feature or library API)
- Crosses a boundary or shows how the system behaves at a seam
- Captures a real user scenario or documents a workflow for future readers

Skip it otherwise.

---

## 3. What Not to Test

- HTML/React basics (rendering inputs, DOM element types)
- Framework features (state update mechanics, hook behavior)
- External library internals (JSON.parse, Zustand store plumbing)
- CSS classes or Tailwind tokens. Test what the user observes (text renders, element is visible/hidden, section order), not which classes produce it. See [Development Guide](./skaffa_development_guide.md) §7.3.

---

## 4. Test Patterns

### Behavior over implementation

Assert observable outcomes: UI state, IPC calls, store snapshots. Avoid asserting internal data structures that could change without affecting behavior.

### Real interaction flows

Model the actual lifecycle: IPC event → store update → UI render. Over-mocking hides the interactions tests exist to document.

Reference: `apps/electron/renderer/state/__tests__/sync-manager.test.ts` (IPC subscription lifecycle with mock `window.skaffa` API).

### Seam-crossing tests

When testing code that bridges two systems, derive mock inputs from the source system's actual output and assertions from the destination system's contract. If both come from the same mental model, the test proves the component is a passthrough — not that the passthrough is correct.

### Test naming

Names describe the exact behavior and boundary under test. Pattern: `<subject> <verb> <expected outcome>`.

Reference: `apps/electron/main/ipc/validation.test.ts` (file header enumerates the five behaviors the suite covers).

### Stable test harnesses

| Rule | Rationale |
|------|-----------|
| Never replace `globalThis.window` in jsdom tests | Breaks other tests sharing the environment. Attach `window.skaffa` instead. |
| Use `waitFor`/`act` for async UI updates | `setTimeout`-based waits are flaky and slow. |
| Suppress warnings inside the triggering test only | Restore afterward. Global suppression masks real failures. |
| Prefer local fixture modules for module-loader tests | Avoids coupling to build artifacts that may not exist. |

### File-level environment declaration

Component tests requiring a DOM:

```typescript
/**
 * @vitest-environment jsdom
 */
```

Place at the top of the file, before imports.

---

## 5. Test Organization

Tests co-locate with source (`*.test.ts`, `*.test.tsx`). Shared infrastructure in `tests/`:

| Directory | Purpose |
|-----------|---------|
| `tests/setup/` | Global config (`vitest.setup.ts`: Testing Library matchers, cleanup) |
| `tests/helpers/` | Reusable utilities (`react-testing.tsx`: `renderWithProviders`, re-exports) |
| `tests/e2e/` | Playwright E2E specs |

Test files are exempt from the source file size guidance in [Development Guide](./skaffa_development_guide.md) §4.1. Test suites are flat and linear — large is fine.

---

## 6. Running Tests

```bash
pnpm test             # All unit/component tests
pnpm test path/to/file.test.ts  # Single file
pnpm test:ui          # Browser UI
pnpm test:coverage    # With coverage report
pnpm test:e2e         # Playwright E2E (requires pnpm build first)
pnpm typecheck        # Type check (catches descriptor drift, schema mismatches)
```

---

## 7. Non-Goals

- Snapshot testing (fragile, low signal for Skaffa's component model)
- 100% coverage targets (coverage is a tool, not a goal)
- Testing third-party library behavior
- E2E tests for every user workflow (smoke tests cover launch; unit/integration tests cover logic)
