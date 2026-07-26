<!-- mino-managed-skill:v1 -->

# Mino command contract

## Agent invocation

Use `--format json --no-input` for every Agent-driven invocation. Read stdout
as one JSON value and keep stderr diagnostic-only. Start each decision cycle
with:

```text
mino project doctor --format json --no-input
mino agent context --format json --no-input
```

If doctor reports that the repository has not been initialized, bootstrap once
with `mino project init --format json --no-input`, then execute only its safe
returned `next_actions` and rerun doctor.

The context fields that control behavior are `active_plan`,
`approval_required`, `blocked_actions`, and `next_actions`. An action contains
an `id` and an `argv` array. Execute argv as an argument vector without shell
parsing. Re-read context after each successful mutation.

## Git identity and active binding

Agent context includes canonical Git worktree identity, branch or detached
HEAD, cleanliness, staged/unstaged paths, and active-binding status when the
project is in a Git worktree. `current` is the only binding status that may
select an active plan. Treat `foreign_worktree`, `stale_branch`, `stale_head`,
and `not_repository` as no active plan; do not fall back to another plan.

Use `mino git inspect --plan <id> --format json --no-input` for a complete
read-only relationship report. Use
`mino git bind --plan <id> --current --format json --no-input` only to bind the
named non-Done plan to the exact current worktree and branch or detached HEAD.
Binding writes `.mino/active.json`; it does not stage, commit, switch branches,
or otherwise mutate Git. Exact repeated binds are idempotent; binding another
plan explicitly replaces only the current worktree's prior selection.

Use `mino git branch propose --plan <id> --format json --no-input` to obtain the
only supported branch name and current blockers. The proposal is read-only.
`mino git branch create --plan <id> --approval-ref <ref> --format json
--no-input` is an approval boundary: stop first, obtain explicit user approval
for that exact proposal, and pass its auditable reference. An optional
`--branch` must exactly equal the proposal. Creation writes a recoverable intent,
disables repository hooks, creates/switches only that local branch at the exact
captured base, then binds it after post-state verification. Follow returned
errors; never delete refs, locks, or intent files to force a retry.

Use `mino git commit --plan <id> --task <task-id> --format json --no-input`
only when returned by Agent context for the first Done task with a pending
required commit gate. It is already bounded by the current plan approval and
Approved Git Flow consent; do not request a second generic approval or rebuild
the command. Mino refuses pre-staged, mixed, unrelated, or out-of-scope changes,
then journals snapshots, stages exact task paths, runs normal commit hooks with
the exact planned message, verifies the commit, and records Commit evidence plus
the gate. On a hook/Git failure, preserve the staged state, use the returned
`exec resume` action, and retry the exact commit argv. Never reset, unstage,
delete journals, use `--no-verify`, or create a replacement commit manually.

## Advisory repository hooks

Inspect the two optional default hooks without mutation:

```text
mino git hook propose --format json --no-input
mino git hook status --format json --no-input
```

Present every hook state, actual/template digest, blocker, manual integration
snippet, and the exact proposal hash. User-owned hooks, symbolic links,
unsupported file kinds, and custom `core.hooksPath` are never overwritten.
Before installation, stop for explicit approval of the current hash, then run:

```text
mino git hook install --proposal-hash <sha256> --approval-ref <ref> --format json --no-input
```

Hook installation is independent of plan approval and Git Flow consent. Exact
retries are idempotent; ownership/config/template changes require a new
proposal. Installed pre/post commit hooks invoke `git hook run`, tolerate errors,
and remain advisory. Runtime may emit diagnostics and read-only next actions,
but it never writes Git, plan, event, evidence, binding, or hook state and must
not be treated as workflow completion.

## Standards conflicts

When validation or Agent context reports a standards conflict, run the returned
`standards conflict list` or `refresh` argv and present every candidate, source,
precedence, and digest. Precedence is user requirement, repository rule,
project configuration, language package, then Common; it supplies display
order only and never authorizes an implicit selection.

`standards.conflict.resolve` is an approval boundary. Stop until the user
selects one exact current candidate and supplies a rationale plus auditable
decision reference, then execute only the returned resolve shape with those
values. Re-read context afterward. If source bytes change, the decision is
stale and must be refreshed and explicitly made again.

## Initial creation

Only when the user explicitly requests formal or durable planning and context
has no active plan, either create a new plan or explicitly import a supplied
legacy managed Markdown plan. For import, invoke:

```text
mino project import legacy --source <legacy-plan.md> --name <stable-name> --request-id <uuid> --actor <actor> --format json --no-input
```

Review every returned mapping, warning, missing field, and authored command or
path. The result must remain Draft; never infer lifecycle, approval, check,
commit, review, or evidence state from the source, and never finalize it without
explicit review. The source must remain byte-identical.

For a new plan, create a UTF-8 request file containing the exact request, then
invoke:

```text
mino plan create --name <stable-name> --trigger durable --request-file <path> --request-id <uuid> --actor <actor> --format json --no-input
```

After creation, use only `next_actions`; do not hand-author the plan JSON or
managed Markdown. Use a fresh UUID for each distinct mutation and the current
`--expect-revision`. Reuse a UUID only for a byte-equivalent retry.

## Evidence and execution

Prefer a returned `exec check run` action for planned commands because it
captures bounded process output and immutable evidence. Use returned
checkpoint, criterion, complete, block, resume, and finish actions in order.
When a completed task has a required commit gate, execute its returned
`git commit` action before starting the next task or finishing the plan.
Use `evidence add` only for supplemental files or observations that are not a
planned check. Never mark a criterion or task complete without the evidence
references required by the current plan.

## Plan alternatives

Create an independent Draft from one exact retained revision with:

```text
mino plan fork --plan <source-id> --from-revision <revision> --name <stable-name> --reason <reason> --request-id <uuid> --actor <actor> --format json --no-input
```

The source chain must audit cleanly. The fork retains authored values and
lineage but resets lifecycle, approvals, review, execution, evidence, commit
results, extensions, archive state, and current Git readiness. Exact retries
reuse the UUID and identical argv; a name collision is not a retry.

Compare alternatives without mutation using:

```text
mino plan diff --left <plan-a> --right <plan-b> --format json --no-input
```

Add `--left-revision` or `--right-revision` to select retained snapshots. Read
the directional Added/Removed/Changed/Moved entries from `mino.plan-diff/v1`.
There is no plan merge command. A plan fork is not a Git branch and does not
authorize `git branch create`.

After the user explicitly selects an alternative, deactivate an unselected plan
without deletion using:

```text
mino plan archive --plan <id> --expect-revision <revision> --reason <reason> --approval-ref <ref> --request-id <uuid> --actor <actor> --format json --no-input
```

Archive is an approval boundary. It preserves lifecycle status, snapshots, and
events while excluding the plan from active selection and blocking fresh
mutations.

## Review and rework

In Review, use `review record` only for exact user feedback and select the
minimum matching classification. Acceptance Defect and In-Scope Rework require
the completed origin task. Start only a returned Acceptance Defect rework argv;
for In-Scope Rework, supply a strict complete YAML task for the already-reserved
`R<n>` identifier, including dependencies, steps, File Map, criteria, checks,
and the required Git gate. After execution returns to Review, use the returned
`review resolve` action so Mino revalidates current evidence.

Follow-Up never enters implementation order. Material Change blocks pending a
protected amendment and must not be passed to `exec resume`. `review accept` is
a separate approval boundary: stop, obtain explicit acceptance of the current
resolved Review, and pass its auditable `--approval-ref`.

## Protected amendments

For Ready or In Progress changes, create a strict YAML patch and run
`mino plan amend propose --plan <id> --reason <reason> --patch-file <path>
--expect-revision <revision> --request-id <uuid> --actor <actor> --format json
--no-input`. Use only typed operations advertised by the CLI. Minor proposals
can be applied by their returned action. A Material proposal makes context an
approval stop; never invoke `plan amend approve` until the user approves that
exact `C<n>` change and supplies an approval reference. Then apply only the
returned action. Re-read context after each mutation. Pending proposals block
execution, evidence addition, and Git commits; applied stale evidence cannot
satisfy current gates.

## Result handling

- `ok: true` means the command completed; `complete: false` still means more
  protocol work or integration is required.
- `next_actions` is the canonical remediation/action set.
- Exit 2 is incomplete or invalid input; correct only the located fields.
- Exit 3 is a revision/request conflict; refresh context before retrying.
- Exit 4 requires explicit approval; stop.
- Exit 5 is a policy refusal; do not bypass it.
- Exit 6 is an execution or verification failure; record/block as directed.
- Exit 7 is environment unavailable; report the dependency or environment.
- Exit 8 is drift/corruption; preserve bytes and stop for diagnosis.
