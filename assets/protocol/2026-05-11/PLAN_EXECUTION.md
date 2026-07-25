# Task Execution Guide

Protocol Version: 2026-05-11  
Protocol Revision: review-rework-git-flow-v1

Use this guide when executing a plan document created from `PLAN_TEMPLATE.md`.
The task document is the execution source of truth. This guide defines how to move
through it without changing scope, skipping verification, overwriting user work, or
losing recoverable progress state.

## Non-Negotiable Rules

- Execute only the next eligible task in `Implementation Task Order`.
- Stay within `Scope`, `File Map`, and task `Acceptance Criteria`.
- Record progress, deviations, and verification in the task document.
- Do not mark work `Done` without recorded verification evidence.
- Do not add, remove, or upgrade dependencies unless the task document explicitly includes the dependency change in `Scope`, `File Map`, and `Verification Plan`.
- Do not print, persist, or copy secrets into the task document; record only that a credential was required or unavailable.
- Do not commit unless the action is either manually approved by the user or is a Plan-approved Git Flow commit for a completed task. Do not push, merge, discard, reset, rebase, amend, force-push, create tags, delete branches, or perform other irreversible Git operations without separate explicit user approval.
- Stop for material scope, behavior, data, dependency, security, or compatibility changes.

## Status Values

Use these values for the top-level `Metadata` status:

| Status | Meaning |
|---|---|
| Draft | The document is not ready to execute. |
| Ready | The document is sufficiently specified and can be executed. |
| In Progress | Implementation or verification is underway. |
| Blocked | Work cannot continue without a named decision, dependency, or fix. |
| Review | Implementation and verification are complete, but human review or acceptance is pending. |
| Done | Acceptance criteria and final verification are recorded as passed, or documented exceptions are explicitly accepted by the user, owner, or project maintainer. |

Use these values for per-task `Status` lines:

| Status | Meaning |
|---|---|
| Draft | The task is not ready to execute. |
| Ready | The task is sufficiently specified and can be executed. |
| In Progress | Implementation or verification is underway. |
| Blocked | Work cannot continue without a named decision, dependency, or fix. |
| Done | Acceptance criteria and task-level verification are recorded as passed, or documented exceptions are explicitly accepted by the user, owner, or project maintainer. |

## Status Invariants

- If any executable task is `In Progress`, the top-level status is `In Progress`.
- If any required task is `Blocked`, the top-level status is `Blocked`.
- Top-level `Review` means implementation and verification are complete, but human review or acceptance is pending.
- `Review` is not a terminal state. Reviewer-requested changes either reopen execution, block for approval, become follow-up work, or complete acceptance.
- A task document can be marked `Ready` only when every task listed in `Implementation Task Order` is `Ready`.
- Deferred work must not appear as an executable item in `Implementation Task Order`; record it as an open question, assumption, decision, or follow-up task.
- Do not start an executable task whose per-task status is `Draft`.
- Top-level `Done` means all acceptance criteria and final verification are recorded as passed, or documented exceptions are explicitly accepted by the user, owner, or project maintainer.

## Phase 0: Git Readiness Confirmation

Before executing an approved task-specific plan:

1. Confirm the plan contains `Git Readiness`.
2. Confirm `Metadata > Git Repository`, `Git Working Tree`, `Git Base Commit`, `Git Base Status`, `Git Flow Enabled`, `Git Flow Consent`, and `Plan Approved At` are populated.
3. If `Git Flow Enabled = Yes`, confirm the plan started from a clean Git working tree or that approved pre-plan cleanup was completed.
4. If `Git Flow Enabled = Yes` but `Git Flow Consent = Pending`, stop before execution and ask whether plan approval includes Git Flow consent. Do not silently continue with Git Flow skipped.
5. If `Git Flow Enabled = Yes` but `Plan Approved At = Pending`, stop before execution and record the plan approval timestamp or set `Git Flow Consent = Disabled`.
6. If `Git Flow Consent = Disabled`, confirm `Git Flow Enabled = No` before execution. If not, update `Git Flow Enabled = No` or mark the plan `Blocked` until the consent state is corrected.
7. If the working tree is dirty before execution starts:
   - Determine whether the dirty files are pre-existing changes or plan execution changes.
   - Do not continue if the changes cannot be safely separated.
   - If they are pre-existing changes, stop and request explicit user approval for cleanup commits.
8. Do not treat pre-plan cleanup commits as Git Flow commits.

## Ready Criteria

A task document can be marked `Ready` only when:

- All execution-critical placeholders are replaced.
- `Scope`, `File Map`, `Implementation Task Order`, task `Acceptance Criteria`, task `Verification`, and `Verification Plan` are executable.
- Open questions have confirmed answers, active assumptions, or deferred status.
- No executable task remains `Draft`.
- If `Git Flow Enabled = Yes`, `Git Readiness`, `Git Base Commit`, `Git Base Status`, `Plan Approved At`, and every executable task's `Git Flow` row are populated.

## Phase 1: Load

1. Read this guide.
2. Read the task document completely.
3. Read applicable repository instructions, including `AGENTS.md`.
4. Confirm the task document status is `Ready`, `In Progress` when resuming implementation,
   or `Review` when processing reviewer feedback through Phase 7.
5. If an implementation task remains not `Done`, confirm the next eligible task has executable `Acceptance Criteria` and `Verification`.
6. If execution was requested and the document is not executable, mark it `Blocked` and record the smallest blocker.

Do not start implementation from a `Draft` task document unless the user explicitly asks
to proceed and the missing fields can be resolved from repository inspection.
If the document is still being authored, leave it `Draft`.

## Phase 2: Select Next Task

1. Find the first task in `Implementation Task Order` that is not `Done`.
2. Continue only if that task is `Ready`, or `In Progress` when resuming.
3. If that task is `Draft` or `Blocked`, do not skip it; keep or set the top-level status accordingly and record the blocker.
4. Confirm every task listed in `Depends On` is `Done`.
5. Review relevant `File Map` rows and `Current State and References`.
6. Check the working tree for unrelated changes before editing.
7. Mark the selected task and top-level status `In Progress`.
8. Add a `Progress Log` entry.

Do not start a task when dependencies are incomplete, acceptance criteria are not testable,
or required edits cannot be separated safely from unrelated user changes.

## Phase 3: Implement

1. Implement only the selected task.
2. Prefer test-first for behavior changes when the repository has a clear, low-friction test pattern.
3. Keep edits within `File Map`.
4. Record minor support edits before verification.
5. Stop for material deviations requiring user approval.

Minor support edits are allowed when they are required by the current task and do not change
public behavior. Add them to `File Map` as soon as they are identified and no later than before verification. Examples include
barrel exports, test fixtures, snapshots, import path updates, type exports, or test setup.

Material deviations require approval before continuing. Material deviations include changes
to user-visible behavior, public APIs, data shape, dependencies, compatibility guarantees,
security boundaries, or task scope.

## Phase 4: Verify

1. Run the task-level `Verification`.
2. Check every task-level `Acceptance Criteria` item.
3. Record actual results and evidence in `Verification Results`.
4. If verification passes, mark the task `Done`.
5. If verification fails because implementation is incomplete, fix the task and rerun verification.
6. If verification fails because of plan, environment, or scope mismatch, mark the task `Blocked`.

A task cannot be marked `Done` when required verification was not run, unless the task
document explicitly allows an equivalent check or the exception is documented and accepted
by the user, owner, or project maintainer. Record the approver and reason in `Progress Log`
or `Implementation Notes`.

## Phase 4.5: Task Git Flow Commit

After the selected task is marked `Done` and before moving to the next task:

1. Check `Metadata > Git Flow Enabled` and `Metadata > Git Flow Consent`.
2. If `Git Flow Enabled != Yes` or `Git Flow Consent != Approved`:
   - Do not create a Plan-approved Git Flow commit.
   - If the task requires a commit, stop and request Manual Commit Approval.
   - Continue only when the task explicitly does not require a commit or the user accepts skipping it.
3. If Git Flow is enabled and consent is approved, find the selected task's row in `Git Flow`.
4. If the selected task has no Git Flow row:
   - Set `Commit Status = Blocked`.
   - Set the top-level status to `Blocked`.
   - Record that the approved Git Flow plan is missing the task commit gate.
   - Do not continue to the next task.
5. If `Commit Required = No`:
   - Set `Commit Status = Not Required`.
   - Record the reason.
   - Continue to Phase 5.
6. If `Commit Required = Yes`, continue only when:
   - The task is listed in `Implementation Task Order`.
   - The task status is `Done`.
   - Task-level verification evidence is recorded.
   - The commit message exactly matches `Planned Commit Message`.
   - The files to be staged are within the task's declared `File Map` and `Commit Scope`.
   - The changes were created after plan approval and are not pre-plan cleanup changes.
   - The working tree has no unrelated changes that would be staged or committed.
7. Before staging:
   - Run `git status --short`.
   - Run `git diff --name-only`.
   - Run `git diff --cached --name-only`.
   - If the index already contains staged files, confirm they are only for the current task. If not, mark the task document `Blocked`; do not unstage without explicit user approval.
   - Inspect the relevant diff for the current task's `File Map` and `Commit Scope`.
   - Do not use `git add .`.
