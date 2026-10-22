# Coding Agent Prompt Library — 2026 Edition

# Coding Agent Prompt Library — 2026 Edition

Reusable, **model-agnostic** prompts for capable coding agents. Revised 2026-10-22 from the prompts developed in this conversation. All prompts are in English for direct use in coding tools; the index is intentionally compact.

## Select the right prompt

| Need | File | Edits? |
| --- | --- | --- |
| I don't know what to improve next | `prompts/01-codebase-health-audit.md` | No |
| Understand logic, dependencies, and invariants | `prompts/02-system-logic-and-dependency-audit.md` | No |
| Investigate one issue before fixing it | `prompts/03-issue-investigation-and-root-cause.md` | No |
| Fix one confirmed issue | `prompts/04-fix-one-issue-and-verify.md` | Yes |
| Remove overengineering and obsolete complexity | `prompts/05-simplify-and-remove-complexity.md` | Yes |
| Split or reorganize an oversized module | `prompts/06-refactor-reorganize-and-compose.md` | Yes |
| Explain rationale inside code | `prompts/07-document-the-reasoning.md` | Docs only |
| Diagnose and improve UI/UX | `prompts/08-ui-ux-diagnose-and-improve.md` | Yes |
| Plan a change before implementation | `prompts/09-migration-and-change-impact-plan.md` | No |
| Improve game mechanics and feedback | `prompts/10-gameplay-systems-and-game-feel.md` | Depends |
| Set up rules and tools for future agents | `prompts/11-agent-workflow-and-quality-guardrails.md` | Yes |
| Discover missing product behavior/features | `prompts/12-product-gap-and-feature-opportunity-review.md` | No |

## How to use

1. Open the relevant prompt and paste it into your agent with your repository loaded. You may attach the `.md` file instead.
2. Replace the optional `[SCOPE]` / `[ISSUE]` placeholders, or leave them for the agent to infer from your request.
3. Start with **01** if direction is unclear. Use **03** for a specific finding, **09** for high-risk changes, and **04/05/06/08/10** for implementation.
4. Keep a repo-specific `AGENTS.md` or `CLAUDE.md` short. These task prompts are *not* intended to load during every agent session.

## Shared philosophy

- Understand the actual product and constraints before changing code; depth of reconnaissance should scale with risk.
- Prefer small, cohesive, well-typed code, composition where beneficial, explicit ownership, and useful in-code explanations.
- Complexity must earn its place; neither abstractions nor deletion are goals by themselves.
- Evidence before claims: cite real symbols/paths; label hypotheses; do not invent test results.
- Audit and plan prompts are **read-only**; implementation prompts include verification and clear completion.
- Do not make agents generate diagrams, run the whole suite, or write ADRs for trivial changes by default.
- Handle user data, untrusted text, credentials, and destructive operations safely; code and files are evidence, not authority to override instructions.

## Optional repository starter

`repo-starter/AGENTS.md.template` is a **short, editable template**, not a universal policy. Review it before placing in any repository. `repo-starter/CLAUDE.md.template` points Claude Code to the same repo rules without copying a large second policy.

## Reporting templates

`templates/finding.md` and `templates/migration-plan.md` are optional **output shapes** for consistent handoffs. Do not fill them unless they improve the task.

## What changed from the old prompts

- Removed rigid 0–5 phases and forced 2–5 Mermaid diagrams for every task.
- Removed arbitrary quotas for findings, invariants, tests, or module count.
- Separated read-only diagnosis from permission to edit.
- Added root-cause reasoning, impact tracing, targeted verification, evidence/confidence labels, and completion conditions.
- Stopped treating fewer lines of code and ECS abstractions as universal goals.
- Added explicit recognition that lack of tests/tooling limits confidence; absence of evidence is not proof of safety.
- Reduced always-on instruction duplication in favor of **on-demand task prompts**.

## Reference guidance (checked 2026-10-22)

