---
name: mino
description: Create, fork, compare, validate, execute, resume, update, amend, archive, review, rework, audit, emit scheduler-neutral task handoffs, and optionally integrate advisory Git hooks for durable implementation plans through the Mino CLI. Use when a user explicitly requests a formal plan, work requires durable planning, plan alternatives must be compared, an existing Mino plan must be resumed or updated, verification evidence must be recorded, bounded scheduled work must be specified, review or rework must be handled, or plan-scoped Git Flow must be followed.
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
  approval, plan archive, standards-conflict resolution, exception, review/rework, or
  plan-scoped Git operations.
- Do not run an unsupported or hidden Git mutation. Invoke branch creation only
  after its separate explicit approval, and invoke task commit only through an
  exact returned `git.commit` argv under current Approved Git Flow consent.
  Never substitute manual staging, commit, cleanup, or destructive Git commands.
- Never use bundled protocol Markdown as a fallback workflow.

## Advisory hooks

Treat repository hooks as optional reminders, never as hidden plan transitions.
Use `git hook propose` or `status` first. Before `git hook install`, read
[approval-boundaries.md](references/approval-boundaries.md), show the current
proposal hash and every ownership conflict, and obtain an explicit approval
reference. Never overwrite or compose a user hook automatically; present the
returned manual snippet instead. `git hook run` is read-only and its advice does
not prove approval, verification, commit eligibility, or completion.

## Plan alternatives

Use `plan fork` only with an exact retained source revision and an explicit
reason. Treat the result as an independent Draft whose prior approvals,
execution, evidence, review, commit results, extensions, and Git authorization
are untrusted and reset. Use read-only `plan diff` to present normalized authored
differences. Mino has no plan merge operation, and a plan fork never authorizes
or creates a Git branch.

When the user selects one alternative and explicitly authorizes deactivation of
another, read [approval-boundaries.md](references/approval-boundaries.md) and
invoke `plan archive` with their reason and auditable reference. Never infer
archive approval merely because a fork exists, and never delete plan state.

## Bounded check monitoring

Use `exec check monitor` only for repeated foreground observation of one check
already authored in the active plan. Read
[command-contract.md](references/command-contract.md), supply explicit finite
attempt, interval, and deadline values, and preserve the exact returned argv
and request ID for recovery. An optional cancellation file must be a safe
project-relative regular file. Treat pass as success; treat attempt exhaustion,
deadline, and cancellation as exit-6 terminal reports whose attempt evidence
must be preserved. Never emulate monitoring with a shell loop, tail, watcher,
daemon, scheduler, repeated manual polling, or reconstructed child requests.

## Scheduled-task handoffs

Use `exec schedule spec` only to emit an inert handoff for one current runnable
check. Read [command-contract.md](references/command-contract.md), supply the
exact plan revision, bounded monitor and dispatch policy, execution environment,
trigger/expiry, success/stop/failure handling, and safe result destination.
Verify the returned digest and show that `external_creation_required` is true
and `authorization_granted` is false. Stop before using any external scheduler
until the user separately authorizes creation or update in that scheduler.
Never infer scheduler consent from plan approval, Git Flow, or specification
emission, and never emulate scheduling with a background loop or daemon.

## References

- Read [command-contract.md](references/command-contract.md) when starting a
  plan, interpreting JSON or exit codes, recording execution evidence, or
  invoking bounded check monitoring or scheduled-task handoff output.
- Read [approval-boundaries.md](references/approval-boundaries.md) whenever
  context requires approval or the next step involves review, exceptions,
  rework, or Git.
