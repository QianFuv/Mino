# Mino command and JSON contract

This document is the authoritative implemented CLI inventory. The v0.1 plan
protocol remains stable while explicitly labeled v0.2 Git, review, standards,
and plan-variant surfaces are added.
`mino --help` remains the source for individual argument spelling and value
choices.

## Invocation rules

Global options are accepted before or after subcommands:

| Option | Meaning |
|---|---|
| `--root <path>` | Start project discovery at a file or directory. Defaults to the current directory. |
| `--format human|json` | Select one-line human output or versioned machine output. Defaults to `human`. |
| `--no-input` | Prohibit interactive input. Required for all `agent` commands. |

Agent integrations must use both `--format json` and `--no-input`. Each output
is one UTF-8 JSON value followed by a newline. Successful machine output uses
stdout; JSON failures also use stdout so callers can parse them; diagnostics
that cannot form a Mino result use stderr. Never merge the streams.

Creating a plan with `plan create` or `plan fork` does not use
`--expect-revision`; fork instead binds creation to an exact retained
`--from-revision`. Both require a UUID `--request-id`. Mutations against an
existing plan or its evidence require a current `--expect-revision` and a UUID
`--request-id`; authored mutations also identify `--actor`. Use a fresh UUID
for each distinct mutation. Reuse it only for an exact retry. After a successful mutation,
  discard the old revision and read context again. `git bind`, `git branch
  create`, and `git commit` do not accept caller-selected plan mutation
  arguments. Binding and branch creation do not change a plan revision; branch
  creation has its own explicit approval reference. Task commit derives
  idempotent evidence/plan request IDs from its immutable journal and records the
  resulting gate revision internally.

## Complete command inventory

### Project and protocol

| Command | Mutation | Contract |
|---|---:|---|
| `mino project init` | Filesystem | Create missing `.mino` files and install/verify the Skill; block application is opt-in. |
| `mino project show` | No | Return parsed config/locks plus doctor findings. |
| `mino project doctor` | No | Diagnose locks, transactions, projections, Skill, and managed blocks. |
| `mino project scan` | No | Return ignore-aware workspace/language evidence. |
| `mino project migrate legacy` | No | Analyze supplied AGENTS/template/execution files and propose mappings without writes. |
| `mino project import legacy` | New Draft only | Require `--source`, `--name`, and `--request-id`; preserve the source, map supported authored fields through normal create/apply APIs, and report all ignored or unsupported content. |
| `mino protocol status` | No | Verify embedded resource digests and project-lock compatibility. |
| `mino protocol migrate` | Plan when a transform exists | Require explicit target/revision/request ID. v0.1 only supports an already-current no-op; other targets fail without writes. |

`project init --apply-agents-block` and
`project init --apply-gitignore-block` modify only their owned marker regions.
`project import legacy` refuses an existing active plan or name collision. A
successful import returns `complete: false`, `draft_review_required: true`,
`source_preserved: true`, and `historical_execution_trusted: false`; lifecycle,
approval, check-result, commit-result, and evidence assertions never enter the
new aggregate. Reuse the same source bytes, name, actor, and request UUID only
for an exact two-phase retry.

### Git inspection, active binding, branches, and task commits

| Command | Mutation | Contract |
|---|---:|---|
| `mino git inspect` | No | Return repository, common-directory, worktree, Git-directory, index, HEAD, upstream, porcelain-v2 status, and active-binding facts; optional `--plan` verifies one plan and reports whether it is bound. |
| `mino git bind` | `.mino/active.json` only | Require `--plan <id> --current`; bind one non-Done plan to the canonical current worktree plus branch, or to the exact detached HEAD. Exact retries preserve the original bytes. |
| `mino git branch propose` | No | Derive `mino/<plan-id>`, validate it with Git, and report clean/base/source/existing-ref blockers without writing Git or Mino state. |
| `mino git branch create` | Local branch + Mino journal/binding | Require `--plan <id> --approval-ref <ref>`; optional `--branch` must exactly equal the proposal. Recheck the clean worktree, captured branch/detached mode, and base HEAD before creating and switching. |
| `mino git commit` | Exact index paths + one local commit + evidence/plan/journal | Require `--plan <id> --task <id>`. Execute only the first Done task with a pending required gate under current plan approval and Approved Git Flow consent, exact same-worktree binding/branch/parent, compatible evidence, and changed paths inside both File Map and Commit Scope. |
| `mino git hook propose` | No | Inspect default pre/post commit paths, ownership markers, template/actual digests, custom hook configuration, and return one stable proposal hash. |
| `mino git hook status` | No | Return the same bounded ownership/content facts without installing or invoking a hook. |
| `mino git hook install` | Hook files only, approval boundary | Require the current `--proposal-hash` and non-empty `--approval-ref`; install/repair only absent or Mino-owned default hooks, then verify exact bytes. |
| `mino git hook run` | No | Require `--hook pre-commit|post-commit`; observe staged/HEAD and active-binding facts with read-only Git commands and emit diagnostics/next actions. |

