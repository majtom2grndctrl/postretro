# Project model guidance

Reviewed: 2026-09-29. Read when choosing a model or reasoning effort for a project skill or its workers.

These are project defaults inferred from official model guidance, not PostRetro benchmark results. Keep the lightest setting that meets acceptance and review quality.

## Task defaults

| Work | Model | Reasoning effort |
|---|---|---|
| Exact source lookup, prescribed mechanical edit, test execution and reporting | `gpt-6-luna` | `low` |
| Single-file repair with no contract changes; mechanical spec fixes; mechanical-only hygiene review | `gpt-6-luna` | `medium` |
| Bounded implementation, skill design, build coordination | `gpt-6.1-sol` | `medium` |
| Spec drafting, broad/anchor/implementability review, code review, cross-file repairs | `gpt-6.1-sol` | `high` |
| GPU/layout/persistence contracts, producer/consumer seams, deep review lenses | `gpt-6.1-sol` | `xhigh` |
| Unresolved architecture, conflicting evidence, subtle lifecycle invariants, repeated failure after a focused retry | `gpt-6-astra` | `high` or `xhigh` |

Use Luna only when the brief supplies the behavior and scope. Source lookup must report facts, not decide contracts. A small diff can still need Sol or Astra. Escalate when a worker discovers ambiguity or downstream consumers; higher effort alone does not replace the stronger model's judgment. Reserve `max` for an unusually hard, named question. Do not default whole panels to `max` or `ultra`.

## Dispatch and availability

- User model/effort choices take precedence. Recommendations do not change the active chat's model or authorize extra workers.
- Check the current tool's model and effort list before dispatch. API availability does not establish Codex account or worker availability. Use only supported identifiers and settings.
- Prefer `gpt-6.1-sol` over the previous `gpt-6-sol`. If unavailable, use available `gpt-6-sol`, then `gpt-5.6-sol`, at the same supported effort. If Luna is unavailable, use Sol at `low` or `medium`. If Astra is unavailable, use Sol at `xhigh` and report the substitution for consequential work. Never silently downgrade to Luna for a contract task.
- Explicit model/effort overrides require a fresh or limited-history worker (`fork_turns: "none"` or a positive turn count) with a self-contained brief. Full-history workers inherit the parent model/effort; omit overrides for them. Follow the actual dispatch tool's schema.
- Skills run in the current chat may recommend a setting, but cannot switch themselves. Apply role defaults when selecting a chat or dispatching a worker.

## Research basis

OpenAI describes [GPT-6.1 Sol](https://developers.openai.com/api/docs/models/gpt-6.1-sol) as near-Astra quality for complex coding at lower cost; [GPT-6 Luna](https://developers.openai.com/api/docs/models/gpt-6-luna) as efficient for focused work; and [GPT-6 Astra](https://developers.openai.com/api/docs/models/gpt-6-astra) as the most capable model for demanding work. [Model selection](https://developers.openai.com/api/docs/guides/model-selection) recommends choosing by task, quality, latency, and cost, then comparing on representative inputs.

All three API model pages support `low`, `medium`, `high`, `xhigh`, and `max`. Codex may expose additional settings; use its live list. This guidance avoids API pricing and context-limit assumptions for ChatGPT plan usage.

When new models arrive, re-open these official sources, check live worker availability, and revise this table and explicit dispatch defaults together. Retain older models only as availability fallbacks or when project evidence favors them.
