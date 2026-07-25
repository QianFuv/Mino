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

## Initial creation

Only when the user explicitly requests formal or durable planning and context
has no active plan, create a UTF-8 request file containing the exact request,
then invoke:

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
Use `evidence add` only for supplemental files or observations that are not a
planned check. Never mark a criterion or task complete without the evidence
references required by the current plan.

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
