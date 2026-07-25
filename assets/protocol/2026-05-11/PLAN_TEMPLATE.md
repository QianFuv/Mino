# <Requirement Name>

Protocol Version: 2026-05-11  
Protocol Revision: review-rework-git-flow-v1

Status values and execution rules are defined in `PLAN_EXECUTION.md`.

## Metadata

| Field | Value |
|---|---|
| Task ID | <YYYY-MM-DD-slug or tracker id> |
| Status | Draft |
| Priority | <P0, P1, P2, P3, or N/A> |
| Type | <frontend, backend, fullstack, test, docs, infra, refactor, bugfix, or N/A> |
| Area | <package, module, product area, or N/A> |
| Owner | <name or role> |
| Created | <YYYY-MM-DD> |
| Updated | <YYYY-MM-DD> |
| Branch | <branch name or N/A> |
| Git Repository | Unknown |
| Git Working Tree | Unknown |
| Git Base Commit | Unknown |
| Git Base Status | Unknown |
| Git Flow Enabled | No |
| Git Flow Consent | Pending |
| Plan Approved At | Pending |
| Pre-Plan Cleanup Required | Unknown |
| Pre-Plan Cleanup Decision | Pending |
| Related Links | <issues, PRs, designs, docs, logs, or N/A> |

Git metadata value sets:

- Git Repository: `Unknown`, `Present`, `Missing`
- Git Working Tree: `Unknown`, `Clean`, `Dirty`, `Not Applicable`
- Git Base Commit: `Unknown`, `<short hash>`, `N/A`
- Git Base Status: `Unknown`, `<clean status evidence>`, `N/A`
- Git Flow Enabled: `Yes`, `No`
- Git Flow Consent: `Pending`, `Approved`, `Disabled`
- Plan Approved At: `Pending`, `<YYYY-MM-DD HH:mm TZ>`, `N/A`
- Pre-Plan Cleanup Required: `Yes`, `No`, `Unknown`
- Pre-Plan Cleanup Decision: `Pending`, `Approved`, `Declined`, `Completed`, `Not Required`

## Summary

<1-2 sentence objective and implementation approach.>

## Context

### Original Request

<Restate the request in enough detail that this document can stand alone.>

### Current State and References

| Reference | Fact | Implication |
|---|---|---|
| `<path, command, log, or URL>` | <discovered fact> | <how this affects the plan or implementation> |

## Git Readiness

| Check | Command / Source | Result | Decision |
|---|---|---|---|
| Git repository present | `git rev-parse --is-inside-work-tree` |  |  |
| Current branch | `git branch --show-current` |  |  |
| Git base commit | `git rev-parse --short HEAD` |  |  |
| Working tree status | `git status --short` |  |  |
| Pre-existing changes | Diff inspection |  |  |
| Git Flow default | Derived from Git readiness |  |  |

### Git Readiness Decision

- If Git repository is present and working tree is clean: set `Git Flow Enabled = Yes`.
- If Git repository is missing: ask whether to initialize Git and create initial single-responsibility commit(s).
- If Git repository is present but working tree is dirty: ask whether to organize pre-existing changes into single-responsibility cleanup commit(s).
- If cleanup is declined and task changes cannot be safely separated from existing changes: set plan status to `Blocked`.

## Pre-Plan Cleanup

Use this section only for changes that existed before this task plan was approved.

| Cleanup Item | Logical Change | Files | Proposed Commit Message | Consent Status | Actual Commit Hash | Notes |
|---|---|---|---|---|---|---|
| N/A | N/A | N/A | N/A | Not Required | N/A | Working tree was clean before plan approval. |

Pre-plan cleanup commits require explicit user approval and are not covered by Git Flow consent.
Keep the `N/A` row only when cleanup is not required; it records the final cleanup decision. When cleanup is required, replace the `N/A` row with `C1`, `C2`, etc. Delete all unresolved placeholder cleanup rows before marking the plan `Ready`.

## Scope

### Goal

<State the intended outcome in concrete terms.>

### Deliverables

- <Exact feature, file, endpoint, command, component, migration, or behavior>
- <Exact feature, file, endpoint, command, component, migration, or behavior>

### In Scope

- <Behavior, file area, workflow, or compatibility requirement included in this task>
- <Behavior, file area, workflow, or compatibility requirement included in this task>

### Out of Scope / Must Not Change

- <Related behavior or scope that should not be implemented>
- <Forbidden change, compatibility boundary, or anti-pattern to avoid>

## Decisions, Assumptions, and Open Questions

| Item | Type | Default / Decision | Reason | Status |
|---|---|---|---|---|
| <decision, assumption, or question> | <Decision, Assumption, or Question> | <chosen option, default, or proposed answer> | <why it matters> | <Confirmed, Active, Open, or Deferred> |

## Plan

### Approach

<Describe the implementation approach. The plan should be decision complete: a future implementer should not need to choose architecture, interfaces, data flow, test strategy, or acceptance criteria.>

