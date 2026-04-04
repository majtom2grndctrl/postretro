# Skaffa Development Guide - for Skaffa, not PostRetro

> **Read this when:** setting up the repo, adding a feature, or onboarding. Covers dev setup, conventions, and coding standards.
> **Key invariant:** code lives where it runs — respect process boundaries, validate at every crossing.
> **Related:** [Architecture Index](./index.md) · [IPC Boundaries](./skaffa_ipc_boundaries_and_sequences.md) · [Testing Guide](./testing_guide.md) · [Documentation Style Guide](./documentation_style_guide.md)

---

## Agent TL;DR

- Optimize for **readability over cleverness**; prefer small, explicit changes.
- Respect **process boundaries**: renderer → preload → main → (extension host / preview runtime).
- Put **shared types + Zod schemas** in `apps/electron/shared/` and validate at every boundary.
- Renderer never imports Electron/Node; main owns privileged capabilities and project writes.
- In renderer: single projection store (Zustand) + selectors + commands. Feature code never touches IPC directly. See [Renderer State Architecture](./skaffa_renderer_state_architecture.md).
- **Deliver the impact defined in docs and tickets.** Specs define what and why; use judgment on how. When the plan doesn't survive contact with the code, adapt — but surface deviations and update the docs. See §1.

---

## 1) Implementation Quality

Docs and tickets define the **impact** to deliver — the *what* and *why*. The recommended approach is the starting plan, not a mandate. Deliver the intended impact cleanly; use judgment when the plan doesn't survive contact with the code.

### 1.1 Deliver the impact

Read the spec and ticket before writing code. They tell you what outcome matters and what constraints apply. Start with the recommended approach, but treat it as a plan, not a contract.

**Do:**
- Handle the error states and edge cases that fall within the defined scope.
- Write tests now, while you have full context on what the code should do.
- When the approach works as specced, deliver it without embellishment.

**Don't:**
- Add capabilities the ticket didn't ask for ("while I'm here, I'll also add...").
- Skip work that's clearly within scope and justify it with "TODO" or version labels.
- Invent abstractions, helpers, or config options for hypothetical future needs.

### 1.2 When the plan doesn't work

Sometimes the spec's approach hits a wall — type conflict, circular dependency, invariant violation. The response depends on scale:

**Small adjustment** (same impact, minor approach change):
1. Ask the user: explain what you found and propose the alternative.
2. On confirmation, implement the alternative.
3. Update the spec/docs to reflect the actual approach taken.

**Significant change** (different contracts, shifted scope, new trade-offs):
1. Stop and surface the issue to the user with enough detail to decide.
2. Propose options with trade-offs.
3. On resolution, update docs/specs *before* resuming implementation.

The key principle: **specs are working documents — update them during implementation, never silently deviate.** After the feature ships, durable knowledge lives in architecture docs and code comments; the spec is consumed and removed. See §1.5.

### 1.3 Clean, not clever

Over-engineering is as costly as under-delivering. Both create surface area that has to be understood, tested, and maintained.

| | Under-delivering | Over-engineering |
|---|---|---|
| **What** | Shipping scope with missing validation, error handling, or tests | Adding scope, abstractions, or infrastructure the ticket didn't request |
| **Cost** | Broken states, follow-up tickets, lost context | Unnecessary complexity, harder reviews, maintenance burden |
| **Example** | "Undo works but doesn't handle empty stack" | "Added a generic undo framework with plugin hooks" |

### 1.4 When to file a follow-up instead

Adjacent work discovered during implementation gets a follow-up ticket, not a scope expansion.

- **Robustness gaps** outside the ticket's scope → file a ticket with `[Robustness]` prefix.
- **Optimizations** with no current performance problem → file a ticket, don't cache preemptively.
- **Abstractions** without multiple concrete consumers → three similar lines beat a premature helper.

When you defer, create a ticket with enough context for the next agent. Never leave a bare `// TODO: fix later`.

### 1.5 Documentation lifecycle

Specs are working documents — they align on what to build and why, then get consumed during implementation.

**Where specs live:**

- **Default: Beads ticket.** Spec content lives in the parent ticket alongside orchestration guidance (child task breakdown, dependencies, sequencing). One focused space for everything about the epic. Tickets include acceptance criteria — the conditions that must hold for the work to be complete. Vague "done" conditions push ambiguity to the implementer.
- **Escape hatch: `docs/tmp/`.** When the spec outgrows ticket format — complex schemas needing cross-referencing, extensive tables, content multiple implementers need open simultaneously — extract to a temporary doc. The parent ticket keeps orchestration and links to the temp doc. Use judgment on when to extract.

**After the feature ships:**

