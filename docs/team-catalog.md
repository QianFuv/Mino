# Team standards catalogs

Mino v0.3 can turn a reviewed directory of TOML standards packages into a
static, digest-verified catalog. The catalog is data distribution, not a plugin
runtime: it contains only package manifests, rules, checks, and generated index
metadata. Existing projects consume it through the same explicit
`mino standards sync --all` command introduced in v0.1.

## Source contract

A source root contains exactly `catalog-source.toml` and `packages/`. The source
file declares `source_version = 1`, a lowercase DNS-like namespace, and a
canonical HTTPS base URL. Every immediate package directory contains exactly
`manifest.toml`, `rules.toml`, and `checks.toml`.

Package IDs belong to the namespace, such as `engineering.example.common` and
`engineering.example.rust`. Versions are canonical SemVer. Rule and check IDs
must belong to their package, paths must be normal UTF-8 relative paths, and all
documents are bounded non-empty data files. Symlinks, executable source files,
special files, duplicate identities, unknown fields, escaping paths, and size
limit violations are rejected.

Start a source tree with an inert example package:

```text
mino standards catalog init \
  --source team-standards \
  --namespace engineering.example \
  --base-url https://standards.example.com/mino \
  --format json --no-input
```

`init` never overwrites an existing destination. Replace the example values
with reviewed organization policy before distribution.

## Validate and build

Validation is read-only:

```text
mino standards catalog validate \
  --source team-standards \
  --format json --no-input
```

Build a static hostable tree:

```text
mino standards catalog build \
  --source team-standards \
  --output dist/team-standards \
  --format json --no-input
```

The builder normalizes TOML and LF endings, sorts every identity, computes
package and tree SHA-256 digests, stages the complete output, verifies it, and
then publishes it atomically. The output contains:

```text
dist/team-standards/
|-- catalog.toml
|-- catalog-manifest.json
`-- packages/
    `-- <package>/
        |-- manifest.toml
        |-- rules.toml
        `-- checks.toml
```

`catalog.toml` is the compatibility surface consumed by Mino sync.
`catalog-manifest.json` is supplemental operator evidence: it records the exact
source, file, package, catalog, and tree identities. Building the same canonical
source twice produces the same bytes and digests. Rebuilding may replace only
an existing output that passes the complete Mino catalog verification; corrupt
or unrelated destinations are preserved and rejected.

## Host, sync, and apply

Serve the generated directory as immutable static files at the exact HTTPS base
URL declared in the source. Do not edit published files in place. Publish a new
SemVer package version and rebuild when policy changes, retaining older bytes
for projects whose lock still references them.

Each project configures the catalog URL in `.mino/config.toml`, then performs
the only catalog network operation explicitly:

```text
mino standards sync --all --format json --no-input
```

Sync requires HTTPS, disables redirects, enforces elapsed and byte limits,
verifies every package digest, writes a new immutable cache generation, and
updates `.mino/standards.lock` only after the generation is complete. An
identical verified generation is reused. The CLI does not permit loopback HTTP;
that policy exists only for deterministic library tests.

Synchronization and application are intentionally separate. Sync downloads,
caches, and locks every catalog package, but does not activate those packages
or add every catalog language to a project. The current v0.3
`standards recommend` and `standards apply --recommended --seed-verification`
surface continues to resolve the built-in project/language recommendations; it
does not expose a remote-package selection flag. This catalog increment adds
safe team authoring and complete sync compatibility without silently widening
an existing project's active rule set.

Standards application never silently resolves conflicts. Conflicting current
user, repository, project, language-package, or Common values still require the
normal `standards conflict` review and approval flow.

## Trust and recovery

Trust is explicit at every boundary:

- The catalog author controls source review and HTTPS hosting.
- Namespace ownership and SemVer identify policy; SHA-256 identifies exact
  bytes.
- Mino trusts no downloaded package until its declared documents and aggregate
  digest verify.
- A failed download, parse, limit, digest, or atomic publication leaves the
  previously active cache and lock unchanged.
- A source change invalidates an earlier standards-conflict decision until the
  new candidates are refreshed and explicitly selected.

For recovery, restore the last verified static tree or correct the source and
publish a new version, then rerun `standards sync --all`. Never repair cached
files or `.mino/standards.lock` manually.

## Deliberate non-goals

Team catalogs do not execute arbitrary code, discover packages, infer trust,
merge conflicting rules, push updates, poll a server, auto-update a project, or
provide a hosted registry service. Mino has no catalog daemon or background
refresh. Network access occurs only for the user's explicit sync command.
