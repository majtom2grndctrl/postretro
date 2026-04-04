# Skaffa – Architecture Index - not for PostRetro

> **Use as a router:** pick 2–4 linked docs for the task, don't load everything.
> **Source of truth for:** product definition, process model, source layout, and where contracts live.
> **Not for:** deep schema details (load the specific contract doc instead).
> **Pre-stable note:** refactors may introduce breaking API changes; update all call sites and related tests in the same change (no compatibility shims by default).

## Agent Router (Task → Minimal Docs)

- **Engineering conventions / code style** → `docs/skaffa_development_guide.md`
- **Documentation writing style** → `docs/documentation_style_guide.md`
- **UI visual language / theme tokens** → `docs/design/visual-language.md`
- **Add/normalize UI components (shadcn)** → `.claude/skills/add-shadcn-component/SKILL.md`, `docs/design/visual-language.md`
- **Descriptor discovery / auto-instrumentation** → `docs/component_descriptor_discovery.md`, `docs/skaffa_component_registry_schema.md`
- **Content manipulation (inline editing, component insertion, element movement, picker)** → `docs/skaffa_content_manipulation.md`, `docs/skaffa_component_registry_schema.md`, `docs/skaffa_runtime_adapter_contract.md`
- **Inspector behavior & editing semantics** → `docs/skaffa_inspector_ux_semantics.md`, `docs/skaffa_component_registry_schema.md`, `docs/skaffa_override_model.md`
- **Override model (data + persistence)** → `docs/skaffa_override_model.md`, `docs/skaffa_preview_session_protocol.md`
- **Save to disk (validation + promotion)** → `docs/skaffa_save_to_disk_protocol.md`, `docs/skaffa_override_model.md`, `docs/skaffa_inspector_ux_semantics.md`
- **Sidecar process (file index, watching, stale detection)** → `docs/skaffa_sidecar_process.md`, `docs/skaffa_extension_api.md`, `docs/skaffa_save_to_disk_protocol.md`
- **Workspace file edits** → `docs/skaffa_project_edit_protocol.md`, `docs/skaffa_extension_api.md`
- **Preview sessions & selection flows** → `docs/skaffa_preview_session_protocol.md`, `docs/skaffa_ipc_boundaries_and_sequences.md`, `docs/skaffa_runtime_adapter_contract.md`
- **Runtime adapter implementation** → `docs/skaffa_runtime_adapter_contract.md`, `docs/skaffa_runtime_adapter_integration_guide.md`
- **Harness model (managed Vite previews)** → `docs/skaffa_harness_model.md`, `docs/skaffa_preview_session_protocol.md`, `docs/skaffa_project_configuration_skaffa_config.md`
- **Project configuration** → `docs/skaffa_project_configuration_skaffa_config.md`
- **User settings (per-user preferences)** → `docs/skaffa_project_configuration_skaffa_config.md` §13
- **Project setup** → `docs/skaffa_project_setup.md`, `docs/skaffa_project_configuration_skaffa_config.md`
- **Extension API / modules** → `docs/skaffa_extension_api.md`, `docs/skaffa_extension_authoring_guide.md`
- **Renderer state, stores, selectors** → `docs/skaffa_renderer_state_architecture.md`, `docs/skaffa_development_guide.md`
- **Cross-process IPC debugging** → `docs/skaffa_ipc_boundaries_and_sequences.md`, `docs/skaffa_development_guide.md`
- **MCP server/tooling** → `docs/skaffa_mcp_server_contract.md`
- **Agent integration (ACP, profiles, CLI)** → `docs/skaffa_agent_integration.md`, `docs/skaffa_mcp_server_contract.md`, `docs/skaffa_agent_integration_guide.md`
- **Data flow instrumentation (fetch, iteration, services, data-bound props)** → `docs/skaffa_data_flow_instrumentation.md`, `docs/skaffa_component_registry_schema.md`, `docs/skaffa_inspector_ux_semantics.md`
- **Telemetry (consent, events, extension API)** → `docs/tmp/telemetry.md`, `docs/skaffa_extension_api.md`
- **Dev environment / pitfalls** → `docs/skaffa_development_guide.md`

---

## Doc Index

**Architecture contracts:**

