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

## Filesystem boundaries

- Project roots are canonicalized and discovered deterministically. Managed
  artifact paths must remain project-relative; absolute paths and traversal are
  rejected where the protocol requires repository ownership.
- `.mino/plans/**` is the canonical store. Do not edit it manually.
- `.mino/active.json` is a versioned worktree/branch binding store. Change it
  only through `mino git bind`; malformed or stale identity is diagnosed
  instead of silently repaired.
- `.mino/git/branches/**` contains immutable approval-bound branch intents and
  completions. Prepared records are recovery state, not permission to alter or
  delete Git data manually.
- `docs/plan/*.md` is a digest-checked projection. Manual changes cause drift
  and are preserved rather than overwritten.
- Plan transactions, snapshots, events, run journals, evidence records, and
  blobs use create-new, guarded replacement, locks, canonical bytes, and digest
  checks appropriate to their role.
- Skill/block integration refuses symbolic-link components, non-file block
  targets, unowned Skill bytes, and malformed/duplicate markers. Valid updates
  replace only owned bytes and retain a backup until publication succeeds.
- Legacy migration is read-only and enforces UTF-8/non-empty/1 MiB bounds.

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
not a general bypass.

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

`mino git branch create` is the sole implemented Git mutation. It requires an
explicit approval reference, accepts only the deterministic proposed name,
rechecks clean source/base/worktree identity, prepares an immutable recovery
intent, disables repository hooks with a command-local `core.hooksPath`, and
binds only after exact post-state confirmation. A refusal occurs before the
intent or Git mutation. A failed or interrupted attempt preserves its intent
and observed state for exact retry; Mino never resets, deletes, or cleans up the
repository to conceal a partial external result.

Mino does not yet stage, commit, push, merge, rebase, reset, amend, force-push,
tag, delete branches, or create/delete worktrees. A plan's Git Flow consent and
commit gate still constrain an external authorized commit workflow but do not
yet execute it.

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
- An approval/exception/Git operation not already covered by explicit user and
  repository policy.
- Exit 5 policy refusal, exit 8 drift/corruption, or malformed integration
  ownership.
- A material change outside the approved plan outcome, File Map, criteria, or
  commit scope.
- Review in v0.1, because acceptance/rework commands are not implemented.

Never approve on the user's behalf, infer authorization from conversational
tone, copy the protocol template as a fallback, or fabricate plan/evidence
state when Mino is unavailable. There is no hidden Git mutation path: local
branch creation is exposed only as `git branch create`, while `git bind` is the
declared Git-adjacent Mino-state write. There is no arbitrary status setter.

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