8. Stage only explicit paths or hunks within the task's declared `File Map` and `Commit Scope`.
9. Confirm staged files with `git diff --cached --name-only`. If any staged file is outside the task's `File Map` and `Commit Scope`, mark the task document `Blocked` and do not commit.
10. Run `git commit` with exactly the planned single-line commit message. Do not use `git commit --no-verify` unless the user explicitly approved bypassing hooks.
11. After committing:
    - Run `git rev-parse --short HEAD`.
    - Run `git show --stat --oneline HEAD`.
    - Run `git status --short`.
    - Continue only if `git status --short` is clean.
    - If remaining changes exist, set `Commit Status = Blocked` unless the user explicitly accepts continuing with documented unrelated changes.
12. If `git commit` fails or hooks modify files in a way that changes the task scope, set `Commit Status = Blocked`, set the top-level status to `Blocked`, and record the failure.
13. Record the actual commit hash, committed files, and Git evidence in `Git Flow`.
14. Add a `Progress Log` entry for the task-level commit.
15. Stop for explicit user approval before any Git operation outside this task-level commit.

## Phase 5: Continue

Repeat Phases 2 through 4.5 until all tasks are `Done` or the document is `Blocked`.

Before moving to the next task:

- The current task is `Done`.
- The current task's Plan-approved Git Flow commit gate is `Committed`, `Not Required`, or `Skipped` with an accepted reason.
- If the commit gate is `Blocked`, do not move to the next task; keep the top-level status `Blocked`.
- `Progress Log` records the transition.
- `Implementation Notes` records relevant deviations.
- `Verification Results` records the latest check results.

## Phase 6: Final Verification

1. Run the global `Verification Plan`.
2. Confirm every `In Scope` item is covered by deliverables, tasks, or verification.
3. Confirm every `Out of Scope / Must Not Change` item remains unchanged.
4. Confirm changed files match `File Map`, or that deviations are documented.
5. Record final `Verification Results`.
6. Update `Final Outcome`.
7. Set top-level status to `Review` when implementation and verification are complete but user review or acceptance is pending.
8. Set top-level status to `Done` only when final verification is recorded as passed, or documented exceptions are explicitly accepted by the user, owner, or project maintainer.

## Phase 7: Review Feedback and Rework

When the top-level status is `Review`, process reviewer feedback before setting the
document to `Done`.

1. Classify each review item as one of:
   - Acceptance defect: existing acceptance criteria or verification evidence is incomplete,
     stale, or contradicted.
   - In-scope rework: a task-local correction within existing `Scope`, `File Map`,
     and acceptance criteria.
   - Material change: a change to behavior, data shape, public API, dependencies,
     compatibility, security boundary, or task scope.
   - Follow-up: valid work that is outside this document's scope and should not block acceptance.
   - Accepted: reviewer accepts the original scope with no required changes.

2. For an acceptance defect:
   - Set the affected task status back to `Ready` or `In Progress`.
   - Set the top-level status to `In Progress`.
   - Record the review finding in `Review Feedback` when present and in `Progress Log`.
   - Record the technical reason in `Implementation Notes`.
   - Preserve prior `Verification Results`; add new verification rows after rework.

3. For in-scope rework:
   - When Git Flow is enabled and an existing task is reopened or a new review rework task is added, update the Git Flow row before executing the rework.
   - For reopened tasks, decide whether the existing task commit is final or whether rework needs a new `R` task commit.
   - For new `R` tasks, add a Git Flow row with `Commit Required`, `Planned Commit Message`, and `Commit Scope`.
   - If the commit scope or message cannot be specified without changing the approved plan materially, return to the Plan Review Gate.
   - Do not use `git commit --amend` for rework unless the user separately approves it.
   - If the rework clearly belongs to an existing task:
     - Set the affected task status back to `Ready` or `In Progress`.
     - Set the top-level status to `In Progress`.
     - Record the review item in `Review Feedback` when present and in `Progress Log`.
     - Update `File Map`, `Acceptance Criteria`, or `Verification` only when the existing entries are incomplete or stale.
     - Preserve prior `Verification Results`; add new verification rows after rework.
     - Execute it using Phases 2 through 6.
   - If the rework does not clearly belong to an existing task:
     - Add a new task such as `R1: Review rework - <short title>`.
     - Add it to `Implementation Task Order` after the original tasks.
     - Set the new task status to `Ready`.
     - Set the top-level status to `In Progress`.
     - Add or update `File Map`, `Acceptance Criteria`, and `Verification` for the rework.
     - Record the review item in `Review Feedback` when present.
     - Execute it using Phases 2 through 6.