| Doc | Topic |
|-----|-------|
| [Content Manipulation](./skaffa_content_manipulation.md) | Inline editing, component insertion, element movement, picker |
| [Runtime Adapter Contract](./skaffa_runtime_adapter_contract.md) | Framework bridge: instance ID, click-to-select, override application |
| [Descriptor Discovery](./component_descriptor_discovery.md) | `*.skaffa.ts` auto-discovery for project components |
| [Component Registry Schema](./skaffa_component_registry_schema.md) | `ComponentDescriptor`, editability, control types |
| [Inspector UX Semantics](./skaffa_inspector_ux_semantics.md) | Editability states, affordances, override display |
| [Preview Session Protocol](./skaffa_preview_session_protocol.md) | Session lifecycle, attachment modes, Editor View |
| [Override Model](./skaffa_override_model.md) | Draft overrides, persistence, structural ops |
| [Save-to-Disk Protocol](./skaffa_save_to_disk_protocol.md) | Validation, promotion, file edit transactions |
| [Project Configuration](./skaffa_project_configuration_skaffa_config.md) | `skaffa.config.js` shape, extensions, modules, registry composition |
| [Project Edit Protocol](./skaffa_project_edit_protocol.md) | Workspace file edits |
| [Project Graph Schema](./skaffa_project_graph_schema.md) | Graph entities, patch protocol, query API |
| [Renderer State Architecture](./skaffa_renderer_state_architecture.md) | Zustand stores, sync manager, selectors |
| [IPC Boundaries](./skaffa_ipc_boundaries_and_sequences.md) | Process boundaries, channel schemas, sequence diagrams |
| [MCP Server Contract](./skaffa_mcp_server_contract.md) | Agent-facing API: resources, tools, validation |
| [Agent Integration](./skaffa_agent_integration.md) | ACP client, profiles, override batching |
| [Data Flow Instrumentation](./skaffa_data_flow_instrumentation.md) | Fetch, iteration, services, data-bound props |
| [Iteration Deck](./skaffa_iteration_deck_integration.md) | Deferred — variant preview sketch |

**Design language:**

| Doc | Topic |
|-----|-------|
| [Visual Language](./design/visual-language.md) | Layout, color tokens, theme system |

**Implementation guides:**

| Doc | Topic |
|-----|-------|
| [Extension Authoring Guide](./skaffa_extension_authoring_guide.md) | Building extension modules and packages |
| [Development Guide](./skaffa_development_guide.md) | Conventions, build pipeline, dev workflows |
| [Testing Guide](./testing_guide.md) | Test patterns, Vitest config, coverage |
| [Runtime Adapter Integration Guide](./skaffa_runtime_adapter_integration_guide.md) | Implementing a framework adapter |
| [Harness Model](./skaffa_harness_model.md) | Managed Vite preview servers |
| [Agent Integration Guide](./skaffa_agent_integration_guide.md) | Connecting agents, tool restriction, MCP setup |
| [Project Setup](./skaffa_project_setup.md) | Detection, config generation, extension manifests |
| [Documentation Style Guide](./documentation_style_guide.md) | Writing standards for architecture docs |
| [Code Review Checklist](./code_review_checklist.md) | Skaffa-specific antipatterns for code review |

---

## 1. Product Definition

Skaffa is an **Integrated Design Environment (IDE) for web-based software**.

Designers work directly with real, production web code through:
- Visual structure inspired by game engines (notably Godot)
- Instance-first UI editing
- Explicit, engineer-authored configuration and guardrails
- AI-assisted workflows that are constrained, inspectable, and reversible

Skaffa is not a code generator. It abstracts incidental complexity, preserves control over output code, and always provides an escape hatch to source. The UI surfaces a composable subset of structure and configuration; complex logic lives in code files.

### Essential Development Commands

Use `pnpm` for all commands from the repository root:

| Command | Purpose |
|---------|---------|
| `pnpm dev` | Start Skaffa (Vite dev server + Electron). Rebuilds modules only when sources changed. |
| `pnpm build` | Full production build (modules → Electron → renderer) |
| `pnpm demo:refresh` | Rebuild and re-vendor extensions for demo project |
| `pnpm typecheck` | Type-check all TypeScript (main, renderer, extensions) |
| `pnpm test` | Unit/component tests (Vitest) |
| `pnpm test:e2e` | E2E tests (Playwright) |

Targeted rebuilds: `pnpm build:modules` (shared types + workspace modules + packages), `pnpm build:electron`, `pnpm build:renderer`.

---

## 2. Architectural Principles

Five principles shape Skaffa's design. Each has a dedicated doc with full contracts.

| Principle | Invariant | Dedicated doc |
|-----------|-----------|---------------|
| **Instance-first editing** | Designers edit concrete rendered instances, not abstract component types | [Inspector UX Semantics](./skaffa_inspector_ux_semantics.md) |
| **Explicit editability** | Engineers declare what is editable via component descriptors. No inference. | [Component Registry Schema](./skaffa_component_registry_schema.md), [Descriptor Discovery](./component_descriptor_discovery.md) |
| **Modular + extensible** | Core ships minimal features. Registries, graph producers, and launchers are extension modules. | [Extension API](./skaffa_extension_api.md), [Extension Authoring Guide](./skaffa_extension_authoring_guide.md) |
| **Extension host isolation** | All module code runs in a dedicated process. Modules access capabilities only through a typed, versioned API. | [Extension API](./skaffa_extension_api.md), [IPC Boundaries](./skaffa_ipc_boundaries_and_sequences.md) |
| **AI as assistant** | AI changes flow through the override model (validated, draft, reversible). Agents never edit files directly. | [Agent Integration](./skaffa_agent_integration.md), [MCP Server Contract](./skaffa_mcp_server_contract.md) |

