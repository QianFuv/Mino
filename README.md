# Mino

Mino is a local, versioned plan protocol engine for coding agents. It turns
durable implementation plans into revision-checked state, deterministic
Markdown projections, ordered execution, immutable evidence, and explicit
approval boundaries. It is not a template generator and does not contain an
LLM.

The current package is `mino 0.1.0`. It implements the lifecycle from project
initialization through `Review`. Review acceptance/rework, Git mutation,
worktrees, plan variants, scheduled observation, team catalogs, and plugin
distribution are intentionally not v0.1 commands.

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

## Requirements and installation

The crate targets Rust 1.96 with edition 2024. Git is optional for non-Git
projects, but Git-backed plans can record readiness and use read-only File Map
inspection.

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
  `--request-id`.
- Planned commands are launched directly as argv, never through a shell, with
  finite time/output limits and a minimal environment allowlist.
- Mino performs no Git mutation in v0.1. Git Flow consent and commit gates are
  auditable constraints, not a hidden commit permission or cryptographic
  authorization.
- Existing unowned Skills and malformed managed blocks are preserved.
- Legacy migration produces a report and proposals; it never deletes or edits
  legacy sources.

## Documentation

- [Architecture and state ownership](docs/architecture.md)
- [CLI and JSON contract](docs/command-contract.md)
- [Protocol and legacy migration](docs/migration.md)
- [Security and operational boundaries](docs/security.md)

Use `mino <group> --help` for current argument details. The command inventory
and stable machine contracts are also checked by the test suite.