4. For material change:
   - Set the top-level status to `Blocked`.
   - Record the reviewer request, why it is material, what was attempted, and the smallest
     approval or decision needed to continue.
   - Record the blocker in `Review Feedback` when present.
   - Do not implement until explicitly approved.
   - If approved and still part of the same objective, amend the task document with new
     task(s), acceptance criteria, and verification.
   - If it is a separate objective, record it as a follow-up task and keep this document
     eligible for acceptance if the original scope passes.

5. For follow-up work:
   - Record it in `Final Outcome > Follow-Up Tasks`.
   - Record it in `Review Feedback` when present.
   - Do not add it to `Implementation Task Order`.
   - Keep the top-level status as `Review` until accepted, or move to `Done` if the reviewer
     accepts the original scope.

6. For acceptance with no required changes:
   - Record the acceptance in `Review Feedback` when present and in `Progress Log`.
   - Set the top-level status to `Done`.

7. After any rework:
   - Run affected task verification.
   - Run the global `Verification Plan`.
   - Record new evidence in `Verification Results`.
   - Update `Final Outcome`.
   - Return the top-level status to `Review` when ready for another review cycle.
   - Set top-level status to `Done` only after acceptance or accepted exceptions are recorded.

## Deviation Handling

Use this triage before blocking:

| Situation | Action |
|---|---|
| The answer is clear from repository inspection. | Update the task document and continue. |
| The deviation is minor, task-local, and does not change behavior or scope. | Record it in `File Map` and `Implementation Notes`, then continue. |
| The deviation changes behavior, data, dependencies, compatibility, security, or scope. | Mark `Blocked` and ask for approval or clarification. |

## User Change Protection

If unrelated modified files exist:

- Do not edit them unless they are listed for the current task.
- If a listed file already has unrelated user changes, inspect the diff before editing.
- Preserve user changes and record the conflict or constraint in `Progress Log` or `Implementation Notes`.
- Stop if the required edit cannot be separated safely.

## Stop Conditions

Stop execution and mark the task document `Blocked` when any of these happen:

- Requirements are missing, contradictory, or materially stale.
- A required environment variable, credential, service, package, fixture, or tool is unavailable.
- Required changes exceed `In Scope` or violate `Out of Scope / Must Not Change`.
- Verification fails for reasons outside the current task.
- Required verification cannot run and no equivalent check or accepted exception exists.
- User changes cannot be safely preserved.
- An irreversible Git operation would be required without explicit user approval.
- `Git Flow Enabled = Yes` but `Git Flow Consent` is not `Approved`.
- `Git Flow Enabled = Yes` but the selected task has no Git Flow row.
- A task marked `Commit Required = Yes` cannot be committed using only files in its `File Map` and `Commit Scope`.
- The planned commit message is missing, invalid, or no longer matches the implemented change.
- The index contains staged files that are unrelated to the current task.
- The working tree contains unrelated changes that cannot be safely excluded from the task-level commit.
- `git commit` fails or hooks modify files in a way that changes the task scope.
- A review rework task is added while Git Flow is enabled but no Git Flow row is added for the rework task.

For each blocker, record:

- Task ID.
- Blocker.
- What was attempted.
- Smallest decision or dependency needed to continue.

## Document Update Rules

- `Progress Log`: state transitions and execution-state events only.
- `Implementation Notes`: technical details, plan deviations, and execution-time decisions only.
- `Verification Results`: actual checks, results, and evidence only.
- `Review Feedback`: reviewer requests or acceptance, classification, action, linked task, and status only.
- `Final Outcome`: final result, remaining risk, and follow-up tasks only.
- `Metadata.Updated`: update whenever status, task state, implementation notes, verification results, or final outcome changes.

Do not rewrite the task document for style while executing it. Only update fields required
to keep state, implementation notes, and verification evidence accurate.

## Resuming Work

When resuming, run Phase 0, then Phase 1.

- If an implementation task in `Implementation Task Order` is not `Done`, select the first not-`Done` task and continue from Phase 2.
- If all implementation tasks are `Done` but top-level status is not `Review` or `Done`, continue from Phase 6.
- If the top-level status is `Review` and reviewer feedback requests changes, run Phase 7 before resuming implementation.
- If Phase 7 reopens a task or adds a review rework task, continue from Phase 2.
- If Phase 7 records only follow-up work and the reviewer accepts the original scope, set the top-level status to `Done`.
- Do not redo `Done` tasks unless verification is missing, stale, or contradicted by later changes.