Binding status is one of `missing`, `current`, `foreign_worktree`,
`stale_branch`, `stale_head`, or `not_repository`. Once an active binding file
exists, a foreign or stale binding never falls back to a plan from another
worktree or branch. An explicit bind replaces only the current worktree's prior
binding and preserves entries for other worktrees. `git inspect` does not
modify Git or Mino state. `git bind` does not stage files or change HEAD,
branches, refs, the index, or commits.

`git branch create` is an Agent approval boundary. Before invoking it, the
caller must obtain explicit user authorization and pass its auditable reference;
plan approval or Git Flow consent is not a substitute. Mino publishes an
immutable `.mino/git/branches/<plan-id>/intent.json` before Git, disables
repository hooks for the switch, binds only after the exact branch/base state
is observed, and then publishes `completion.json`. An exact retry either
retries an unchanged failed source state, reconciles an already-created branch,
or replays the completed result without a second mutation.

`git commit` is not a new approval boundary: the current plan approval and its
Approved Git Flow consent authorize only the gate's exact one-line message and
resolved task paths. Mino refuses any pre-existing staged path, mixed
index/worktree content, undeclared/out-of-scope path, unsupported submodule,
symlink, rename, clean filter, branch, or parent drift before mutation. It writes
`.mino/git/commits/<plan-id>/<task-id>/intent.json` before `git add -- <exact
paths>`, records `staged.json`, runs normal repository commit hooks, verifies the
new parent/tree/message/file set, records immutable Commit evidence and the plan
gate, then publishes `completion.json`. A hook/Git failure leaves exact staged
state visible and blocks the plan; after `exec resume`, an exact retry recovers
the staged or already-created commit without duplicating it. Mino never invokes
`--no-verify` or automatically resets/unstages a failed attempt.

Repository hooks are optional advisory integrations. `git hook propose` binds
the canonical worktree/common-directory identity, both target paths, current
ownership classes, conflict digests, and embedded template digests. Safe
Absent, Current, and Mino-Owned-Drifted states share an idempotent install class;
a user hook or custom `core.hooksPath` changes/refuses the proposal. `git hook
install` is a separate approval boundary and never treats plan approval or Git
Flow consent as a substitute. It preserves user hooks byte-for-byte and returns
the exact manual snippet instead of composing or overwriting them.

The installed `pre-commit` and `post-commit` scripts carry the exact
`mino-managed-hook:v1` marker, invoke only `mino git hook run`, tolerate an
unavailable/refusing Mino, and exit successfully. Runtime inspection disables
optional Git locks, reads staged/HEAD and binding facts, and never invokes a Git
mutation or writes plan, event, evidence, binding, or hook state.

### Plan authoring and approval