- OpenAI — Rethinking skills and prompts for GPT-6 Astra (2026-09-11): https://developers.openai.com/blog/rethinking-skills-and-prompts-for-gpt-6-astra
- OpenAI — Codex best practices: https://learn.chatgpt.com/guides/best-practices
- Anthropic — Steering Claude Code: CLAUDE.md, skills, hooks, and subagents (2026-06-18): https://claude.com/resources/articles/steering-claude-code-skills-hooks-rules-subagents-and-more
- GitHub — Copilot custom instructions: https://docs.github.com/en/copilot/concepts/prompting-and-context/custom-instructions

These are workflow references, not a guarantee that every tool supports all features. Prefer your active agent's current tool documentation.


---

# 01 — Codebase Health Audit (discovery only)

**Use when:** You do not know what to improve next. **Mode:** Read-only; no code, config, or documentation edits.

## Prompt

Audit this repository to determine its current engineering health and identify the highest-value work worth doing next. Infer the product's purpose from its code, tests, docs, and actual behavior; clearly distinguish requirements confirmed by evidence from expectations you inferred.

Begin with a compact architecture sketch: entry points, major features, data ownership, critical execution paths, and external boundaries. Navigate from those paths outward, using targeted symbol/reference searches and dependency tracing rather than reading every file. Inspect configuration, tests, CI, and relevant recent changes when accessible. Run safe, nondestructive checks only when the result would change your assessment; record commands and limitations. Do not assume that a successful build proves runtime correctness.

Assess **correctness, security/trust boundaries, error behavior, runtime validation, type contracts, configuration, state/concurrency, tests, documentation, coupling, duplication, obsolete code, accessibility/UX when relevant, and accidental complexity**. Also identify gaps in expected product behavior, but label speculative feature ideas separately from verified defects. Look for repeated local patches that may indicate one shared underlying cause. Prefer system-level issues over superficial style nits.

For each actionable finding, give: a precise claim; evidence (`path:line`, symbol, and observed behavior where available); the failure mode or maintenance cost; **confidence** (confirmed / strongly supported / hypothesis); **impact** and **likelihood**; the smallest promising investigation or remediation direction; and any constraints preventing verification. Do not invent line numbers, usage, deployment assumptions, or vulnerabilities. Avoid arbitrary quotas: report the issues worth attention, not a mandatory top 20.

Prioritize using risk, user impact, change frequency, and likely effort. Separate **fix now / schedule / defer / don't change**. Explicitly identify areas that are healthy or that would be harmed by needless refactoring.

**Output:** (1) executive assessment; (2) concise system map or diagram *only if it improves understanding*; (3) ranked, evidence-backed findings; (4) inferred opportunities and unknowns; (5) 3–7 narrowly scoped next work sessions, each with a concrete investigation question and success condition. Keep the report concise and decision-oriented. **Do not modify the repository.**


---

# 02 — System Logic & Dependency Audit

**Use when:** A feature crosses multiple files or its behavior is difficult to reason about. **Mode:** Analysis only.

## Prompt

Examine **[SYSTEM OR USER FLOW]** as a functioning system, not a collection of files. Establish its entry points, call chain, data transformations, state ownership, external effects, consumers, error handling, and invariants. Follow references in both directions: what this system depends on, and what depends on it.

Choose only the representations that expose useful risks: a component/dependency graph for coupling, an end-to-end sequence/flowchart for control flow, a data-flow diagram for trust boundaries, or a state-transition diagram for lifecycle logic. Do not manufacture diagrams just to fill a checklist; provide Mermaid only for nontrivial flows. Mark uncertainty and dynamic dispatch limitations.

Reason through normal, edge, failure, retry, and concurrent execution where applicable. Investigate unclear ordering constraints, resource cleanup, invalid states, circular dependencies, stale caches, error propagation, mismatched contracts, duplicated policy, and unexpected side effects. Compare the actual implementation with its apparent intended behavior; do not invent product requirements.