### File Map

| Path | Change | Reason | Task |
|---|---|---|---|
| `<path/to/file.ext>` | <Create, Modify, Delete, Test, or N/A> | <why this file is involved> | T1 |

### Interfaces / Data Flow

<Describe affected APIs, schemas, commands, configuration, state transitions, events, or component boundaries. Use `N/A` if not affected.>

### Edge Cases

| Case | Expected Behavior | Covered By |
|---|---|---|
| <happy path, edge case, or failure mode> | <binary observable outcome> | <task id or verification check> |

## Implementation Task Order

1. T1: <first task and why it comes first>
2. T2: <second task and dependency>

Final verification is handled by `Verification Plan` after all implementation tasks are `Done`.

Review rework tasks may be appended after original implementation tasks as `R1`, `R2`,
etc. only when review feedback is in-scope or corrects acceptance/verification defects.
Out-of-scope follow-up work must not be added to `Implementation Task Order`.
When Git Flow is enabled, every executable review rework task must also have a Git Flow row before execution.

## Git Flow

Git Flow consent is active only when:

- `Metadata > Git Flow Enabled = Yes`;
- `Metadata > Git Flow Consent = Approved`;
- the user explicitly approved this plan;
- `Metadata > Git Base Commit` and `Metadata > Plan Approved At` are populated;
- the task changes were created after plan approval.

Commit Status values:

- Pending: commit gate has not run.
- Not Required: no commit is required for this task.
- Skipped: commit was intentionally skipped with an accepted reason.
- Blocked: commit could not be created safely.
- Committed: task-level commit was created and recorded.

| Task | Commit Required | Commit Status | Planned Commit Message | Commit Scope | Actual Commit Hash | Committed Files | Git Evidence | Notes |
|---|---|---|---|---|---|---|---|---|
| T1 | Yes | Pending | `type(scope): short imperative description` | `path/to/file` |  |  |  |  |
| T2 | Yes | Pending | `type(scope): short imperative description` | `path/to/file` |  |  |  |  |

Replace placeholder Git Flow rows with the real task list before marking the plan `Ready`. Do not leave placeholder task rows in an executable plan.

## Tasks

### T1: <Task Title>

Status: Ready  
Depends On: None

**Do**

- <Concrete implementation step>
- <Concrete test or verification step>

**Acceptance Criteria**

- [ ] <Observable behavior or condition>
- [ ] <Regression or compatibility condition>

**Verification**

- Command / Steps: `<exact command or tool steps>`
- Expected Result: <specific pass/fail output or observable result>
- Planned Evidence: <expected artifact, screenshot, log, command output summary, or N/A>

### T2: <Task Title>

Status: Ready  
Depends On: T1

**Do**

- <Concrete implementation step>
- <Concrete test or verification step>

**Acceptance Criteria**

- [ ] <Observable behavior or condition>
- [ ] <Regression or compatibility condition>

**Verification**

- Command / Steps: `<exact command or tool steps>`
- Expected Result: <specific pass/fail output or observable result>
- Planned Evidence: <expected artifact, screenshot, log, command output summary, or N/A>

## Verification Plan

| Check | Command / Steps | Expected Result | Planned Evidence |
|---|---|---|---|
| <lint, typecheck, unit test, integration test, build, UI check, API check, or manual tool check> | `<exact command or tool steps>` | <specific pass/fail output or observable result> | <planned evidence location or summary> |

## Progress Log

Only state transitions and execution-state events.

| Timestamp | Status | Notes |
|---|---|---|
| <YYYY-MM-DD HH:mm TZ> | Draft | <initial context or decision> |

## Implementation Notes

Only technical details, important implementation decisions, and deviations from the plan.

- <Implementation note>
- <Plan deviation and reason>

## Verification Results

| Timestamp | Task / Check | Result | Evidence |
|---|---|---|---|
| <YYYY-MM-DD HH:mm TZ> | T1 / <check name> | <Passed, Failed, Not Run, or Blocked> | <command output summary, artifact path, or reason> |

## Review Feedback

| Timestamp | Reviewer | Feedback | Classification | Action | Linked Task | Status |
|---|---|---|---|---|---|---|
| <YYYY-MM-DD HH:mm TZ> | <name, role, or N/A> | <requested change or acceptance note> | <Acceptance Defect, In-Scope Rework, Material Change, Follow-Up, or Accepted> | <reopen task, add R task, block for approval, record follow-up, or mark accepted> | <T1, R1, follow-up id, or N/A> | <Open, In Progress, Resolved, Blocked, or Deferred> |

For `Classification = Accepted`, use `Action = mark accepted` and `Status = Resolved`.

## Final Outcome

- Summary: <final user-visible or system-visible result>
- Remaining Risk: <known residual risk or N/A>
- Follow-Up Tasks: <new task ids or N/A>