| Command | Mutation | Contract |
|---|---:|---|
| `mino plan create` | Yes | Create a revision-one Draft from an explicit request file/stdin or bounded interactive wizard. |
| `mino plan next` | No | Return deterministic missing fields and canonical remediation. |
| `mino plan validate` | No | Run schema, semantic, graph, and policy validation in fixed order. |
| `mino plan show` | No | Return the complete verified source-of-truth plan. |
| `mino plan finalize` | Yes | Validate a complete Draft and transition it to Ready. |
| `mino plan review` | No | Return the revision/hash-bound approval summary for a Ready plan. |
| `mino plan approve` | Yes, approval boundary | Record an explicit plan approval and an Approved or Disabled Git Flow consent decision. |
| `mino plan apply` | Yes | Strictly apply one bounded YAML Draft document; unknown fields are rejected. |
| `mino plan amend propose` | Yes | Record one strict typed patch, immutable base revision/hash, classifier-derived impact, and monotonic `C<n>` ID. A caller may raise but never lower the minimum classification. |
| `mino plan amend approve` | Yes, approval boundary | Require `--change C<n> --approval-ref <ref>` and record explicit approval only for the current pending Material proposal. |
| `mino plan amend apply` | Yes | Apply the current eligible proposal atomically. Minor invalidates only affected checks/evidence; Material resets execution gates, supersedes affected review state, clears plan approval/Git consent, and returns Ready. |
| `mino plan fork` | New Draft only | Require `--plan`, exact positive `--from-revision`, new `--name`, `--reason`, request ID, and actor; audit source history, copy authored values, record lineage/hash, and reset all execution/trust bindings. |
| `mino plan diff` | No | Compare `--left` and `--right` at current or optional exact retained revisions; return stable Added/Removed/Changed/Moved authored paths under `mino.plan-diff/v1`. |
| `mino plan archive` | Yes, approval boundary | Require current revision/request ID, `--reason`, and auditable `--approval-ref`; record semantic deactivation without deleting bytes or changing lifecycle status. |
| `mino plan metadata set` | Yes | Replace supplied Draft metadata fields. |
| `mino plan summary set` | Yes | Replace the Draft summary from an argument or stdin. |
| `mino plan context add` | Yes | Append one current-state reference/fact/implication. |
| `mino plan scope set` | Yes | Replace supplied goal/deliverable/in-scope/out-of-scope fields. |
| `mino plan scope add` | Yes | Append one value to one scope list. |
| `mino plan decision add` | Yes | Append one decision, assumption, or question. |
| `mino plan task add` | Yes | Append the next deterministic task and optional commit gate. |
| `mino plan task step add` | Yes | Append one ordered task implementation step. |
| `mino plan task criterion add` | Yes | Append the next deterministic acceptance criterion. |
| `mino plan task verification add` | Yes | Append one task-scoped planned command. |
| `mino plan file add` | Yes | Append one task-owned File Map responsibility. |
| `mino plan verification add` | Yes | Append one global planned command. |

Direct authored changes are legal only in Draft. Ready and In Progress changes
must use the typed amendment protocol; arbitrary JSON/field paths and execution
fields are rejected. Minor permits only test fixtures, barrel exports,
snapshots, task-local support files, verification-command corrections, and
implementation notes. User-visible behavior, public API, data/schema,
dependencies, compatibility, scope, security, and core task order are Material.
Ready changes clear the current plan approval. Material application clears plan
approval and Git consent, resets task/global gates, removes execution-only
checkpoints, marks affected evidence stale, and requires validation plus a new
review/approval cycle. Approval records are auditable declarations, not
cryptographic signatures.

`plan fork` reads only an audited immutable source snapshot before publishing a
separate revision-one Draft. It retains authored request, scope, decisions,
standards, tasks, checks, and commit intent, but resets lifecycle, approvals,
amendments, review/follow-up state, all evidence/status/result bindings,
execution extensions, final outcome, current Git readiness, and archive state.
Its lineage records the source plan, exact revision, reason, canonical snapshot
hash, and fork timestamp. A name collision fails before mutation unless the
request is an exact replay.

`plan diff` normalizes only authored values and never writes either plan. It
excludes plan identity, lifecycle, Git readiness, approvals, amendments,
review, evidence, execution, lineage, archive, final outcome, and extensions.
Direction is explicit: Added on left-to-right becomes Removed in the reverse,
and before/after values swap. There is no plan merge command. A plan fork is a
plan-history operation and is independent of `git branch create`.

`plan archive` is a user-selection boundary, not a lifecycle state or delete.
It appends reason, actor, approval reference, and timestamp in a new revision,
preserves all snapshots/events/projections, and excludes that plan from active
selection. An archived plan rejects fresh semantic mutations; exact retries
remain idempotent.

### Review and rework

| Command | Mutation | Contract |
|---|---:|---|
| `mino review record` | Yes | Record exactly one Acceptance Defect, In-Scope Rework, Material Change, or Follow-Up item. Task classifications require a completed `--task`; other classifications reject one. |
| `mino review rework` | Yes | Reopen an Acceptance Defect task for evidence-only execution, or materialize the reserved `R<n>` task from one strict complete YAML definition. |
| `mino review resolve` | Yes | Resolve an In Progress rework item only after all current task, commit, global-check, evidence, and deviation gates pass. |
| `mino review accept` | Yes, approval boundary | Require `--approval-ref`, all feedback resolved/deferred, and all live evidence valid; append an Accepted record and transition Review to Done. |

Review item IDs are contiguous `REV-n` values. In-Scope Rework reserves a
monotonic `R<n>` task ID at record time; an invalid or rejected task definition
never releases that ID. An R task enters implementation order only when its
dependencies are Done and its steps, File Map, criteria, checks, and required
Git commit gate are complete. Acceptance Defect retains the prior committed
gate, requires fresh evidence, and refuses changed files. Material Change moves
the plan to a Review-owned Blocked state; `exec resume` cannot bypass it.
Follow-Up is Deferred and never changes task order or a Git gate.

