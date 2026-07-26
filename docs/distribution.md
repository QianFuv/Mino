# Native Codex plugin distribution

Mino v0.3 defines a reproducible native artifact for the canonical Codex plugin
source in `plugins/mino/`. Artifact construction is local and target-native.
The repository does not publish, upload, install, update, or register the
plugin automatically.

## Source and compatibility identity

The canonical source contains `.codex-plugin/plugin.json`, `launcher.json`, a
README, and the exact Skill tree copied from `assets/skill/mino`. It contains no
binary. `launcher.json` pins:

- the Cargo/plugin semantic version;
- protocol version, revision, schema, and renderer;
- Agent capabilities, context, and next-action schemas plus the capabilities
  digest;
- embedded standards package versions;
- five supported native targets and exact binary names;
- the non-interactive capabilities, doctor, and context probes; and
- offline execution, no PATH mutation, and environment-unavailable exit 7 for
  a missing or incompatible binary.

The Rust distribution contract rejects version, protocol, capabilities,
standards, Skill-byte, manifest, target, path, file-type, or unexpected-content
drift before packaging. A plugin bundle never falls back to an environment
binary and never downloads a replacement.

## Build one native artifact

Build Mino and invoke the maintainer xtask on the same operating system and
architecture as the requested target:

```text
cargo build --release --locked --bin mino
cargo run --release --locked --bin xtask -- package-plugin \
  --repository . \
  --binary target/release/mino \
  --target x86_64-unknown-linux-gnu \
  --output dist
```

On Windows, use `target/release/mino.exe` and
`x86_64-pc-windows-msvc`. Packaging is host-native only. The declared targets
are:

- `x86_64-pc-windows-msvc`
- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

`.github/workflows/release-artifacts.yml` defines one native runner for each
target. The workflow fetches locked dependencies, validates the source
contract, builds the native CLI, and assembles/smokes the artifact. It has
read-only repository permissions and no upload, release, publish, secret, or
marketplace step.

## Artifact layout and verification

Each target output directory contains exactly three files:

```text
dist/<target>/
|-- SHA256SUMS
|-- artifact-manifest.json
`-- mino-plugin-<version>-<target>.zip
```

The ZIP contains one `mino/` plugin tree: canonical source files, the complete
MIT and Apache-2.0 license texts, and exactly one `bin/mino` or `bin/mino.exe`.
Entries are sorted, stored without compression, timestamped at the ZIP epoch,
and normalized to mode 0644 for data or 0755 for the binary. The manifest binds
every entry path, byte count, mode, and SHA-256 digest plus archive, source,
Skill, protocol, standards, target, and capabilities identities.

Before use, verify `SHA256SUMS`, parse the canonical
`mino.plugin-artifact-manifest/v1` manifest, and require the expected target and
archive name. The xtask performs the same strict verification and refuses
absolute, parent, duplicate, unsorted, symbolic-link, special, missing, extra,
changed, compressed, timestamp-drifted, mode-drifted, or digest-drifted entries.
An existing identical target directory is reused; a mismatched directory is not
overwritten.

## Isolated smoke and installation boundary

Before publishing its local output, the xtask extracts the verified ZIP into a
temporary installation and runs the archived binary through four bounded
probes:

```text
mino --version
mino agent capabilities
mino project doctor
mino agent context
```

HOME, USERPROFILE, and temporary directories point into the smoke root. The
host PATH is passed through unchanged only so read-only Git discovery can work;
the bundle is invoked by its exact absolute path and PATH is never modified.
Smoke performs no network access and creates no user installation or
marketplace state.

Installation is a separate user-authorized operation outside the build. After
verifying a target artifact, extract the `mino/` directory as one plugin root
using the installation mechanism supported by the active Codex environment.
Keep `.codex-plugin/plugin.json`, `launcher.json`, `skills/`, licenses, README,
and `bin/` together. The Skill resolves only the binary declared relative to
its own plugin root. Run the launcher-declared capabilities probe before doctor
or plan work.

If the binary is absent, wrong-platform, non-regular, incompatible, or reports
different version/capability identities, stop with environment-unavailable
exit 7 guidance. Do not modify PATH, search for another Mino, download one, or
mix files from different artifacts.

## Upgrade, rollback, and publication

Cargo package version is authoritative. An upgrade must change the canonical
source and native binary together, regenerate every target artifact, rerun all
compatibility probes, and produce new checksums. Never replace only the Skill,
launcher, or binary inside an existing bundle.

Keep the prior verified artifact for rollback. Roll back by selecting the
complete prior target bundle, not by copying individual files. Project-local
`.mino` migration remains governed by `mino protocol status` and
`mino protocol migrate`; installing a plugin does not authorize a protocol
migration or a plan mutation.

The repository artifact workflow validates only. Uploading files, publishing a
release, creating a marketplace entry, or changing a user installation requires
a separate explicit action and authorization.

## Deliberate non-goals

The plugin is not an arbitrary plugin runtime, package manager, updater,
downloader, daemon, cloud service, Web UI, or execution sandbox. It contributes
one declarative Skill and one native Mino binary. It does not add MCP servers,
Apps, hooks, telemetry, auto-update, or hidden Git/network behavior.