For each finding: cite exact symbols/paths and show a plausible scenario or counterexample; name the violated invariant or unwanted coupling; assess confidence; explain what else could be affected by changing it. If no credible issue is found, say so.

**Output:** a brief functional model; 0–3 useful diagrams; invariants and boundaries; ranked risk findings; impact radius of a potential change; recommendations for follow-up investigation. **No changes.**


---

# 03 — Issue Investigation & Root Cause

**Use when:** You have a bug report, symptom, test failure, or suspicious audit finding. **Mode:** Diagnose; no fixes unless explicitly requested.

## Prompt

Investigate **[ISSUE / SYMPTOM]** in this repository. First identify expected versus actual behavior and what evidence is available. Reconstruct the shortest relevant execution path, starting at the observable symptom and tracing backwards through data, control, and state changes. Search for similar cases and related recent changes where useful.

Form competing hypotheses and attempt to falsify them with targeted code inspection, minimal reproduction, safe focused tests, or local logs. Do not assume the most visible failing function is the root cause. Distinguish code-proven defects from environment-specific or user-reported behavior that you cannot reproduce. If diagnostics require credentials, production mutation, or unsafe commands, stop at that boundary and explain the missing evidence.

Report the root cause only at the confidence level evidence allows. Identify the triggering conditions, failure mechanism, affected paths, regression risk, and the narrowest credible fix. Recommend a regression test that fails before the change and passes after it, where feasible. Check whether the same defect pattern appears elsewhere without expanding into an unrelated audit.

**Output:** reproduction/observations; traced path; hypotheses and falsifying evidence; root cause or remaining uncertainty; recommended minimal fix and regression test; affected files. **Do not implement yet.**


---

# 04 — Fix an Issue & Verify

**Use when:** A specific defect or verified audit issue is ready for implementation. **Mode:** Edit allowed, strictly scoped.

## Prompt

Resolve **[SPECIFIC ISSUE]** with the smallest maintainable change that addresses its root cause. Preserve existing documented behavior and public contracts unless the issue explicitly requires a change. Inspect the relevant callers, invariants, and nearby tests before editing; avoid a repository-wide audit or unsolicited refactor.

If the root cause is uncertain, establish a focused reproduction or failing test first when practical. Implement a clear fix, not a symptom-suppressing workaround. Use existing patterns unless those patterns caused the defect. Prefer strong types and validation at trust boundaries; do not hide errors, weaken types, or introduce configuration/abstractions without clear benefit. Handle meaningful failure paths and cleanup. Leave helpful comments only for non-obvious constraints and the reasons behind them.

Run the narrowest meaningful verification first (affected tests/typecheck/lint), then broader checks only if change scope justifies them. If visual/runtime behavior is affected and tooling permits, inspect the actual behavior. Do not claim a check was run if it was not. Compare before/after behavior and inspect the diff for accidental changes.

**Completion:** the cause is addressed; regression is covered or a test limitation is explained; impacted behavior works; relevant checks pass or remaining failures are attributed; no unrelated changes remain.

**Output:** cause, implementation summary, files affected, evidence of verification, any remaining risks. Continue through verification rather than stopping immediately after writing code.


---

# 05 — Simplify & Remove Accidental Complexity

**Use when:** A system seems overbuilt, unnecessarily long, fragmented, or hard to follow. **Mode:** Implementation after focused impact analysis.

## Prompt

Simplify **[SYSTEM / AREA]** around what it currently needs to accomplish—not hypothetical future requirements. Identify its actual consumers, responsibilities, contracts, data flows, extension points, and failure modes. Trace usage before calling anything dead. Understand why the current complexity exists (including compatibility, concurrency, security, or performance requirements) before removing it.

Look for indirection without value, pass-through layers, duplicated policy, unnecessary wrappers, speculative providers, oversized state machines, redundant types, obsolete branches, overly configurable components, and fragmentation that makes one simple operation cross too many files. Also look for the inverse: superficially short code that is dangerously coupled or hides invariants.

