---
name: mino
description: Create, validate, execute, resume, update, amend, review, rework, and audit durable implementation plans through the Mino CLI. Use when a user explicitly requests a formal plan, work requires durable planning, an existing Mino plan must be resumed or updated, verification evidence must be recorded, review or rework must be handled, or plan-scoped Git Flow must be followed.
---

<!-- mino-managed-skill:v1 -->

# Mino

Treat the `mino` CLI as the sole authority for plan state, legal transitions,
verification evidence, and generated Markdown. Use the Skill only to interpret
the user's intent and orchestrate returned commands.

## Required workflow

1. Run `mino project doctor --format json --no-input`.
2. If `mino` is unavailable, stop and explain that it must be installed. Never
   copy a planning template or emulate Mino state manually.
3. If doctor reports that no initialized Mino project exists, run
   `mino project init --format json --no-input` once, then follow its returned
   `next_actions`.
4. Follow any safe canonical `next_actions` returned by doctor. Do not repair a
   conflict or malformed managed block by editing around it.
5. Run `mino agent context --format json --no-input` before creating, resuming,
   changing, executing, or reviewing a plan.
6. If `approval_required` is `true`, stop and request the explicit approval
   described by the context. Do not invoke an approval action for the user.
7. When `next_actions` is non-empty, execute only an exact returned `argv`
   action that matches the user's requested work. Preserve argv boundaries and
   do not translate it into a shell string.
8. After every successful mutation, discard the prior revision and read Agent
   context again before selecting another action.
9. Record planned checks, acceptance criteria, checkpoints, and supplemental
   evidence through Mino commands. Never claim completion from prose alone.

## Starting a plan

When context reports no active plan and the user explicitly requests a formal
or durable plan, read [command-contract.md](references/command-contract.md) and
use `plan create` once with the user's request and a fresh request ID. Then
return to the required workflow and follow the CLI's canonical next actions.

## State and Git guardrails

- Never edit `.mino/**` or Mino-managed `docs/plan/*.md` directly.
- Pass the current `--expect-revision` on every mutation.
- Use a fresh request ID for each distinct mutation. Reuse one only to retry
  the exact same command and inputs.
- Never infer approval from plan completeness, conversation tone, or a prior
  task. Read [approval-boundaries.md](references/approval-boundaries.md) before
  approval, standards-conflict resolution, exception, review/rework, or
  plan-scoped Git operations.
- Do not run an unsupported or hidden Git mutation. Invoke branch creation only
  after its separate explicit approval, and invoke task commit only through an
  exact returned `git.commit` argv under current Approved Git Flow consent.
  Never substitute manual staging, commit, cleanup, or destructive Git commands.
- Never use bundled protocol Markdown as a fallback workflow.

## References

- Read [command-contract.md](references/command-contract.md) when starting a
  plan, interpreting JSON or exit codes, or recording execution evidence.
- Read [approval-boundaries.md](references/approval-boundaries.md) whenever
  context requires approval or the next step involves review, exceptions,
  rework, or Git.
