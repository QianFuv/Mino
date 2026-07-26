# Mino

Mino is a local, versioned plan protocol engine for coding agents. It turns
durable implementation plans into revision-checked state, deterministic
Markdown projections, ordered execution, immutable evidence, and explicit
approval boundaries. It is not a template generator and does not contain an
LLM.

The current package is `mino 0.1.0`. It contains the stable v0.1 lifecycle plus
the v0.2 local Git, review, and protected-amendment increments: worktree-aware plan binding,
explicitly approved branch creation, exact plan-scoped task commits, classified
review feedback, rework, final acceptance, typed Minor/Material plan changes,
conservative legacy-plan import, and explicit source-bound standards-conflict
decisions, historical plan forks, semantic plan diffs, and approval-bound
non-destructive archive state. Scheduled observation, team catalogs, and
plugin distribution remain deferred. Optional advisory pre/post commit hooks
are also available with explicit installation approval.

## What v0.1 provides

- Recoverable canonical plan JSON, immutable revision snapshots, and an
  append-only event log.
- Deterministic, drift-protected `docs/plan/*.md` projections.
- Strict Draft authoring, validation, revision-bound review, and explicit plan
  approval.
- Ordered task execution, bounded no-shell checks, checkpoints, blocking and
  resume, acceptance evidence, task completion, and finish-to-Review.
- Immutable supplemental evidence with content-addressed blobs and redaction.
- Project discovery, diagnostics, embedded standards, explicit standards
  synchronization, and a digest-verified planning protocol bundle.
- A repository-level `$mino` Skill plus opt-in managed blocks for `AGENTS.md`
  and `.gitignore`.
- Stable JSON Agent context, next-action, and capability contracts.

The v0.2 Git increment adds strict repository/worktree inspection, explicit
active-plan binding, recoverable branch intents, and task commits constrained by
Approved Git Flow consent, File Map, Commit Scope, exact message, evidence, and
an empty initial index. It does not expose push, merge, rebase, reset, amend,
force-push, tags, branch deletion, or worktree creation/deletion.

The v0.2 review increment adds `review record`, `rework`, `resolve`, and
approval-gated `accept`. Acceptance defects rerun existing task evidence,
in-scope changes reserve monotonic `R<n>` tasks with full execution and Git
gates, follow-ups remain outside task order, and material changes block pending
a protected amendment.

The v0.2 amendment increment adds `plan amend propose`, `approve`, and `apply`.
Only typed semantic operations are accepted. Minor is a fixed task-local
allowlist; Material changes block, require an explicit approval reference,
invalidate prior plan approval and affected review/evidence state, reset
execution gates, and return the plan to Ready for validation and reapproval.

The v0.2 import increment adds `project import legacy`. It parses supported
authored fields from one bounded legacy Markdown plan, reports every mapping and
warning, and creates a separate Draft through the normal plan APIs. Source bytes
are preserved; historical status, approval, check, commit, and evidence claims
are always ignored and marked unverified.

The v0.2 standards increment adds `standards conflict list`, `refresh`, and
approval-gated `resolve`. Conflicting values are shown with exact source,
digest, source class, and precedence: current user requirement, repository
rule, project configuration, language package, then Common. Mino never merges
them silently; a source change invalidates the prior decision.

The v0.2 plan-variant increment adds `plan fork`, `diff`, and `archive`. A fork
audits and copies one exact retained revision into an independent revision-one
Draft, preserves authored content, records source lineage and hash, and clears
approval, review, execution, evidence, commit-result, and extension trust.
Semantic diff is read-only and compares normalized authored fields. Archive
requires an explicit selection reference and deactivates a plan without
deleting it or rewriting its lifecycle status. Plan forks are not Git branches,
and Mino intentionally provides no plan merge command.

The v0.3 hook increment adds `git hook propose`, `status`, approval-gated
`install`, and read-only `run`. Mino installs only absent or marker-owned
default pre/post commit hooks after a current proposal hash and approval
reference. User hooks, symbolic links, and custom `core.hooksPath` settings are
preserved with manual integration guidance. Hook runtime observes staged/HEAD
and binding facts through read-only Git commands, emits advice, always remains
optional, and never mutates Git or Mino state.

## Requirements and installation

The crate targets Rust 1.96 with edition 2024. Git is optional for non-Git
projects and required only for Git inspection, binding, branch, File Map, and
task-commit workflows.

```text
cargo build --release
cargo install --path .
mino --version
```

Mino is local and offline by default. Only an explicit `standards sync --all`
request performs network access.

## Quick start

Initialize project-local state and inspect the returned JSON:

```text
mino project init --format json --no-input
```

Fresh initialization installs the bundled Skill but only proposes changes to
`AGENTS.md` and `.gitignore`. Apply those owned blocks by executing the exact
`next_actions[0].argv` returned by the command, or explicitly run:

```text
mino project init --apply-agents-block --apply-gitignore-block --format json --no-input
mino project doctor --format json --no-input
```

To adopt an existing managed Markdown plan, import it under a new stable name
and review the returned mappings, warnings, `missing`, and `next_actions`:

```text
mino project import legacy --source legacy-plan.md --name imported-change --request-id 00000000-0000-0000-0000-000000000001 --actor user --format json --no-input
```

Create a durable Draft from the exact request bytes:

```text
mino plan create --name example-change --trigger durable --request-file request.md --request-id 00000000-0000-0000-0000-000000000001 --actor user --format json --no-input
```

Then use the stable Agent loop:

```text
mino agent context --format json --no-input
```

Execute only a returned canonical `next_actions[].argv`, use the new revision,
and read context again. Stop whenever `approval_required` is true. Never edit
`.mino/**` or a Mino-managed plan projection directly.

## Safety model

- Plan mutations require the current `--expect-revision` and an idempotency
  `--request-id`; new-plan fork creation instead binds to an exact
  `--from-revision` and also requires an idempotency request ID.
- Planned commands are launched directly as argv, never through a shell, with
  finite time/output limits and a minimal environment allowlist.
- Core execution commands perform no Git mutation. The v0.2 `git` commands can
  create only an explicitly approved deterministic branch or an eligible exact
  task commit; Git Flow consent is auditable scope, not broad or cryptographic
  authorization.
- Existing unowned Skills and malformed managed blocks are preserved.
- Standards conflicts block validation until their exact candidates are
  snapshotted and an explicit source-bound decision with rationale and external
  reference is recorded.
- Legacy workflow analysis produces inert proposals. Legacy plan import may
  create one separate Draft, but neither command deletes, renames, or edits a
  legacy source.
- Plan diff never mutates either input. Plan fork verifies source history before
  publishing a target, and archive preserves every prior snapshot and event.
- Advisory hooks require an explicit hash-bound install approval. Runtime hooks
  are reminders only: their templates tolerate an unavailable/refusing Mino and
  never stage, commit, or change plan/evidence state.

## Documentation

- [Architecture and state ownership](docs/architecture.md)
- [CLI and JSON contract](docs/command-contract.md)
- [Protocol and legacy migration](docs/migration.md)
- [Security and operational boundaries](docs/security.md)

Use `mino <group> --help` for current argument details. The command inventory
and stable machine contracts are also checked by the test suite.