---

## 3. Process Model

Skaffa is a **multi-process Electron application**. Code lives where it runs — do not reach across process boundaries with imports. Code is organized by process boundary.

| Process | Role | Constraints |
|---------|------|-------------|
| **Main** | App lifecycle, project management, window orchestration, extension host supervision, override store, graph store | Authority for all state |
| **Renderer** | Workbench UI (Component Tree, Editor View, Inspector, Agent Panel), Launcher view | No direct Electron/Node access; effects through preload APIs |
| **Preload** | Typed capability gateway between renderer and main | Minimal surface, security enforcement |
| **Extension Host** | Executes module code, registers contributions (registries, adapters, graph producers) | Boxed — no core internals, capabilities via ExtensionContext API |
| **Sidecar** | Workspace file index, caching, incremental recomputation | Service owned by main; no project writes, not reachable from renderer |

Cross-process communication, channel schemas, and sequence diagrams: [IPC Boundaries](./skaffa_ipc_boundaries_and_sequences.md). Sidecar details: [Sidecar Process](./skaffa_sidecar_process.md).

### Source Code Organization

Code is organized by process boundary under `apps/electron/`:

```
apps/electron/
├── main/                    # Main process (Electron host)
│   ├── main.ts             # Entry point
│   ├── config/             # Config loading + validation
│   ├── settings/           # User settings (per-user preferences)
│   ├── extension-host/     # Extension host management
│   ├── sidecar/            # Sidecar process management
│   ├── graph/              # Project graph store
│   ├── overrides/          # Override store + persistence
│   ├── registry/           # Registry composition
│   ├── project/            # Project management
│   ├── project-setup/      # Detection, config generation, dep install
│   └── ipc/                # IPC handlers (validation + routing)
├── renderer/                # Renderer process (Workbench UI)
│   ├── main.tsx            # Entry point
│   ├── router.tsx          # TanStack Router config
│   ├── components/         # UI components (Inspector, Graph, etc.)
│   ├── state/              # Zustand stores
│   └── views/              # Top-level views (AppShell, Launcher, Workbench)
├── extension-host/          # Extension host process
│   ├── main.ts             # Entry point
│   ├── extension-context.ts # Extension API surface
│   └── module-loader.ts    # Module loading + activation
├── sidecar/                 # Project sidecar process
│   └── main.ts             # Entry point (ndjson over stdio)
├── preload/                 # Preload scripts (capability gateway)
│   └── preload.ts          # Main window preload
└── shared/                  # Cross-boundary protocol types
    ├── index.ts            # Root barrel re-exports from all subdirectories
    ├── types/              # Foundational types (common, config)
    ├── registry/           # Component registry, descriptors, discovery
    ├── overrides/          # Override model, save, runtime adapter
    ├── settings/           # User settings schemas
    ├── agent/              # Agent profiles, MCP schemas
    ├── graph/              # Project graph, project edits
    ├── preview-session/    # Preview sessions, agent IPC
    ├── project/            # Project types
    ├── inspector/          # Inspector section types
    ├── sidecar/            # Sidecar protocol types
    └── ipc/                # IPC channel schemas
```

Build output structure and dev workflows: [Development Guide](./skaffa_development_guide.md).

---

## 4. Renderer Tech Stack

### Core
- React, React DOM, TypeScript
- Vite + @vitejs/plugin-react

### Styling
- Tailwind CSS (Skaffa UI only), PostCSS, Autoprefixer
- shadcn/ui (BaseUI variant) — default for new UI components. Remap palette classes to Skaffa theme tokens in `apps/electron/renderer/styles.css`.

### State
- Zustand (single projection store). Renderer state is a read projection of main process authority. See [Renderer State Architecture](./skaffa_renderer_state_architecture.md).

### Routing, Tables, Virtualization
- @tanstack/react-router, @tanstack/virtual, @tanstack/table

### Forms & Async
- @tanstack/form (installed; Inspector currently uses `useState` for field state)
- @tanstack/react-query

### Validation
- Zod

### Devtools
- @tanstack/react-query-devtools, @tanstack/router-devtools, React DevTools

### Explicit Non-Goals
- No Radix UI
- No CSS-in-JS
- No global React Context as app state
- Renderer does not import Electron APIs

---

## 5. Code Style

Prefer clear over clever. Prefer accessible markup with semantic HTML. Follow [Development Guide](./skaffa_development_guide.md).

---

## 6. Deferred (Out of Scope)

- Engineer agent profile (file writes, terminal access) — design profile shipped in v1
- Public extension marketplace
- Untrusted module sandboxing UI
- Type-level component authoring UI
- Full runtime tree introspection
- Iteration Deck variant preview — see [sketch](./skaffa_iteration_deck_integration.md)

---

## 7. Guiding Principle

**Skaffa edits what it can prove is safe to edit, displays what it cannot, and always provides an escape hatch to code.**
