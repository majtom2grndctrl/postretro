---
name: create-skill
description: >
  Creates a new Claude Code skill for this project. Guides the user through
  designing the skill's purpose, trigger conditions, structure, and content.
  Use when the user wants to add a new slash command or automated workflow.
disable-model-invocation: true
argument-hint: "[skill-name]"
---

# Create Skill

Design a new Claude Code skill. Output: `.claude/skills/<name>/SKILL.md`.

## Existing skills

!`ls -1 .claude/skills/ 2>/dev/null`

## Process

### 1. Gather requirements

If name and description are clear, proceed. Otherwise ask focused questions — don't interrogate:

- **Purpose:** What problem does it solve?
- **Trigger:** User-invoked (`disable-model-invocation: true`) or auto-activated?
- **Scope:** Single-step task or multi-step workflow?
- **Side effects:** Modifies files, runs commands, pushes to git, calls external services?
- **Isolation:** Main conversation or forked subagent (`context: fork`)?

### 2. Design the skill

Based on requirements, decide on:

**Frontmatter fields:**
- `name` — lowercase, hyphens, max 64 chars
- `description` — third-person voice. Two parts: WHAT it does + WHEN to use it. Front-load the key use case (truncated at 250 chars in listings). Include specific trigger keywords from real workflows.
- `disable-model-invocation` — `true` for anything with side effects or that the user should explicitly choose to run
- `context` — `fork` if the skill should run in an isolated subagent (research, review, batch operations)
- `agent` — subagent type when using `context: fork` (e.g., `Explore`, `Plan`, `general-purpose`)
- `allowed-tools` — pre-approve tools to avoid permission prompts mid-skill
- `argument-hint` — autocomplete hint like `[file-path]` or `[issue-id]`
- `hooks` — skill-scoped hooks that only fire while the skill is active

**Content principles:**
- Keep SKILL.md under 500 lines. Move reference material to separate files.
- Use concrete examples over abstract rules. Show input/output pairs.
- Only include context Claude doesn't already have. Challenge every line: "Does Claude really need this?"
- Use `` !`command` `` syntax to inject dynamic context (e.g., `` !`git status` ``).
- Default to atomic (one skill, one job). Only orchestrate when the workflow genuinely requires coordination.

### 3. Write the skill

Create the skill directory and `SKILL.md`. Reference material, templates, or scripts go in separate files within the skill directory.

### 4. Verify

After writing, confirm:
- [ ] Description is specific, third-person, includes trigger keywords
- [ ] Frontmatter fields match the skill's needs
- [ ] Content is concise — under 500 lines
- [ ] Side-effect skills have `disable-model-invocation: true`
- [ ] Dynamic context uses `` !`command` `` where live data is needed
