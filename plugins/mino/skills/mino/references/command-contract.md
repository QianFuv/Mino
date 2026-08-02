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
parsing. Every `next_actions[].id` must also appear in `allowed_actions`; never
insert a state-changing command that context did not return. Re-read context
after each successful mutation.

## Git identity and active binding

Agent context includes canonical Git worktree identity, branch or detached
HEAD, cleanliness, staged/unstaged paths, and active-binding status when the
project is in a Git worktree. Project plan selection is independent of Git
binding; a stale or foreign binding does not silently select another plan.
`current` is the only binding status that permits a Git-identity-gated action.
When Approved Git Flow is ready to start or commit and binding is not current,
execute the returned `git.bind` argv, refresh context, and wait for Mino to
return `exec.start` or `git.commit`.

Agent context uses `git: null` only when Git explicitly confirms that the
project is not a repository. A missing executable, timeout, permission or
metadata failure, or invalid machine-readable output returns a typed non-zero
error and blocks action generation. The error does not echo raw Git output.

Use `mino git inspect --plan <id> --format json --no-input` for a complete
read-only relationship report. Use
`mino git bind --plan <id> --current --format json --no-input` only to bind the
named non-Done plan to the exact current worktree and branch or detached HEAD.
Binding writes `.mino/active.json`; it does not stage, commit, switch branches,
or otherwise mutate Git. Exact repeated binds are idempotent; binding another
plan explicitly replaces only the current worktree's prior selection.

Plan creation and fork persist a typed live Git observation. Before finalize,
review, approval, execution start, or branch creation, Mino compares current
repository mode, canonical worktree/common directory, branch, full HEAD, and
status with that observation. On drift, execute only the returned revisioned
`mino git readiness refresh --plan <id> --expect-revision <revision>
--request-id <uuid> --actor codex --format json --no-input` argv. A Ready-plan
refresh preserves Ready but clears the old plan approval, Git Flow consent, and
workspace baseline. Exact retries replay without observing Git again. Commit
preflight compares repository identity and branch without requiring a clean
tree, so planned task dirt remains eligible while parent, index, scope, and
checked blob rules still apply. A legacy plan without typed readiness must also
refresh before a protected transition.

Missing repositories and dirty worktrees require explicit, auditable decisions.
When context returns `git.setup.decide`, use the exact revisioned argv and one
of `initialize-approved`, `continue-without-git`, or
`blocked-until-manual-setup`. The first choice records permission only: Mino
never runs `git init`. For dirty worktrees, create a strict YAML proposal for
the returned `git.cleanup.propose` action. Ordered `C<n>` items must have
disjoint files that exactly cover every observed dirty path and one-line
Conventional Commit messages. Approve each item only at the returned
`git.cleanup.approve` boundary, or explicitly decline cleanup.

Create any approved cleanup commit outside Mino under the repository's own
authorization, then execute the returned `git.cleanup.record` argv with the
full current-HEAD object ID. Record verifies exact order, parent, message, and
files; it never stages or commits. After every item is recorded and the live
tree is clean, execute `git.readiness.refresh` to mark cleanup complete and
recompute Git Flow eligibility. Unsafe Git state or overlap between a dirty
path and the task File Map remains recoverably Blocked until external repair
and refresh.

## Durable planning authority

When doctor or Agent context returns `project.authority.status`, inspect that
read-only status before creating a durable plan. Mino scans only active
Formal Plan Trigger, Pinned Gist/External Resource, Plan Review Gate, and Plan
Execution clauses outside fenced examples. A pending or stale conflict between
those clauses and the Mino workflow block blocks durable creation.

Use `mino project authority propose --format json --no-input` to obtain the
exact source digest, authority revision, replacement digest, and section range
without writing. At the explicit approval boundary, either record
`coexistence-approved` or `declined` with `project authority decide`, or run
`project authority apply --apply-rewrite` with the exact proposal digests.
Coexistence makes Mino the sole durable-workflow owner while the legacy text is
inert reference; declined prevents Mino durable creation. Apply replaces only
one detected Planning Documents section and records superseded only after the
guarded transaction publishes successfully.

Never edit `.mino/authority.json` or recovery files. Any AGENTS byte change
makes a prior decision stale; execute the returned `project.init` refresh to
bind a new pending detection without inheriting the old approval. A changed
digest, symbolic link, non-file,
oversized source, concurrent edit, or unprovable recovery state must be
preserved and re-inspected instead of overwritten. An exact interrupted apply
request may be retried through the complete recovery action returned by status;
that action reuses the persisted approval instead of inventing a new one.

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

For non-conflict `POLICY-STANDARD-*` findings, execute the returned plan-scoped
`standards apply --recommended --seed-verification --plan ...` argv. Do not
substitute read-only `standards recommend` or Draft-only `plan apply`.
Untracked/stale conflicts route to `standards conflict refresh`; unresolved
conflicts route to `list` and then an explicit approved resolution. Ready
reconciliation invalidates old plan approval, so refresh context and stop at
the new approval boundary.

`POLICY-TOOL-UNAVAILABLE` is an external environment blocker. Do not run
`standards apply` or `plan apply` for that finding. Install the required tool or
expose it through PATH/PATHEXT, then rerun `mino plan validate` or `mino agent
context` without mutating the plan.

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

