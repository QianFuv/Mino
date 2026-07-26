# Security and operational boundaries

## Trust model

Mino is a local protocol engine, not an authorization service or sandbox. It
assumes the operating-system account may read/write the selected repository and
launch the exact tools named by an approved plan. Repository content, plan
commands, legacy files, check output, evidence artifacts, remote catalogs, and
the calling agent are all untrusted inputs that must pass deterministic policy.

Plan approval is an auditable user declaration. It is not cryptographic proof,
identity verification, or permission for arbitrary filesystem, network, Git,
deployment, or messaging operations.

Final review acceptance is a separate auditable declaration. `review accept`
requires an explicit approval reference and cannot infer acceptance from clean
checks, resolved feedback, or an earlier plan approval.

Material amendment approval is also an auditable declaration rather than a
cryptographic authorization token. `plan amend approve` must identify the exact
pending `C<n>` proposal and carry a non-empty external approval reference.

Plan archive is a separate auditable user-selection declaration. `plan archive`
requires a non-empty reason and approval reference; plan approval, clean checks,
or creation of an alternative cannot infer that the original should be
deactivated.

Advisory hook installation is also a separate declaration. `git hook install`
must match the current proposal hash and carry a non-empty external approval
reference. Plan approval, Git Flow consent, and an earlier hook proposal are not
installation authority by themselves.

Standards-conflict resolution is a separate auditable declaration.
`standards conflict resolve` must select a displayed current candidate and
carry both a rationale and external decision reference. A precedence default,
plan approval, or prior conflict decision is not a substitute. `exec start`
rechecks the live source fingerprints at the current revision, so a source
change after approval cannot bypass refresh and a renewed explicit decision.

## Filesystem boundaries

- Project roots are canonicalized and discovered deterministically. Managed
  artifact paths must remain project-relative; absolute paths and traversal are
  rejected where the protocol requires repository ownership.
- `.mino/plans/**` is the canonical store. Do not edit it manually.
- `.mino/standards.local.toml` is optional user-reviewed input. Each declared
  file source must remain within the canonical project root, be a regular file,
  and fit the bounded read limit; traversal and symbolic-link escapes are
  rejected.
- `.mino/active.json` is a versioned worktree/branch binding store. Change it
  only through `mino git bind`; malformed or stale identity is diagnosed
  instead of silently repaired.
- `.mino/git/branches/**` contains immutable approval-bound branch intents and
  completions. Prepared records are recovery state, not permission to alter or
  delete Git data manually.
- `.mino/git/commits/**` contains immutable content snapshots, staged-tree
  identities, and terminal task-commit results. Do not edit these records or use
  them as permission for a broader commit.
- Default `.git/hooks/pre-commit` and `.git/hooks/post-commit` are managed only
  after hash-bound approval and only when absent or marked Mino-owned. User
  hooks, symbolic links, non-files, oversized hooks, and custom
  `core.hooksPath` configurations are preserved and require manual integration.
- `docs/plan/*.md` is a digest-checked projection. Manual changes cause drift
  and are preserved rather than overwritten.
- Plan transactions, snapshots, events, run journals, evidence records, and
  blobs use create-new, guarded replacement, locks, canonical bytes, and digest
  checks appropriate to their role.
- Skill/block integration refuses symbolic-link components, non-file block
  targets, unowned Skill bytes, and malformed/duplicate markers. Valid updates
  replace only owned bytes and retain a backup until publication succeeds.
- Legacy workflow analysis is read-only. Legacy plan import enforces regular
  file, UTF-8/non-empty/NUL-free/1 MiB bounds and preserves exact source bytes;
  its only write is a separate Draft through the normal recoverable plan store.
- Imported lifecycle, approval, result, commit, review, and evidence assertions
  are ignored. Unsafe or Mino-owned paths, shell-control syntax, and known shell
  or destructive check executables are omitted with warnings. Import never
  finalizes, approves, executes, or commits the Draft.
- Plan fork audits source events and snapshots before it creates target storage.
  Missing, corrupt, or digest-inconsistent history fails without publishing a
  target or changing source bytes. Forked execution, evidence, approval, review,
  commit-result, and extension state is never trusted.
- Plan archive never deletes or relocates canonical state. Its typed record is
  revisioned with the plan, while every prior snapshot and event remains
  immutable and readable.