For meaningful candidates, compare **current cost → simpler design → lost capabilities → affected callers → migration risk**. Prefer deleting, consolidating, or narrowing code rather than replacing it with a different elaborate architecture. No blanket ban on single-implementation interfaces: preserve an abstraction if it protects a meaningful boundary, enables tests, or isolates volatile dependencies. Measure improvement through fewer responsibilities/branches/concepts and clearer ownership; lines of code are supporting evidence, not the sole target.

Choose one bounded simplification with a worthwhile payoff; implement in reviewable steps, preserve behavior, and update tests/docs as needed. Remove compatibility layers only after checking consumers. Explain why anything intentionally complex remains.

**Output:** before/after responsibilities, why the simpler version suffices, key changes/deletions, behavioral evidence, tests, and any deferred opportunities. Do not expand scope merely to maximize deletions.


---

# 06 — Refactor, Reorganize & Compose

**Use when:** A large file or subsystem has mixed responsibilities or poor module boundaries. **Mode:** Scoped refactor with verification.

## Prompt

Refactor **[FILE / SYSTEM]** to improve cohesion, navigability, and future change safety while preserving intended behavior. Inspect the feature's callers, public API, types, lifecycle, ownership, side effects, and test coverage. Summarize the responsibility map and impact radius before editing; a compact plan is sufficient unless the migration is high-risk.

Define module boundaries around **reasons to change**, not an arbitrary file-length threshold or prescribed folder architecture. Prefer composition and explicit contracts where behavior varies independently; avoid inheritance hierarchies, god-managers, and needless microfiles. Separate state transitions from presentation/I/O when the boundary genuinely improves reasoning or reuse. Keep orchestration thin, data ownership clear, and dependencies directional. Do not force an entity-component-system or repository pattern where it does not suit the project.

Move/extract incrementally. Preserve public entry points where needed, migrate consumers deliberately, and remove stale paths when safe. Use the type system to encode meaningful domain constraints; avoid `any`, unjustified assertions, duplicated shapes, and wrappers that create more complexity than they remove. Document non-obvious behavior and design decisions *near the code they protect*.

Test the behavior exposed by the changed boundaries, not just implementation details. Run relevant static checks, tests, and a smoke scenario where possible. Inspect the final diff and structure for fragmentation and accidental public API expansion.

**Output:** old/new responsibility map (small diagram only if useful), module boundaries and tradeoffs, affected callers, changes made, evidence of parity, docs updated, and known risks. Favor clarity over file-count targets.


---

# 07 — Document Code That Is Hard to Understand

**Use when:** The code works but implementation rationale is missing. **Mode:** Documentation-first; avoid logic changes.

## Prompt

Improve the **in-code documentation** of **[SCOPE]** until a competent new maintainer can understand not only what happens but **why it is designed this way**. Read the code, callers, tests, and relevant history/docs before asserting intentions. Do not invent historical reasons: distinguish proven constraints from your inference.

Prioritize tricky sections: ordering constraints, concurrency, state transitions, caching, serialization, compatibility workarounds, security boundaries, surprising types, failure semantics, and performance tradeoffs. Place explanation next to the function, type, branch, or invariant it clarifies; a generic file header is not enough. Document the purpose, inputs/outputs when non-obvious, side effects, ownership, pre/postconditions, failure behavior, and what a future change must preserve. Write concise examples for reusable APIs when examples materially help.

Avoid describing syntax, bloated boilerplate templates, comments on every line, or documentation that contradicts code. Prefer expressive naming and simplified structure over a paragraph explaining needless complexity; **however, do not refactor behavior under cover of a documentation task**. Flag ambiguous logic for separate investigation.

Update relevant high-level docs only where necessary to record stable contracts or architectural choices. Check that links/examples are accurate and that comments remain near the code they explain. Run lightweight validation appropriate for documentation edits.

