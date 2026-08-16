# Planner-Executor Workflow

## Roles

The main agent is the planner. Executors are GPT-5.6 Luna subagents running at Max reasoning effort. Verifiers and adversarial reviewers are GPT-5.6 Terra subagents running at High reasoning effort.

Use one executor, one verifier, and one adversarial reviewer by default. The planner may run up to three executors concurrently only when tasks are independent, have disjoint file ownership, and can be verified separately. Verification and review agents are read-only and do not repair findings. The planner keeps integration, roadmap edits, dependency decisions, conflict resolution, repair orders, and final acceptance in the main thread.

## Loop

```text
inspect -> select -> plan -> execute -> verify -> adversarial review
   ^                                                   |
   |                                                   v
   +--------- next ready task <- record <- planner decision
```

### 1. Inspect

Read the repository-root `AGENTS.md`, the relevant documents under `docs/`, the active milestone, current source, and current test failures. Check the worktree before assigning ownership. This step is complete when constraints, existing behavior, and unrelated user changes are identified.

### 2. Select

Choose the earliest unchecked task whose dependencies and prior milestone gate are complete. Mark only that task `[~]`. This step is complete when the task ID, dependency state, and acceptance criteria are explicit.

### 3. Plan

The planner creates one bounded work order:

```text
Task: <roadmap ID and title>
Goal: <observable behavior>
Own: <files or modules the executor may change>
Read: <required files and contracts>
Constraints: <task-specific boundaries>
Verify: <commands and manual checks>
Return: <changed files, evidence, risks, follow-up>
```

Prefer one vertical slice over separate implementation and test assignments. Split work only when each result is independently useful. This step is complete when an executor can act without guessing product behavior or file ownership.

### 4. Execute

The Luna Max executor inspects its owned area, makes the smallest complete change, adds focused behavior tests, and runs the assigned checks. It does not change task scope, architecture, dependencies, or files outside ownership without returning a blocker to the planner. This step is complete when the executor returns the requested evidence or a concrete blocker.

### 5. Independent Verification

The planner gives a Terra High verifier the work order, executor report, diff, acceptance criteria, and relevant `TESTING.md` sections. The verifier checks observable behavior independently, runs focused checks first, and runs every quality command required by the current gate. For UI tasks, it inspects `TestBackend` output and performs the specified real-terminal check when practical. It does not trust executor claims without evidence and does not edit files.

The verifier returns:

```text
Verification: pass | fail | blocked
Acceptance: <criterion-by-criterion result and evidence>
Commands: <commands run and results>
Manual: <checks run and observations>
Gaps: <missing evidence or unverified behavior>
Risk: <remaining verification risk>
```

This step is complete when every acceptance criterion has evidence or an explicit gap and unrelated changes remain intact.

### 6. Adversarial Review

After verification, the planner gives a separate Terra High reviewer the work order, diff, verification report, product and architecture contracts, and relevant tests. The reviewer looks for behavioral regressions, invalid assumptions, failure modes, security or privacy issues, concurrency and terminal-lifecycle defects, scope creep, and tests that pass without proving the required behavior. It remains read-only and does not repeat the verifier's checklist unless challenging its evidence.

The reviewer returns findings first, ordered by severity:

```text
Review: findings | no findings | blocked
Findings: <severity, file:line, impact, evidence, smallest safe correction>
Questions: <material unresolved assumptions>
Test gaps: <missing regression or failure-path coverage>
Residual risk: <risk that remains even if findings are fixed>
```

If there are no findings, the report states that explicitly. This step is complete when each finding is actionable and evidence-backed, or the reviewer records why review was blocked.

### 7. Planner Decision

The verifier and reviewer send their reports to the planner. The planner compares them with the task contract and chooses one outcome:

- `accept`: verification passed and no unresolved finding violates acceptance, product, architecture, security, or quality gates;
- `repair`: send the Luna Max executor one bounded repair order with observed evidence, expected behavior, severity, and file ownership;
- `block`: record the missing dependency, evidence, credential, or decision that prevents safe progress;
- `escalate`: ask the user when resolution changes product scope, architecture, cost, permissions, or an irreversible action.

After a repair, repeat independent verification and adversarial review for the changed behavior. After two failed repair passes, the planner resolves the issue directly or records a blocker. This step is complete when the task is accepted, blocked, or escalated.

### 8. Record

For an accepted task, mark it `[x]` and add an `Evidence:` line with the executor, verification, and review results. Update documentation when behavior or a decision changed. Close a quality gate only after all checks pass and review findings are resolved or explicitly accepted by the planner with rationale. Report changed files, behavior, verification results, review findings, and remaining risks. Then return to inspection for the next ready task.

## Parallel Execution

Parallel work is permitted only when all conditions hold:

- each executor has a different roadmap task, and each verifier or reviewer has one read-only assignment;
- file ownership does not overlap;
- neither task changes shared domain types, dependency manifests, lockfiles, architecture, or roadmap state;
- completion of one task cannot change the other's acceptance criteria;
- the planner can integrate and verify each result independently.

Examples of safe parallel work after interfaces exist include pure formatter tests and a README draft. Event-loop work and state rendering are not safe to parallelize because they share behavior and types.

Verification and adversarial review stay independent. Do not combine them into one agent or run adversarial review before the verification report is available. Their different prompts and context are the control against shared blind spots.

## Decision Policy

The planner decides in this order:

1. Follow explicit user requirements.
2. Preserve contracts in `AGENTS.md`, `PRODUCT.md`, and `ARCHITECTURE.md`.
3. Satisfy the active roadmap task and gate.
4. Choose the smallest approach that keeps the next accepted milestone possible.

Record an architecture decision in `ARCHITECTURE.md` when adding a process boundary, persistent storage, provider, production dependency, or security-sensitive behavior. Record a product decision in `PRODUCT.md` when changing first-release scope or observable behavior. Ask the user before publishing, spending money, sending private data, changing permissions, or affecting a live service.

## Blockers

An executor returns a blocker instead of guessing when it finds ambiguous product behavior, overlapping user changes, unavailable credentials required for the acceptance test, an architecture conflict, or an irreversible action. A blocker report contains the task ID, observed evidence, attempted safe actions, impact, and the smallest decision needed.

Missing live API credentials do not block implementation or automated tests. Use fixtures and a local mock server, then record live-provider validation as remaining manual evidence.

## Completion Report

Use this compact form:

```text
Task: Mx-yy accepted | blocked
Changed: <files and behavior>
Verified: <commands and results>
Reviewed: <findings resolved, accepted, or none>
Evidence: <acceptance criteria and manual observations>
Risk: <remaining risk, or none>
Next: <next ready task or required decision>
```
