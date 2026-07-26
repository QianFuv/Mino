# Mino Codex plugin source

This directory is the canonical, validation-ready source for the Mino Codex
plugin. It is not a native install artifact by itself: target packaging adds
exactly one `bin/mino` or `bin/mino.exe` binary while preserving every source
file byte.

The bundled Skill is copied byte-for-byte from `assets/skill/mino` and guarded
by the Rust plugin contract. `launcher.json` pins the CLI, protocol, Agent
schemas/capabilities, embedded standards, supported targets, relative binary
layout, and non-interactive probe argv used before any plan work.

The Skill resolves the binary relative to its own plugin root. Installation and
execution must not mutate `PATH`, download a replacement, use an alternate Mino
binary, or perform network access. A missing, wrong-platform, or incompatible
binary is an environment-unavailable failure with exit code 7 and requires a
matching native artifact.

Validate this source with the current plugin-creator validator and the Rust
contract before packaging. This source does not publish, install, update, or
create a marketplace entry.