### Standards

| Command | Mutation | Contract |
|---|---:|---|
| `mino standards detect` | No | Return supported languages from scanner evidence. |
| `mino standards recommend` | No | Recommend Common plus applicable language packages, optionally for File Map paths. |
| `mino standards apply` | No | Resolve exact packages/rules/checks; v0.1 requires `--recommended --seed-verification`. |
| `mino standards sync` | Cache/lock | Explicitly fetch and activate a digest-verified catalog; v0.1 requires `--all`. |
| `mino standards conflict list` | No | Show every conflicting candidate, source class, precedence, source digest, and current decision status. |
| `mino standards conflict refresh` | Plan | Snapshot the exact live candidate set without selecting a value; source changes clear stale decisions. |
| `mino standards conflict resolve` | Plan, approval boundary | Select one current candidate with rationale and an auditable decision reference. |

Detection, recommendation, and apply are offline over embedded or already
cached packages. Only sync uses the configured network catalog. Conflict
precedence is user requirement, repository rule or local declaration, project
configuration, language package, then Common. A unique highest-precedence
candidate is displayed as a default, never applied as an implicit merge or
decision. Validation remains blocked until every exact current conflict has an
explicit decision.

### Agent API

| Command | Contract |
|---|---|
| `mino agent context` | Complete dynamic project/active-plan state, allowed/blocked actions, approval boundary, and canonical next argv. |
| `mino agent next` | Focused active-plan, approval, blocked-action, and next-action view. |
| `mino agent capabilities` | Static implemented action inventory and invocation/mutation requirements. |

These commands return their direct schemas, not a `mino.result/v1` wrapper.
They fail with exit 5 unless JSON and no-input mode are both selected.

### Evidence

| Command | Mutation | Contract |
|---|---:|---|
| `mino evidence add` | Evidence only | Add immutable File, GitDiff, Commit, Url, Log, Screenshot, ManualObservation, or AcceptedException evidence; Command evidence is runner-owned. |
| `mino evidence list` | No | List records in monotonic evidence-ID order with optional task/type filters. |
| `mino evidence show` | No | Return one exact immutable record. |

Artifact paths must remain within the project. A correction creates a new
record with `supersedes`; records and blobs are never rewritten. Evidence
invalidated by an applied amendment remains immutable history but cannot
satisfy completion. No evidence may be added while a proposal awaits apply.

### Execution

| Command | Mutation | Contract |
|---|---:|---|
| `mino exec start` | Yes | Start only the first eligible Ready task after plan approval and after every prior required task commit is recorded. |
| `mino exec checkpoint` | Yes | Record a typed checkpoint for the active task. |
| `mino exec check run` | Yes + process/evidence | Run one planned task/global check with durable lease/result and evidence attachment. |
| `mino exec check monitor` | Yes + bounded processes/evidence | Re-run one existing planned check in the foreground under required attempt, interval, and elapsed-deadline bounds; stop on pass, attempt exhaustion, deadline, or an optional safe cancellation file. |
| `mino exec criterion pass` | Yes | Bind one compatible immutable evidence record to one active criterion. |
| `mino exec complete` | Yes | Complete the active task after check, evidence, deviation, checkpoint, and File Map gates. |
| `mino exec block` | Yes | Block a Ready/In Progress plan with a non-empty resumable reason. |
| `mino exec resume` | Yes | Restore the exact recorded Ready/In Progress state. |
| `mino exec finish` | Yes | Require all tasks, required task commit gates, and global checks complete, then transition In Progress to Review. |

`exec check monitor` requires `--max-attempts 1..=100`,
`--interval-milliseconds 1..=60000`, and
`--deadline-milliseconds 1..=86400000`. The deadline must leave at least one
millisecond of process budget per attempt after every possible interval. Mino
divides that remaining budget across attempts, caps each check at five minutes,
and retains the existing 1 MiB output bound. `--cancel-file`, when supplied,
must name a project-relative regular file with an existing in-project parent.
Its presence is checked only between attempts; no watcher or background service
is created.

Each attempt uses deterministic child request IDs, advances the plan through
the normal two check revisions, and records ordinary immutable command
evidence. The first terminal condition is persisted canonically at
`.mino/plans/<plan-id>/monitors/<request-id>/summary.json`. The summary is bound
to the complete request hash and published without replacing an existing file.
An exact retry returns it without executing, sleeping, or mutating state;
different inputs under the same request ID return exit 3. A passing terminal
reason returns exit 0. Attempt exhaustion, deadline, and cancellation return
exit 6 with the complete `monitor` report and all completed attempt evidence.

