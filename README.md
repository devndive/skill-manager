# Skill Manager

Skill Manager discovers coding-agent Skills in Git Source Repositories and
persists and synchronizes the subset selected for subsequent use. A Skill is a
tracked directory containing `SKILL.md`; selecting it records its identity,
while synchronization materializes its tracked subtree.

## Requirements

- A `git` executable available on `PATH`. Skill Manager invokes Git for local
  and public GitHub Source Repositories.
- For development installation, Rust 1.85 or newer.

## Install

Download the archive for your platform from
[GitHub Releases](https://github.com/devndive/skill-manager/releases), extract
it, and place `skill-manager` (or `skill-manager.exe`) on `PATH`. Each release
includes `SHA256SUMS`.

| Platform | Architectures |
| --- | --- |
| Linux | x86_64, ARM64 |
| macOS | Intel x86_64, Apple Silicon ARM64 |
| Windows | x86_64, ARM64 |

For development directly from a clone:

```console
cargo install --path . --locked
```

The crate is not published to crates.io.

## Discover Skills

Discover tracked Skills from a local Source Repository:

```console
skill-manager discover ../agent-skills
```

Public GitHub HTTPS URLs are also supported:

```console
skill-manager discover https://github.com/example/agent-skills
```

By default, Skill Manager resolves `HEAD`. Use `--ref` with a branch, tag, or
commit:

```console
skill-manager discover ../agent-skills --ref v1.2.0
skill-manager discover https://github.com/example/agent-skills --ref main
```

The output distinguishes the requested revision from the resolved commit.
Local discovery reads the requested revision from the existing Source
Repository. GitHub discovery temporarily clones the public Source Repository
and removes the clone afterward. Root Skills use path `.`, Nested Skills retain
their parent path, and duplicate names remain distinct because Skill Identity
uses Source Repository plus repository-relative path.

Add `--json` for the versioned machine-readable contract.

## Select Skills

Run `select` in a terminal to choose interactively:

```console
skill-manager select ../agent-skills
```

For scripts and other non-interactive input, use `--all` or one or more exact
`--select` paths:

```console
skill-manager select ../agent-skills --all
skill-manager select ../agent-skills --select review --select nested/testing
```

`--ref` controls the revision as it does for `discover`. The default manifest
is `skills.toml` in the current working directory; use `--manifest FILE` to
choose another location. Re-selecting a Source Repository replaces only that
repository's Skill Selection. Add `--json` to emit the persisted selection.

## List and remove selections

`list` reads the manifest without accessing any Source Repository:

```console
skill-manager list
skill-manager list --manifest config/skills.toml --json
```

`remove` deletes one Source Repository's Skill Selection from the manifest:

```console
skill-manager remove ../agent-skills
skill-manager remove https://github.com/example/agent-skills --yes
```

Removal asks for confirmation unless `--yes` is supplied.

## Synchronize selected Skills

Materialize every selected Skill from a local Source Repository:

```console
skill-manager sync
```

The default Synchronization Destination is `.agents/skills` relative to the
manifest directory. Use `--manifest FILE` to read another manifest and
`--target DIRECTORY` to choose a destination for this invocation only:

```console
skill-manager sync --manifest config/skills.toml
skill-manager sync --manifest config/skills.toml --target ./agent-skills
```

Synchronization is non-interactive and leaves the manifest unchanged. Each
Skill is copied from the exact `resolved_commit` recorded in the manifest, so
uncommitted files and newer commits in a local Source Repository do not affect
the result. Materialized Skills use flat top-level directories by Skill name.
If a parent and Nested Skill are both selected, each is materialized
independently with its complete tracked subtree.

Duplicate Skill names and pre-existing unmanaged destination entries are
rejected before the destination changes. `--force` never adopts or overwrites
unmanaged entries; during initial synchronization it does not change collision
behavior. Successful synchronization writes
`.skill-manager-state.json` in the destination to record managed ownership,
Skill Identity, resolved commits, and deterministic content digests. Add
`--json` for the versioned machine-readable result.

## Contracts and errors

- [`skills.toml` schema](docs/skills-toml.md)
- [Versioned JSON contracts](docs/json-contracts.md)
- [Synchronization destination state](docs/synchronization-state.md)

Human and JSON success output is written to standard output. Diagnostics are
written to standard error, and failures return a non-zero status. A failed JSON
command writes no success-shaped JSON to standard output. Failed or cancelled
selection and removal operations leave an existing manifest unchanged.
Initial synchronization stages all source content before changing the
destination.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE)), or
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option. The package metadata declares `MIT OR Apache-2.0`.