- **Delete the spec.** Temp docs in `docs/tmp/` are removed when the epic closes. Ticket specs close naturally with the ticket.
- **Architecture docs** (`docs/`) capture what's durable — design principles, process boundaries, contracts, pipeline topology. Content that survives a rewrite.
- **Code comments** capture implementation-level "why" decisions. Rationale a reader can't derive from the code alone. See §9.

**What doesn't belong in `docs/`:**

- Specs for specific features or epics (use tickets or `docs/tmp/`)
- Implementation plans or task breakdowns (use tickets)
- Content that names specific functions, types, or file paths as load-bearing detail (see [Documentation Style Guide](./documentation_style_guide.md) §Persistent vs. Ephemeral Content)

---

## 2) Development Setup

### Initial Setup

After cloning, compile shared types before your first dev session:

```bash
pnpm build:modules
```

This compiles shared types → `.js` and builds all downstream packages. The compiled `.js` and `.d.ts` files in `shared/` are gitignored build artifacts — `pnpm build:modules` regenerates them.

### Running the App

```bash
# Start dev server (Vite + Electron)
# Checks staleness — only rebuilds modules when source files have changed.
pnpm dev

# Build production bundles (modules → Electron → renderer)
pnpm build

# Targeted rebuilds (for focused iteration):
pnpm build:modules    # Shared types, workspace modules, packages
pnpm build:electron   # Electron main/preload/extension-host
pnpm build:renderer   # Renderer (Vite)
```

### Skipping Auto-Restore

By default, Skaffa restores the last opened project on startup. For development or testing the launcher view, you can skip this behavior:

```bash
# Skip restoring the last project (show launcher)
pnpm dev -- --no-restore
```

### Running the Demo Project

`apps/demos/shadcn/` simulates a standalone project outside the Skaffa repo. The frontend uses `--ignore-workspace` with its own lockfile. Dependencies (extension modules, `@skaffa/config`) are installed from local tarballs in `vendor/` to keep the project portable.

Before running the demo (or after extension changes), rebuild and re-vendor:

```bash
pnpm demo:refresh
```

Builds all modules, packs extension modules + `@skaffa/config` + `@skaffa/layout-primitives-react` into `vendor/`, then installs both demo workspaces.

`pnpm dev` will also warn at startup if the demo vendor packages are stale.

To run the demo, start Skaffa with `pnpm dev` and open the demo project. The Vite launcher extension manages the demo app's dev server automatically.

Skaffa core does not auto-start framework dev servers — dev-server startup is owned by toolchain-specific **preview launcher extensions**. Starting an `app` preview session starts the relevant dev server.

### Config Package (@skaffa/config)

`skaffa.config.js` is validated by shared Zod schemas in `apps/electron/shared/types/`. A type shim is auto-generated as part of `pnpm build:modules`.

`packages/config` copies the compiled config module and types into its `dist/` so external projects can import `defineSkaffaConfig` via `@skaffa/config` without repo-relative paths.

### Hot Reload Behavior

| Process | Reload | Notes |
|---------|--------|-------|
| Renderer | Instant (Vite HMR) | |
| Main | Restart required | Kill and re-run `pnpm dev` |
| Extension host | Restart required | Kill and re-run `pnpm dev` |
| Preload | Restart required | |

---

## 3) Build System Architecture

### Extension Module Build Process

Skaffa bundles project-local extension modules in place:

- Entry discovery: `extensions/*/module/index.{ts,js}`
- SDK and extension modules bundled in place (`.ts` → `.js`)
- Bundling runs via:

```bash
pnpm build:modules
```

Notes:
- `build:modules` runs the full pipeline: shared types → workspace modules → extension SDK types → packages.
- `.ts` entrypoints output to `.js` in the same folder.
- `.js` entrypoints are treated as build artifacts; running the build will overwrite them.
- A `.build-modules-stamp` file is written on success; `pnpm dev` uses this to skip rebuilds when sources haven't changed.

### Output Structure

Build produces:

```
dist/
├── main/
│   └── main.js              # Electron main process (ESM)
├── preload/
│   └── preload.js           # Main window preload (CommonJS)
├── runtime-transport-preload/
│   └── runtime-transport-preload.js  # Preview preload (CommonJS)
├── extension-host/
│   └── main.js              # Extension host process (ESM)
├── sidecar/
│   └── main.js              # Project sidecar process (ESM)
└── renderer/
    ├── index.html
    └── assets/
        ├── index-[hash].js  # Renderer bundle (ESM)
        └── index-[hash].css
```

---

## 4) File & Directory Organization

**Rule: split by responsibility, not by line count.** A file earns a split when it serves distinct jobs. Line count alone is not a trigger.

### 4.1 File size guidance