The managed `.gitignore` block excludes `/.mino/` and `/docs/plan/`, but ignore
rules are not access control or encryption. `.mino` may contain request text,
paths, command summaries, environment digests, evidence, and artifacts. Protect
the repository with normal filesystem permissions and do not share runtime
state blindly.

## Process execution

`exec check run` starts the exact authored executable and argv without a shell.
The working directory must resolve inside the project, and the child receives a
minimal cross-platform environment allowlist rather than the complete parent
environment. The v0.1 defaults are a five-minute timeout and 1 MiB combined
stdout/stderr capture. Protocol constructors enforce finite maxima of one hour
and 16 MiB.

Mino uses process groups/job objects to terminate descendant processes when a
timeout, output limit, or capture failure ends the run. Spawn failure,
unexpected exit, timeout, output limit, capture failure, and interruption are
durable terminal outcomes. Do not treat an exit-6 check as missing evidence;
the failure evidence is intentionally retained.

## Output redaction and evidence

Output is redacted before hashing or persistence. The default policy replaces
secret-shaped `api_key`, `token`, `secret`, `password`, and `authorization`
key/value text and records only rule IDs/counts. Secret-named allowlisted
environment values are also registered as runtime literal redactions.

Redaction is defense in depth, not a guarantee that arbitrary sensitive output
will be recognized. Planned checks should avoid printing secrets. Supplemental
file/log/screenshot evidence can itself contain sensitive bytes; Mino copies it
into the local content-addressed store after path and digest checks. Review
evidence before sharing `.mino` or derived reports.

Evidence is immutable. Corrections create a new record linked by `supersedes`.
A superseded record cannot satisfy current completion gates. AcceptedException
evidence must carry the approval-compatible binding required by policy; it is
not a general bypass. Applied amendments retain affected records but mark their
identifiers stale; stale evidence cannot prove a current criterion, check, or
commit gate. Pending proposals reject new evidence so it cannot be captured
against ambiguous inputs.

## Network behavior

Mino has no telemetry and does not auto-fetch protocol or standards updates.
The only v0.1 network path is an explicit `mino standards sync --all` using the
catalog URL in `.mino/config.toml`.

Default synchronization policy permits HTTPS only, follows no redirects, uses
an end-to-end 30-second timeout, limits catalog and individual documents to
1 MiB, limits the full request to 16 MiB, validates all package documents and
SHA-256 identities, stages them in a new immutable cache generation, and only
then updates `standards.lock`. Loopback HTTP exists only in the library test
policy and is not selected by the CLI.

Evidence URL values and legacy references are stored as references; Mino does
not fetch them.

## Git and external side effects

The implemented Git adapter runs directly without a shell, disables terminal
prompting, bounds captured output, and strictly parses machine-readable
results. Read-only root/repository/worktree/HEAD/index/status probes also
disable optional Git locks. `mino git inspect` writes nothing. `mino git bind`
writes only `.mino/active.json` through a bounded lock and atomic replacement;
it does not mutate Git state.

`mino git branch create` is the only branch/ref creation path. It requires an
explicit approval reference, accepts only the deterministic proposed name,
rechecks clean source/base/worktree identity, prepares an immutable recovery
intent, disables repository hooks with a command-local `core.hooksPath`, and
binds only after exact post-state confirmation. A refusal occurs before the
intent or Git mutation. A failed or interrupted attempt preserves its intent
and observed state for exact retry; Mino never resets, deletes, or cleans up the
repository to conceal a partial external result.

`mino git commit` is the only index/commit mutation path. It requires a current
approved plan with Approved Git Flow consent, a Done first-pending task, current
same-worktree binding and authorized branch, exact parent HEAD, satisfied
evidence, and changed paths inside both File Map and Commit Scope. Pure preflight
rejects every pre-existing staged path, mixed index/worktree content,
out-of-scope path, unsafe file kind, clean filter, and identity drift. After an
immutable intent, Mino stages only explicit paths and runs the exact one-line
message. Repository commit hooks run normally; Mino never uses `--no-verify`.
Git receives null stdin, terminal prompting is disabled, and combined output and
runtime are bounded. Hooks still execute as repository code with the Git
process environment; review and secure them like any other local build script.