The execution commands themselves perform no Git mutation. The v0.2 Git
surfaces can write binding/journal state, create/switch only the deterministic
explicitly approved local branch, and create only an eligible plan-scoped task
commit from exact paths. They do not push, merge, rebase, reset, amend,
force-push, tag, delete a branch, or create/delete a worktree.

## Success envelope

Non-Agent commands in JSON mode flatten their command payload into this stable
envelope:

```json
{
  "kind": "mino.result/v1",
  "ok": true,
  "complete": false,
  "message": "Plan draft initialized.",
  "plan_id": "2026-07-25-example",
  "revision": 1,
  "missing": ["summary"],
  "next_actions": [
    {"id": "plan.summary.set", "argv": ["mino", "plan", "summary", "set", "..."]}
  ]
}
```

`ok` reports whether the command succeeded. `complete` reports whether the
requested workflow has remaining protocol work; `ok: true, complete: false` is
normal. `missing` contains stable locations/codes. `next_actions[].argv` is a
complete argument vector, including executable name, and must not be rebuilt as
a shell string.

Failures use the same result kind:

```json
{
  "kind": "mino.result/v1",
  "ok": false,
  "complete": false,
  "message": "Plan revision is stale.",
  "error": {"code": "revision_conflict", "exit_code": 3},
  "missing": [],
  "next_actions": []
}
```

Command-specific structured details may be flattened into a failure without
overwriting the common keys.

## Stable schema identifiers

| Identifier | Produced by |
|---|---|
| `mino.result/v1` | All non-Agent success/failure envelopes |
| `mino.agent-context/v1` | `agent context` |
| `mino.agent-next/v1` | `agent next` |
| `mino.agent-capabilities/v1` | `agent capabilities` |
| `mino.validation/v1` | Plan validation details |
| `mino.plan-review/v1` | Revision-bound `plan review` payload |
| `mino.check-run/v1` | Persisted check lease/result `schema_version` |
| `mino.monitor/v1` | Immutable terminal `exec check monitor` summary under `monitor_kind` |
| `mino.plan-diff/v1` | Read-only semantic `plan diff` payload under `diff_kind` |
| `mino.git-hook-status/v1` | Read-only `git hook status` and proposal status payload |
| `mino.git-hook-proposal/v1` | Hash-bound `git hook propose` payload |
| `mino.git-hook-install/v1` | Approval-bound `git hook install` result |
| `mino.git-hook-runtime/v1` | Read-only `git hook run` observation |

The plan aggregate also carries numeric `schema_version: 1`; the protocol lock
binds protocol and renderer versions separately.

## Exit codes

| Exit | JSON code | Meaning | Required response |
|---:|---|---|---|
| 0 | N/A | Command succeeded, even if `complete` is false | Read payload/next actions. |
| 2 | `incomplete_or_validation` | Missing input or deterministic validation failure | Correct only reported fields. |
| 3 | `revision_conflict` | Stale revision or conflicting request UUID | Refresh current state before retry. |
| 4 | `approval_required` | Explicit user approval is required | Stop; do not approve on the user's behalf. |
| 5 | `policy_violation` | Illegal transition, unsafe action, or policy refusal | Do not bypass the gate. |
| 6 | `check_failed` | Planned process did not meet its expected result | Preserve evidence and block/remediate as directed. |
| 7 | `environment_unavailable` | Required file, tool, service, lock, or environment failed | Report the unavailable dependency/state. |
| 8 | `drift_detected` | Canonical state, lock, immutable record, or managed bytes disagree | Preserve bytes and diagnose/recover. |

Clap syntax errors also exit 2 and may emit diagnostics rather than a Mino JSON
envelope because command dispatch did not begin.

## State strings

Plan states are `Draft`, `Ready`, `In Progress`, `Blocked`, `Review`, and
`Done`. Task states are `Draft`, `Ready`, `In Progress`, `Blocked`, and `Done`.
Check states are `Pending`, `Running`, `Passed`, `Failed`, and `Blocked`.
Criterion states are `Pending`, `Passed`, `Failed`, and `Accepted Exception`.
Git Flow consent is `Pending`, `Approved`, or `Disabled`.
Amendment classifications are `Minor` and `Material`; amendment states are
`Proposed`, `Approval Required`, `Approved`, and `Applied`.

Active binding states are `missing`, `current`, `foreign_worktree`,
`stale_branch`, `stale_head`, and `not_repository`.

Only transitions exposed by the command inventory are implemented promises.
No command accepts an arbitrary status value: every status change is a named
semantic transition with state-specific preconditions.
