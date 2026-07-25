# Protocol and legacy migration

## Locked protocol bundle

Mino v0.1 embeds the exact imported planning resources as inert bytes:

| Field | Value |
|---|---|
| Protocol version | `2026-05-11` |
| Protocol revision | `review-rework-git-flow-v1` |
| Plan schema | `1` |
| Renderer | `2` |
| `PLAN_TEMPLATE.md` SHA-256 | `73b55c3b64acc7e890464e180a4546b37c288984594f979e49dc117b7b634e9f` |
| `PLAN_EXECUTION.md` SHA-256 | `08076f7ecb2892bdb416c00b170af9d53a835bb5a7e7580f768059d318d1976a` |

Initialization verifies the embedded manifest and writes the corresponding
`.mino/protocol.lock`. `mino protocol status --format json` verifies the bundle
again and compares lock format, protocol version/revision, plan schema, and
renderer. It is read-only and returns compatibility findings through `missing`.

The bundled Markdown is provenance and documentation. Never copy it into a
repository to emulate Mino when the CLI is unavailable; compiled validators and
state transitions are the runtime authority.

## Explicit protocol migration

Use the exact current plan revision and a request UUID:

```text
mino protocol migrate --plan <id> --expect-revision <n> --request-id <uuid> --to <calendar-version> --format json --no-input
```

The v0.1 registry has no older supported transform. A plan already bound to
`2026-05-11/review-rework-git-flow-v1` returns the deterministic
`already_current` disposition and writes no revision, event, snapshot, or
projection. Any other source/target combination exits 5 and preserves every
plan byte. Future releases must register and test an explicit transform before
this command may mutate a plan; silent lock rewrites are forbidden.

## Legacy workflow analysis

Analyze any subset of the three historical documents:

```text
mino project migrate legacy --agents AGENTS.md --template PLAN_TEMPLATE.md --execution PLAN_EXECUTION.md --format json --no-input
```

At least one input is required. Each input is read as non-empty UTF-8 with a
1 MiB maximum. The report includes:

- Exact path, byte count, and SHA-256 identity for every source.
- Every Markdown heading in source order.
- A `mapped`, `ambiguous`, or `unsupported` disposition and proposed owner.
- Stable findings for duplicate headings, missing headings, ambiguities, and
  unsupported custom sections.
- A proposed Mino workflow block diff for AGENTS or an inert bundle migration
  proposal for template/execution documents.
- `applied: false` and an empty `deleted_sources` list.

The command never edits, renames, or deletes a legacy source.

## Ownership mapping

| Historical concern | v0.1 destination |
|---|---|
| Formal/durable plan trigger | Stable AGENTS workflow block plus Skill description |
| Plan template fields | Versioned `Plan` schema and deterministic renderer |
| Status values and execution order | Domain state machine and execution services |
| Ready criteria | `plan validate` and `plan finalize` |
| Plan review gate | `plan review` and `plan approve` |
| Verification results | Check-run journal and immutable evidence store |
| Checkpoints/block/resume | `exec checkpoint`, `exec block`, `exec resume` |
| Git readiness and commit declarations | Plan fields and File Map policy; no v0.1 Git mutation |
| Common/language rules | Embedded standards packages and resolved checks |
| Pinned external planning documents | Embedded protocol bundle and protocol lock |
| Repository-specific MCP/tool routing | Remains in user-owned AGENTS content |
| Custom release/deployment/notification rules | Manual review or a follow-up system |

## Safe adoption sequence

1. Back up or commit current repository instruction files under your normal
   repository policy.
2. Run `project migrate legacy` and review every ambiguous/unsupported finding.
3. Run `project init` without apply flags. This installs the Skill only when the
   path is absent or already Mino-owned and proposes integration blocks.
4. If the proposed blocks are acceptable, run init with the explicit AGENTS and
   `.gitignore` apply flags.
5. Run `project doctor` and `protocol status` until no blocking finding remains.
6. Create new plans through the CLI. Legacy plan documents remain references;
   v0.1 does not import them into live plan state.
7. Delete or simplify old files only after separate user review. Mino never
   performs that cleanup.

## Conflict and recovery behavior

- A Skill without `<!-- mino-managed-skill:v1 -->` is unowned. Mino reports
  `mino_skill_conflict` and preserves its complete tree.
- A Mino-owned Skill may be refreshed file-by-file; unknown repository files
  inside the Skill directory are preserved.
- Missing blocks are proposals until their apply flag is present. Valid owned
  but stale blocks can be refreshed. Duplicate, reversed, partial, non-UTF-8,
  symlinked, or non-file marker targets are conflicts and are not overwritten.
- Protocol migration errors and legacy-analysis errors do not write plan state.
- Prepared plan transactions are recovered by normal plan loads/doctor. Do not
  manually delete `.mino/**` transaction or history files.