If staging or a hook/commit fails, the exact staged state and immutable journal
remain visible, and the plan becomes Blocked. Mino never resets, cleans, checks
out, or unstages to hide the failure. An exact retry after `exec resume` verifies
the recorded source/tree and reconciles an already-created commit before writing
Commit evidence, the plan gate, and terminal completion. It never creates a
second commit for the same journal.

Mino does not push, merge, rebase, reset, amend, force-push, tag, delete
branches, or create/delete worktrees.

`plan fork` does not invoke Git, create a ref, switch a worktree, or inherit Git
authorization. It creates only a separate Mino Draft. Mino does not provide a
plan merge command; alternatives are compared with read-only `plan diff`, then
the user's unselected plan may be archived through its explicit boundary.

Advisory hook installation writes only the two inspected default hook paths; it
does not run `git config`, stage, commit, switch, or alter refs. The hook scripts
invoke only read-only `git hook run`, tolerate errors, and exit successfully.
Runtime uses Git status/config/identity reads with optional locks disabled and
does not write any Mino plan, event, evidence, active binding, or hook file.

Active-plan selection requires the canonical common-directory and worktree to
match. Branch bindings require the same branch; detached bindings require the
same exact HEAD. Stale and foreign bindings expose no active plan, preventing a
plan or authorization decision from leaking across linked worktrees.

File Map matching accepts normalized exact paths plus narrow `*`/`**` patterns.
Traversal, absolute paths, malformed Git porcelain, duplicate paths, and
out-of-scope changes block task completion.

Mino also does not deploy software, send messages, create tickets, or modify
remote systems. Those actions require separate tools and explicit authority.

## Approval and Agent stops

Agent consumers must use JSON/no-input mode and inspect `approval_required`,
`blocked_actions`, and `next_actions` before every action. They must stop on:

- `approval_required: true` or exit 4.
- `git.branch.create`, unless the user explicitly approved that exact proposal
  and supplied the recorded approval reference.
- `git.hook.install`, unless the user explicitly approved the exact current
  proposal hash and supplied an auditable reference.
- `review.accept`, unless the user explicitly accepted the fully resolved
  current Review revision and supplied an auditable reference.
- `plan.amend.approve`, unless the user explicitly approved the exact pending
  Material `C<n>` proposal and supplied an auditable reference.
- `plan.archive`, unless the user explicitly selected the alternative, supplied
  the archive reason, and supplied an auditable approval reference.
- `standards.conflict.resolve`, unless the user explicitly selected one exact
  current candidate and supplied its rationale and auditable decision
  reference.
- An approval, exception, or Git operation not already covered by explicit user
  and repository policy. A returned `git.commit` action is covered only by the
  current plan's Approved Git Flow gate; branch creation still needs its own
  explicit approval reference.
- Exit 5 policy refusal, exit 8 drift/corruption, or malformed integration
  ownership.
- A material change outside the approved plan outcome, File Map, criteria, or
  commit scope.
- Material review feedback, because it requires a protected amendment and
  cannot be resumed through the generic execution command.
- Any pending amendment: execute only its advertised approve/apply boundary;
  do not run checks, add evidence, commit, or resume around it.

Never approve on the user's behalf, infer authorization from conversational
tone, copy the protocol template as a fallback, or fabricate plan/evidence
state when Mino is unavailable. There is no hidden Git mutation path: local
branch creation is exposed only as `git branch create`, exact task commits only
as `git commit`, hook-file installation only as approval-bound `git hook
install`, and `git bind` is the declared Git-adjacent Mino-state write.
There is no arbitrary status setter. Review-to-Done is exposed only as
`review accept`; task rework is exposed only as a classified recorded review
item followed by `review rework` and `review resolve`. Plan deactivation is
exposed only as approval-bound `plan archive`; it never deletes plan history.

## Recovery guidance

1. Preserve all current bytes and command output.
2. Run `project doctor`, `protocol status`, and the relevant read-only show/list
   command.
3. Refresh Agent context after revision conflicts.
4. Retry only the exact same request UUID/argv when replay is intended.
5. Use canonical returned remediation. Do not delete locks, journals,
   snapshots, evidence, or projections manually.
6. If corruption or a marker conflict remains, stop and restore/reconcile from
   a reviewed backup or seek maintainer support.