- **~400–500 lines** (source, non-test): yellow flag. Consider splitting on next significant addition.
- **~600+ lines**: split before adding more code.
- **Test files**: exempt. Test suites are flat and linear; large is fine.

Existing files above these thresholds are not immediate refactoring targets. Apply when adding significant new code.

### 4.2 Valid seams for splitting

Split along natural boundaries:

1. **Responsibility** — file serves two distinct jobs. Extract each into its own module.
2. **Consumer** — different importers use different subsets of exports. Each subset becomes a module.
3. **Change frequency** — stable plumbing vs. actively-evolving logic. Separate to reduce churn.

### 4.3 Splits to avoid

- Arbitrary line-count splits with no conceptual boundary.
- No grab-bag `utils.ts` / `helpers.ts` files. Co-locate single-caller helpers with their caller.
- Separating types from implementation. Co-locate schemas + types with the code that uses them (exception: `shared/` cross-boundary contracts).

### 4.4 Directory density

- **Uniform directories** (all UI components, all test fixtures): flat-and-many is fine. 20+ files OK.
- **Mixed-concern directories**: introduce subdirectories when you can't tell at a glance which files relate to each other.
- Subdirectories should have barrel `index.ts` re-exports when they have multiple external consumers (other packages, process boundaries, or sibling feature directories).

### 4.5 When to split

- **Proactively**: when adding significant new functionality to an already-large file. You have full context; the split is cheapest now.
- **Not retroactively** just to meet a number. Only when the file actively causes pain (hard to navigate, merge conflicts, too many responsibilities).
- **Never during a bugfix.** Don't mix structural refactoring with behavior changes in one changeset.

---

## 5) TypeScript Conventions

### 5.1 Readability-first

- Prefer **plain TypeScript** over clever type gymnastics.
- Prefer **descriptive names** and early returns over deeply nested logic.
- Keep functions small; extract helpers only when it reduces duplication.

### 5.2 Types at boundaries

- Treat boundary inputs as `unknown`, then **parse/validate with Zod**. (See [Testing Guide](./testing_guide.md) for what to test at these boundaries.)
- Avoid `any`. If you must temporarily, isolate it and add a follow-up ticket.
- Prefer discriminated unions (`type`/`kind`) over "stringly typed" branching.

### 5.3 "Schema + type" co-location

When a shape crosses a boundary, define:
- `XxxSchema` (Zod)
- `type Xxx = z.infer<typeof XxxSchema>`

Keep them in the appropriate `apps/electron/shared/<domain>/` subdirectory and re-export through its barrel `index.ts`. The root `apps/electron/shared/index.ts` re-exports all subdirectory barrels.

---

## 6) Electron Constraints

### 6.1 Window security defaults (non-negotiable)

- `contextIsolation: true`
- `nodeIntegration: false`
- Do not use Electron `webviewTag` for Skaffa UI.
- Prefer host-owned policies over "trusting the guest".

### 6.2 Preload module format

Preloads must build as **CommonJS** (`format: 'cjs'`). Hard Electron constraint — ESM preloads fail silently.

**Symptoms if wrong:** `SyntaxError: Cannot use import statement outside a module`, preload fails to load, `window.skaffa` is undefined in renderer.

### 6.3 WebContentsView embedding rules

- The preview `WebContentsView` must be constrained to a **renderer-owned viewport rectangle**.
- Renderer computes viewport bounds; main applies them to the `WebContentsView`.
- Never assume "full window" bounds; the Workbench has docked panels.
- Treat resize/layout changes as first-class (no flicker, no overlaying panels).

### 6.4 Navigation + new-window policy (preview runtimes)