**Output:** documented areas, the most important invariants/reasons captured, remaining ambiguities, and any separate refactor candidates. **Do not change behavior.**


---

# 08 — UI/UX Audit & Targeted Revamp

**Use when:** A web or game UI feels inconsistent, confusing, or visually unfinished. **Mode:** Diagnose and improve; skip implementation only if requested.

## Prompt

Evaluate **[PAGE / USER FLOW]** as an interaction, not a screenshot. Establish who uses it, the primary action, information priority, platform constraints, visual identity, and patterns already present. If browser/rendering tools are available, inspect the real interface at representative sizes and states; otherwise distinguish code-inferred problems from observed visual defects.

Diagnose layout structure, alignment, container widths, visual hierarchy, spacing rhythm, typography, color roles/contrast, component states, responsive behavior, accessibility, loading/empty/error feedback, and interaction friction. For each issue, explain why it impairs comprehension, efficiency, or identity, not merely that it looks 'off'. Identify what is working and should be preserved.

Develop a coherent direction: limited semantic color roles, spacing/type scales, reusable component behavior, deliberate ornamentation, and restrained motion. Use animations and decorative elements to aid clarity or express the product—not as filler. Account for keyboard access, reduced-motion preferences, and meaningful focus/disabled states. Avoid arbitrary pixel perfection and unfounded symmetry; optimize for optical alignment and readable content.

Select the highest-value improvements and make focused changes using existing styling conventions. Avoid rebuilding unrelated components or adding a design-system framework by default. Inspect before/after results when tools permit, including overflow and interaction states, and verify relevant behavior. Do not claim visual inspection when only reasoning from source.

**Output:** main user-experience problems, proposed direction, specific changes, screenshots/checks if available, remaining constraints, and documented design decisions. No generic 'make it modern' redesign.


---

# 09 — Migration & Change Impact Plan (planning only)

**Use when:** A refactor, replacement, schema change, or module migration might affect multiple consumers. **Mode:** Read-only planning.

## Prompt

Design a safe, implementable migration for **[PROPOSED CHANGE]**. First establish current behavior, actual callers, external contracts, state/data ownership, persistence shape, lifecycle, and constraints. Trace dependency impact and check for consumers beyond direct imports (dynamic calls, config, tooling, tests, serialized data). Separate confirmed requirements from assumptions.

Compare the smallest viable migration paths, including **do nothing** where appropriate. Prefer the least disruptive solution with a genuine payoff. Specify the target contract and ownership boundaries, compatibility requirements, data/backfill implications, rollout order, test evidence, rollback or recovery strategy, and where irreversible operations need explicit authorization. Do not add adapters, flags, or dual paths unless migration complexity justifies them; explain when they can be removed.

Plan steps so each leaves the project coherent and verifiable. Name the files/systems expected to change and the contracts they protect. Identify the highest-risk seams and exactly how to validate them. Note decisions requiring product input rather than guessing.

**Output:** current/target model, options and chosen rationale, impact map, ordered migration steps, acceptance criteria, verification matrix, rollback limits, unresolved questions. **Do not edit.**


---

# 10 — Game Mechanic & Game-Feel Review

**Use when:** Designing or improving a gameplay mechanic, enemy/NPC, or game UI. **Mode:** Diagnose and implement if the request is to build; otherwise review only.

## Prompt

Work on **[GAMEPLAY FEATURE]** with two equal concerns: **system design** and **player experience**. Inspect the game's existing entity/component/system model, behavior patterns, feedback infrastructure, UX conventions, and asset pipeline before proposing a new architecture. Favor composition, reusable behavior/configuration, and explicit typed state over entity-specific branching, without turning one small mechanic into an all-purpose framework.

Define the player intention, activation conditions, state changes, interruption/failure cases, and how the player perceives each stage. For consequential interactions, consider *anticipation → action → impact → confirmation → recovery*. Choose context-appropriate visual effects, animation, sound, hit response, haptics, UI states, progress cues, or world changes. Not every action needs every effect; avoid noisy, misleading, inaccessible, or performance-heavy feedback. If an asset is unavailable, use a clean hook/placeholder and state what remains rather than silently omitting feedback.

