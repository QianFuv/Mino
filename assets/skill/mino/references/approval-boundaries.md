<!-- mino-managed-skill:v1 -->

# Mino approval boundaries

## Stop conditions

Stop and request user action when any of these is true:

- Agent context returns `approval_required: true`.
- A command exits with the approval-required category or exit code 4.
- The next operation would approve a plan, accept an exception, mutate Git
  outside a current plan-scoped commit gate or explicitly approved branch
  proposal, archive a plan, resolve a standards conflict, replace an unowned Skill, or repair
  malformed managed markers.
- The requested change materially exceeds the approved plan File Map, task
  outcome, acceptance criteria, or commit scope.

Do not convert user intent, an earlier approval, or a clean validation result
into a new approval declaration. Never invoke approval commands on the user's
behalf.

## Plan archive

`plan.archive` is an approval boundary for the exact current plan revision.
Before invoking it, show the plan being deactivated, the retained alternative
when applicable, and the proposed reason. Obtain explicit user selection plus
an auditable reference. A prior plan approval, a successful fork, or a diff does
not authorize archive. Archive must preserve the lifecycle status, canonical
state, snapshots, events, and projection; never delete or rename those bytes.

## Advisory hook installation

`git.hook.install` is an approval boundary for one exact current proposal hash.
Show both hook paths, ownership states, actual/template digests, blockers, and
manual integration snippets before asking. Obtain an auditable external
reference, then pass both values unchanged. Plan approval, Git Flow consent, a
prior hook install, or an unchanged repository is not a substitute. Never
install when Mino reports a user hook, symbolic link, unsupported path, or
custom `core.hooksPath`; preserve those bytes and let the user integrate the
returned snippet manually.

## Standards conflicts

Show every current candidate with its source, digest, source class, and numeric
precedence. A unique highest-precedence candidate is only a displayed default;
never treat it as approval or merge competing values. Before invoking
`standards.conflict.resolve`, obtain the user's exact candidate choice,
rationale, and auditable decision reference. A source change invalidates the
decision and requires a new explicit choice.

## Review and rework

Read fresh Agent context before review. Record reviewer feedback through a
supported semantic command when advertised. Keep accepted items immutable.
For rework, use only a canonical returned action and preserve the revision and
evidence history. Acceptance Defect permits an evidence-only rerun; changed
files require In-Scope Rework. An In-Scope R task must use its reserved ID and
complete execution/Git definition. Material Change requires a protected
amendment and cannot use `exec resume`.

`review.accept` is an approval boundary. Never invoke it from clean checks or a
prior plan approval. Stop, obtain explicit acceptance for the fully resolved
current Review, and pass the user's auditable reference.

## Protected amendments

Use only `plan amend propose` with a strict typed patch after Draft. Never edit
the canonical JSON or managed Markdown. Treat Minor as the protocol allowlist,
not a caller preference; a supplied classification may raise but never lower
the computed minimum. While any proposal is pending, do not execute, add
evidence, commit, or use generic resume.

`plan.amend.approve` is an approval boundary for the exact pending Material
`C<n>` record. Stop, show its operations, base revision/hash, and computed
impact, obtain explicit user approval, and pass the auditable reference. After
apply, refresh context. A Material apply returns Ready with plan approval and
Git consent cleared, so validate, review, and obtain new plan approval before
execution. Stale evidence remains history and must be replaced by fresh runs.

## Plan-scoped Git Flow

Treat a plan commit gate as a scope and message constraint, not a general Git
permission. Confirm the repository instructions separately authorize the
specific mutation. `git.branch.create` is a separate approval boundary: first
run the read-only proposal, stop for explicit approval of that exact branch,
then pass the supplied reference to `git branch create`. Plan approval and Git
Flow consent do not authorize branch creation. Mino records a prepared intent
for recovery; do not delete it or retry with a different approval reference.

`git.commit` is executable only when Agent context returns it for a Done task
under current plan approval and Approved Git Flow consent. That existing plan
approval authorizes only the recorded one-line message and exact resolved File
Map/Commit Scope paths, so `git.commit` is not a second approval boundary. Do
not invoke it for another task, branch, parent, message, or path set. Mino stages
only explicit paths and leaves hook failures visible and recoverable; never run
manual `git add`, `git commit`, `--no-verify`, reset, or unstage to bypass it.
Push, merge, rebase, amend, force-push, tag, branch deletion, and worktree
mutation remain outside the Skill unless a later capability explicitly exposes
them and the user authorizes them.