- Never allow guest content to spawn new Electron windows.
- Use `setWindowOpenHandler` (or equivalent) to **deny** or **redirect to system browser**.
- Decide navigation policy explicitly:
  - in Editor View, clicks are consumed (so app interaction doesn't navigate)
  - in Preview Mode (deferred), allow in-app navigation; gate inspect behind a modifier gesture
  - optionally restrict cross-origin navigation (future hardening)

---

## 7) Renderer Conventions (React Workbench)

### 7.1 TanStack Router

Layout routes render children using `<Outlet />` (not `{children}`). Keep route/layout components thin; prefer composition via components + stores.

**Correct:**
```tsx
import { Outlet } from '@tanstack/react-router';

export const AppShell = () => {
  return (
    <div>
      <header>...</header>
      <main>
        <Outlet />  {/* ← Renders matched child route */}
      </main>
    </div>
  );
};
```

**Incorrect:**
```tsx
export const AppShell = ({ children }: PropsWithChildren) => {
  return <main>{children}</main>;  // ✗ Will not render routes
};
```

### 7.2 UI + theming

- Prefer shadcn/ui primitives (BaseUI variant).
- Do not use raw gray palette classes in components; use semantic tokens from `apps/electron/renderer/styles.css`:
  - `bg-surface-*`, `text-fg*`, `border-*`, `ring-focus`, etc.
- Follow [Visual Language](./design/visual-language.md).

**Adding shadcn/ui components to Skaffa's UI:** use `/add-shadcn-component <component...>`. The skill installs the component and automatically normalizes shadcn palette classes to Skaffa theme tokens. This applies to Skaffa's own renderer UI — not to project components built for use within the editor.

### 7.3 Behavioral state

Drive UI behavior with data attributes or ARIA attributes, not dynamic CSS classes.

| Mechanism | Use for |
|-----------|---------|
| ARIA attributes | Semantic state with assistive technology meaning (`aria-expanded`, `aria-disabled`, `aria-selected`) |
| `data-*` attributes | Component state visible to tests and selectors (`data-state="expanded"`, `data-empty`) |
| CSS classes | Appearance only. Token classes are static; never toggle them to signal state. |

Prefer ARIA where a semantic equivalent exists. When behavior is tied to ARIA, a broken assistive technology experience is equally broken for sighted users — increasing the odds it gets fixed.

CSS can select on both (`[aria-expanded="true"]`, `[data-state="open"]`). Tailwind supports these via variant prefixes (`aria-expanded:`, `data-[state=open]:`). Appearance follows state — not the other way around.

---

## 8) Logging, Errors, and Debuggability

### 8.1 Logging rules

- Prefix logs with a subsystem tag: `[PreviewSession]`, `[IPC]`, `[ProjectionStore]`, etc.
- Log actionable failures once at the boundary; avoid spamming logs in hot paths.
- Prefer structured errors for user-facing failures (code + message + details).
- Make "happy path" easy to follow in code; keep failure branches explicit.

### 8.2 Electron DevTools

Open DevTools in the main window:
- macOS: `Cmd+Option+I`
- Windows/Linux: `Ctrl+Shift+I`
- Menu: View → Toggle Developer Tools

### 8.3 Where extension logs appear

`console.log()` from extension modules typically appears in the **Electron DevTools console** (main window), not in the terminal where you launched `pnpm dev`.

If `skaffa.config.js` validation fails or a module fails to load, start by checking the same DevTools console output for errors.

### 8.4 Console subsystem tags

Logs are prefixed with subsystem tags to identify the source process and module. Common tags include `[ExtHostManager]`, `[ExtHost]`, `[GraphStore]`, `[IPC]`, `[PreviewSession]`. Filter by tag to isolate traffic during debugging.

---

## 9) Code Comments

### 9.1 Comments that earn their keep

- **Why, not what.** Explain rationale. The code shows behavior.
- **Non-obvious context.** Why a capability lives in this process, ordering dependencies, Electron quirks, perf-sensitive paths. Things a reader can't derive from the code alone.
- **Spec pointers.** Brief link to the governing doc or contract when code implements a specific spec. File path or doc name — not an inline summary.

### 9.2 File headers

File headers orient a reader, not educate them. Two lines: what this file owns, then which doc governs it.

**Good:**
```
// Drag lifecycle detection and drop resolution for the preview runtime.
// See: docs/skaffa_content_manipulation.md §2.4, §8.2
```

**Too much:**
```
// The adapter owns the full drag lifecycle: pointer handling, hit testing,
// visual feedback, and drop resolution all run locally in the guest
// WebContents. This hook is a thin layer that:
//   1. Listens for adapter lifecycle events (dragInitiated, dragEnded) ...
//   2. Listens for runtime.dropResolved and dispatches ...
//
// State machine: idle → dragging → idle
//   - idle: no drag in progress
//   - dragging: actively dragging (visual feedback is rendered by ...
```

The header's job is to tell you *which doc to load*, not to summarize that doc. Behavioral descriptions, state machines, and architectural context belong in the spec.

### 9.3 Comments to avoid

- **Restating code.** If the code is unclear, improve the code — don't narrate it.
- **Changelog annotations.** `// Added in PR #42`, `// Refactored from old approach`. Git handles provenance.
- **Orphan TODOs.** `// TODO: fix later` without a ticket or actionable context. File a ticket and reference it, or fix it now.
- **Duplicating docs.** Don't restate architecture docs in comments. Two sources of truth, both eventually wrong.

### 9.4 Revising on encounter

When you find a misleading, stale, or code-restating comment in a file you're already changing:

- Fix or remove it in the same changeset. A missing comment beats a lying one.
- Scope to code you're touching. Don't sweep unrelated files for comment cleanup.

---

## 10) Testing

For detailed patterns, test strategy, and commands, see [Testing Guide](./testing_guide.md).
