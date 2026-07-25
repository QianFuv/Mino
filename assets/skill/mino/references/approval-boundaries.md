<!-- mino-managed-skill:v1 -->

# Mino approval boundaries

## Stop conditions

Stop and request user action when any of these is true:

- Agent context returns `approval_required: true`.
- A command exits with the approval-required category or exit code 4.
- The next operation would approve a plan, accept an exception, mutate Git,
  replace an unowned Skill, or repair malformed managed markers.
- The requested change materially exceeds the approved plan File Map, task
  outcome, acceptance criteria, or commit scope.

Do not convert user intent, an earlier approval, or a clean validation result
into a new approval declaration. Never invoke approval commands on the user's
behalf.

## Review and rework

Read fresh Agent context before review. Record reviewer feedback through a
supported semantic command when advertised. Keep accepted items immutable.
For rework, resume only through a canonical returned action and preserve the
revision and evidence history. If v0.1 reports Review with no supported rework
action, stop; do not edit plan state directly.

## Plan-scoped Git Flow

Treat a plan commit gate as a scope and message constraint, not a general Git
permission. Confirm the repository instructions separately authorize the
specific mutation. Stage only declared paths or hunks, never `git add .`, and
use exactly the planned one-line message. Mino v0.1 has no Git mutation command,
so never claim that Mino performed, approved, or verified a commit it cannot
execute. Push, merge, rebase, reset, amend, force-push, tag, and branch deletion
always remain outside the Skill unless a later capability explicitly exposes
them and the user authorizes them.
