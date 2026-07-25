# Mino command and JSON contract

This document is the authoritative implemented CLI inventory. The v0.1 plan
protocol remains stable while explicitly labeled v0.2 Git surfaces are added.
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

Except for initial `plan create`, mutations against an existing plan or its
evidence require a current `--expect-revision` and a UUID `--request-id`;
authored mutations also identify `--actor`. Use a fresh UUID for each distinct
mutation. Reuse it only for an exact retry. After a successful mutation,
discard the old revision and read context again. `git bind` and `git branch
create` do not change a plan revision and therefore do not use the plan
mutation arguments; branch creation has its own explicit approval reference
and immutable prepared-intent journal.

## Complete command inventory

### Project and protocol

| Command | Mutation | Contract |
|---|---:|---|
| `mino project init` | Filesystem | Create missing `.mino` files and install/verify the Skill; block application is opt-in. |
| `mino project show` | No | Return parsed config/locks plus doctor findings. |
| `mino project doctor` | No | Diagnose locks, transactions, projections, Skill, and managed blocks. |
| `mino project scan` | No | Return ignore-aware workspace/language evidence. |
| `mino project migrate legacy` | No | Analyze supplied AGENTS/template/execution files and propose mappings without writes. |
| `mino protocol status` | No | Verify embedded resource digests and project-lock compatibility. |
| `mino protocol migrate` | Plan when a transform exists | Require explicit target/revision/request ID. v0.1 only supports an already-current no-op; other targets fail without writes. |

`project init --apply-agents-block` and
`project init --apply-gitignore-block` modify only their owned marker regions.

### Git inspection, active binding, and branch creation

| Command | Mutation | Contract |
|---|---:|---|
| `mino git inspect` | No | Return repository, common-directory, worktree, Git-directory, index, HEAD, upstream, porcelain-v2 status, and active-binding facts; optional `--plan` verifies one plan and reports whether it is bound. |
| `mino git bind` | `.mino/active.json` only | Require `--plan <id> --current`; bind one non-Done plan to the canonical current worktree plus branch, or to the exact detached HEAD. Exact retries preserve the original bytes. |
| `mino git branch propose` | No | Derive `mino/<plan-id>`, validate it with Git, and report clean/base/source/existing-ref blockers without writing Git or Mino state. |
| `mino git branch create` | Local branch + Mino journal/binding | Require `--plan <id> --approval-ref <ref>`; optional `--branch` must exactly equal the proposal. Recheck the clean worktree, captured branch/detached mode, and base HEAD before creating and switching. |

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

Direct authored changes are legal only in Draft. Ready plans require explicit
approval before execution. The approval is an auditable declaration, not a
cryptographic signature. v0.1 stops at Review; there is no `review` command.

### Standards

| Command | Mutation | Contract |
|---|---:|---|
| `mino standards detect` | No | Return supported languages from scanner evidence. |
| `mino standards recommend` | No | Recommend Common plus applicable language packages, optionally for File Map paths. |
| `mino standards apply` | No | Resolve exact packages/rules/checks; v0.1 requires `--recommended --seed-verification`. |
| `mino standards sync` | Cache/lock | Explicitly fetch and activate a digest-verified catalog; v0.1 requires `--all`. |

Detection, recommendation, and apply are offline over embedded or already
cached packages. Only sync uses the configured network catalog.

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
record with `supersedes`; records and blobs are never rewritten.

### Execution

| Command | Mutation | Contract |
|---|---:|---|
| `mino exec start` | Yes | Start only the first eligible Ready task after plan approval. |
| `mino exec checkpoint` | Yes | Record a typed checkpoint for the active task. |
| `mino exec check run` | Yes + process/evidence | Run one planned task/global check with durable lease/result and evidence attachment. |
| `mino exec criterion pass` | Yes | Bind one compatible immutable evidence record to one active criterion. |
| `mino exec complete` | Yes | Complete the active task after check, evidence, deviation, checkpoint, and File Map gates. |
| `mino exec block` | Yes | Block a Ready/In Progress plan with a non-empty resumable reason. |
| `mino exec resume` | Yes | Restore the exact recorded Ready/In Progress state. |
| `mino exec finish` | Yes | Require all tasks/global checks complete and transition In Progress to Review. |

The v0.1 execution commands perform no Git mutation. The v0.2 Git surfaces can
write Mino binding/journal state and can create/switch only the deterministic
approved local branch. They do not stage, commit, push, merge, rebase, reset,
amend, force-push, tag, delete a branch, or create/delete a worktree.

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

Active binding states are `missing`, `current`, `foreign_worktree`,
`stale_branch`, `stale_head`, and `not_repository`.

Only transitions exposed by the command inventory are implemented promises. In
particular, Review-to-Done and review rework are not current CLI operations.
No command accepts an arbitrary status value: every status change is a named
semantic transition with state-specific preconditions.