The display name remains complete UTF-8. If it contains no ASCII letter or
digit, Mino derives the ASCII ID suffix `plan-<8hex>` from the first eight
lowercase hexadecimal characters of the exact UTF-8 name SHA-256; the full ID
is `YYYY-MM-DD-plan-<8hex>`. Do not transliterate or replace the user-visible
name to manufacture an ID.

## Evidence and execution

Prefer a returned `exec check run` action for planned commands because it
captures bounded process output and immutable evidence. Use returned
checkpoint, criterion, complete, block, resume, and finish actions in order.
When a completed task has a required commit gate, execute its returned
`git commit` action before starting the next task or finishing the plan.
Use `evidence add` only for supplemental files or observations that are not a
planned check. Never mark a criterion or task complete without the evidence
references required by the current plan.

Explicit File Map directories/globs override repository ignore filters for
fingerprint capture, so ignored authorized files still stale evidence when
changed. `.git/**`, `.mino/**`, managed projections, symlink escapes, unsafe
objects, and capture budgets remain protected. Git fingerprints preserve both
raw SHA-256 and the expected filtered blob OID/mode. Automatic and manual
commit recording reject clean filters and require the actual commit tree to
match every expected entry.

`exec finish`, `review resolve`, and `review accept` also compare the approved
plan baseline to the complete current project and, in Git mode, base HEAD to
current HEAD. Only File Map-compatible changes, exact paths from Resolved Minor
deviations, and Mino-owned exclusions are allowed. A passing global check does
not authorize another committed, uncommitted, ignored, or non-Git path.

For finite repeated observation of one existing check, invoke:

```text
mino exec check monitor --plan <id> --check <check-id> --expect-revision <revision> --request-id <uuid> --actor <actor> --max-attempts <1..100> --interval-milliseconds <1..60000> --deadline-milliseconds <1..86400000> --format json --no-input
```

Add `--cancel-file <project-relative-file>` only when the parent already exists
inside the project. Mino remains in the foreground, derives bounded child check
timeouts and deterministic attempt request IDs, and persists every completed
attempt plus one immutable `mino.monitor/v1` terminal summary. Reuse the base
UUID and identical argv only to recover/replay that invocation. Pass returns
success; attempts exhausted, deadline, and cancellation return exit 6 with the
complete report. Do not replace this command with loops, tails, watchers,
background processes, schedulers, manual polling, or rebuilt attempt argv.

Emit a complete external scheduled-task handoff without creating it using:

```text
mino exec schedule spec --plan <id> --expect-revision <revision> --check <check-id> --execution-request-id <uuid> --actor <actor> --execution-environment <environment> --max-attempts <1..100> --interval-milliseconds <1..60000> --deadline-milliseconds <1..86400000> --trigger-at <rfc3339> --expires-at <rfc3339> --max-dispatch-attempts <1..100> --dispatch-retry-milliseconds <1..86400000> --success-condition <text> --stop-condition <text> --failure-handling <text> --result-destination <project-relative-file> --format json --no-input
```

The expiry window must cover every bounded monitor dispatch and retry delay and
cannot exceed 31 days. The result path must use existing regular in-project
parents and cannot target `.mino/**` or `docs/plan/**`. Verify the
`mino.scheduled-task-spec/v1` digest, exact argv/revision context, explicit
outcome policy, false emission side effects, and separate-authorization fields.
This command never contacts or mutates a scheduler. Stop and obtain separate
explicit user authorization before creating or updating external scheduled
work; do not convert the handoff into a loop, daemon, or hidden API call.

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

When a selected plan has live alternatives, Agent context keeps the selected
plan's lifecycle `next_actions` and `approval_required` value. Alternatives add
`plan.alternatives`, `plan.select`, `plan.diff`, and `plan.archive` as optional
allowed actions. They become blocking only when multiple live candidates exist
without a selected plan.

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

Accept Change creates reciprocal links between the Material Review item and its
source amendment and appends an immutable decision record. If that amendment
terminates Rejected, Withdrawn, or Cancelled without applying, context may
allow `review disposition revise`. Stop for a new explicit decision reference
and reason, then revise only to Decline or Defer. Never revise a pending/applied
or replaced amendment, repeat Accept Change, or overwrite prior decision
history.

## Protected amendments

For Ready or In Progress changes, create a strict YAML patch and run
`mino plan amend propose --plan <id> --reason <reason> --patch-file <path>
--expect-revision <revision> --request-id <uuid> --actor <actor> --format json
--no-input`. Use only typed operations advertised by the CLI. Minor proposals
can be applied by their returned action, except that adding a Rust, Python, or
TypeScript/JavaScript path without the selected language package raises the
minimum to Material. Material operations may add/update/remove tasks, criteria,
task/global checks, commit gates, dependencies, definitions, and task order;
inspect the complete affected task/check/evidence set. A Material proposal
makes context an approval stop; never invoke `plan amend approve` until the
user approves that exact `C<n>` change and supplies an approval reference. Then
apply only the returned action. Re-read context after each mutation. Pending
proposals block execution, evidence addition, and Git commits; applied stale
evidence cannot satisfy current gates.

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