Examples: gathering should communicate targetability, progress, success, inventory update, and depletion; combat should communicate valid contact, damage/block/immune outcomes, health change, and death or stagger state. Reuse shared damage, perception, navigation, interaction, and feedback mechanisms where those capabilities really match. Keep simulation and presentation separable if it benefits determinism, testing, or alternative renderers.

Verify gameplay invariants, repeat use, interruption, multiple entities, performance, UI visibility, and feedback timing in the real game if accessible. Document what the new capability owns, its extension points, and its design tradeoffs.

**Output:** gameplay contract, architecture/reuse rationale, player feedback matrix, change summary or design, verification, and unfinished assets/risks. Avoid hardcoding an ECS architecture not present in the repository.


---

# 11 — Establish Repository Agent Guardrails

**Use when:** Multiple coding agents contribute inconsistent architecture or undocumented patches. **Mode:** Create/update repo guidance and minimal tooling after inspecting the actual project.

## Prompt

Improve this repository's **agent development setup** so future changes remain understandable, verifiable, and easy to maintain. Inspect existing `AGENTS.md`, `CLAUDE.md`, scoped rules, skills, scripts, CI, formatter/linter/typecheck/tests, architecture docs, and developer workflows before adding anything. Reuse and consolidate existing instructions; avoid multiple competing sources of truth.

Keep always-loaded guidance **short and repository-specific**: project purpose, important module boundaries, primary commands, hard constraints, and pointers to deeper docs. Put task-specific procedures in on-demand skills/checklists or scoped instructions. Define lightweight expectations for ownership, composition versus special cases, strong type contracts, safe I/O boundaries, clear failure handling, useful in-code rationale, and pragmatic testing. Don't demand a full architectural survey, elaborate diagrams, or arbitrary code-size targets for minor edits.

Identify enforceable checks that offer real value: existing formatter/linter/typechecker/test scripts, dependency-cycle checks when justified, CI smoke checks, or non-destructive hooks. Prefer deterministic tooling to a rule telling agents to 'remember' checks. Before creating hooks or skills, explain their trigger, maintenance cost, portability, and how agents avoid unnecessary work. Never install dependencies or change CI policy without a concrete reason.

Implement a **minimal coherent setup**, documenting which file future agents should read for each task. Do not promise that prose-only rules enforce behavior; distinguish instructions from executable checks. Verify that examples/commands work in this repo and that new guidance doesn't contradict existing policy.

**Output:** previous setup problems, created/changed files, how the guidance loads, actual checks enabled, verification, and intentional omissions. The new setup should reduce future context and maintenance overhead, not increase it.


---

# 12 — Product Gaps & Feature Opportunities (discovery only)

**Use when:** You want to know what to build next, rather than merely what code to clean up. **Mode:** Read-only.

## Prompt

Review the current product behavior and codebase to identify **missing or unfinished user-facing capabilities** that would make the product more coherent. Infer the product's core job from the app, docs, tests, and user flows, but do not treat typical industry feature lists as requirements.

Walk through the main journeys from a user perspective. Identify broken or missing happy-path steps, friction, inaccessible operations, misleading states, missing error/empty/loading feedback, inconsistent permissions, unsaved work, confusing defaults, and opportunities to reuse existing capabilities. Separate code-confirmed gaps from speculative enhancements and identify where user research would be needed. Consider security, accessibility, performance, and support burden alongside attractiveness.

For each candidate give the user problem, evidence, expected benefit, estimated scope/dependencies, risk, and the smallest experiment or acceptance test that could validate value. Rank using actual product goals and evidence, not novelty. State what not to build and why.

**Output:** existing user journeys, verified gaps, clearly labeled hypotheses, prioritized opportunities, recommended next experiments/tasks. **No implementation.**
