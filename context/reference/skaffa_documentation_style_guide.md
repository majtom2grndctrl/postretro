# Documentation Style Guide - for Skaffa, not PostRetro

Short sentences. Clear words. No waste.

## Principles

- **Direct and brief.** Active voice, short sentences, no filler. One idea per sentence. Fragments in lists. Drop articles where natural. See *Prose* below for examples.
- **Durable.** Docs describe what survives refactoring. If a sentence breaks when a file is renamed or a function is extracted, it belongs in a ticket or code comment — not a doc. See *Persistent vs. Ephemeral Content* below.
- **Seamless.** Every edit reads as if the doc was always written this way. Restructure surrounding text when new edits reduce the document's overall cohesion.

## Prose

**Direct and brief.**

**Before:**
```
The renderer is built using shadcn/ui components with custom design
tokens that are defined in the apps/electron/renderer/styles.css file.
```

**After:**
```
Renderer uses shadcn/ui components. Theme tokens in apps/electron/renderer/styles.css.
```

**Before:**
```
The graphStore contains routes, components, and instance information
```

**After:**
```
graphStore: routes, components, instances
```

**Before:**
```
The renderer uses Zustand for state management. There are several
main stores:

- graphStore: This store manages the project graph including routes,
  components, and instances that are tracked during the session.
- inspectorStore: This store is responsible for managing inspector-related
  state such as the selected instance and property overrides that
  the user creates.
```

**After:**
```
- graphStore (Zustand): routes, components, instances.
- inspectorStore (Zustand): selection, overrides.
```

**Before:**
```
What is the IPC Protocol
```

**After:**
```
IPC Protocol
```

Headers are short nouns, not questions.

## Code and Paths

Name boundary files and entry points. Describe contract semantics and invariants in prose. Schema shapes live in code — don't reproduce them.

**Point to boundary files:**
```
Override store: apps/electron/main/overrides/override-store.ts
Runtime adapter entry: packages/react-runtime-adapter/src/index.ts
```

**Describe behavior in prose:**
```
IPC handler validates payload with Zod, routes to the override store,
returns updated snapshot.
```

**When to include code samples:**

- Specs for unbuilt features (mark `// Proposed design`) — remove after implementation
- Convention patterns only when a file reference alone wouldn't convey the pattern

After implementation, point to real code. A stale type signature in a doc creates mismatches at the worst possible layer. Describe the contract's semantics in prose; reference the source file for the shape.

Internal module types belong in code comments, not docs.

## Persistent vs. Ephemeral Content

Docs describe what survives refactoring. Tickets describe what to change right now.

**Litmus test:** "If we rewrote this module in a different framework, would this sentence still be true?" Yes → docs. No → ticket or code comment.

**Belongs in architecture docs (persistent):**
- Design principles and intent
- Cross-system relationships and boundaries
- Pipeline topology as semantic stages (not function names)
- Lifecycle ordering constraints and why alternatives fail
- Data contracts at system boundaries
- Architectural invariants

**Belongs in tickets (ephemeral):**
- Function, method, variable, and type names internal to a module
- Line numbers and exact file locations
- Specific algorithms and implementation choices
- "What to change" instructions with code snippets
- Error messages and log output strings

Function-level detail helps implementers start fast. Put it in tickets — consumed once and closed. In docs, it becomes maintenance debt.

**Example — pipeline stage description:**

**Before** (names a function, an API call, and a component — breaks when any name changes):
```
Stage 3: `__skaffaRepeater()` calls `data.map(callback)`, computes
`Object.keys(data[0])`, wraps result in `<SkaffaRepeaterBoundary>`.
```

**After** (describes the stage's role and contract — breaks only when the architecture changes):
```
Stage 3: Runtime boundary. Wraps iteration output in a boundary
component carrying metadata and inferred data keys. Adapter detects
the boundary in the fiber tree during instance registration.
```

## Document Structure

**Orientation block.** Start each doc with: when to read this, key invariant, related docs. Lets any reader decide in seconds whether to keep reading. See `docs/component_descriptor_discovery.md` for the pattern.

**Tables over prose** for mappings, field definitions, option lists. A table with columns `Field | Type | Default | Description` beats four paragraphs.

**Explicit non-goals.** State what the doc (or feature) does not cover. See *Spec Completeness* below.

**Terminology tables** when renaming or introducing terms. `Old | New` with concrete identifiers.

## Spec Completeness

Define what you're defining, completely. A spec with scattered "TBD" markers pushes ambiguity to the implementer — who has less context than the spec author.

**In-scope items get full coverage.** Edge cases, error states, constraints. If a behavior is worth specifying, specify it completely. If you can't fully specify something yet, move it to non-goals — don't leave a half-defined section.

**Out-of-scope items get a non-goals section.** One list, one place. Not "TBD" annotations sprinkled through the doc.

| | Before | After |
|---|---|---|
| **In scope** | "Error handling TBD" | "Empty stack returns `StackEmpty` error, Inspector shows inline message" |
| **Out of scope** | "Undo across sessions (future)" buried in the undo section | Non-goals: "Undo across sessions" |

**The spec is where scope decisions happen.** Implementation inherits those decisions. If a spec is vague, the implementer guesses — and guesses compound.

## Density vs. Clarity

Some topics need more words: security constraints, cross-process boundaries, non-obvious behavior, setup with prerequisites. Add detail for these — but keep the style direct.

**Before:**
```
The extension host architecture runs modules in a separate process,
which means that extension modules should never import code from
the apps/electron/main/ or apps/electron/renderer/ directories
because they need to access capabilities through the ExtensionContext API.
```

**After:**
```
Extension host runs in separate process — modules cannot import from
apps/electron/main/ or apps/electron/renderer/. All capabilities
accessed via ExtensionContext API. Violating this causes silent
failures: the import resolves at build time but the module has no
access to the main process context at runtime.
```

The "after" is longer. That's fine — the cross-process constraint and its failure mode earn the extra sentence. Brevity means no wasted words, not fewest words.
