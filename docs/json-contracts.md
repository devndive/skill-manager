# Versioned JSON contracts

`discover --json`, `select --json`, `list --json`, and `sync --json` each write
one JSON document to standard output. Diagnostics are written to standard
error. Failures return a non-zero status and write no JSON to standard output.

All current contracts use `"schema_version": 1`.

`select --json` emits JSON when a Skill Selection is persisted. If an
interactive user declines the replacement, the command succeeds without a
selection document and writes `Skill Selection unchanged.` as human-readable
output. Integrations that require JSON on every successful invocation must use
the non-interactive `--all` or `--select PATH` forms.

## Shared values

A Source Repository object contains:

- `type`: `local` or `github`.
- `path`: its canonical identity. Local paths are absolute and lexically
  normalized. GitHub paths use
  `https://github.com/<owner>/<repository>`.

A discovered Skill contains:

- `identity.source`: the canonical Source Repository identity.
- `identity.path`: the repository-relative Skill path.
- `name`: the final path component, or the Source Repository name for `.`.
- `path`: the repository-relative Skill path.
- `parent_path`: the nearest containing Skill path for a Nested Skill,
  otherwise `null`.

Skills are ordered by repository-relative path. Names are display labels and
can be duplicated; Skill Identity remains unique.

## `discover --json`

The top-level object contains `source`, `requested_revision`,
`resolved_commit`, and `skills`. This example includes a root Skill, a Nested
Skill, and duplicate names:

```json
{
  "schema_version": 1,
  "source": {
    "type": "github",
    "path": "https://github.com/example/agent-skills"
  },
  "requested_revision": "v1.2.0",
  "resolved_commit": "0123456789abcdef0123456789abcdef01234567",
  "skills": [
    {
      "identity": {
        "source": "https://github.com/example/agent-skills",
        "path": "."
      },
      "name": "agent-skills",
      "path": ".",
      "parent_path": null
    },
    {
      "identity": {
        "source": "https://github.com/example/agent-skills",
        "path": "alpha/review"
      },
      "name": "review",
      "path": "alpha/review",
      "parent_path": "."
    },
    {
      "identity": {
        "source": "https://github.com/example/agent-skills",
        "path": "beta/review"
      },
      "name": "review",
      "path": "beta/review",
      "parent_path": "."
    },
    {
      "identity": {
        "source": "https://github.com/example/agent-skills",
        "path": "nested"
      },
      "name": "nested",
      "path": "nested",
      "parent_path": "."
    }
  ]
}
```

## `select --json`

The selection contract adds `manifest_path` to the discovery fields. `skills`
contains only the persisted Skill Selection:

```json
{
  "schema_version": 1,
  "manifest_path": "skills.toml",
  "source": {
    "type": "local",
    "path": "/work/agent-skills"
  },
  "requested_revision": "HEAD",
  "resolved_commit": "89abcdef0123456789abcdef0123456789abcdef",
  "skills": [
    {
      "identity": {
        "source": "/work/agent-skills",
        "path": "alpha/review"
      },
      "name": "review",
      "path": "alpha/review",
      "parent_path": null
    }
  ]
}
```

## `list --json`

The top-level object contains:

- `manifest_path`: the supplied manifest path, or `skills.toml`.
- `sources`: selected Source Repositories, ordered by canonical source path
  and then source type.

Each source contains `source`, `requested_revision`, `resolved_commit`, and
`skills`. Listed Skills omit `parent_path` because nesting is not persisted.
This example shows multiple sources:

```json
{
  "schema_version": 1,
  "manifest_path": "skills.toml",
  "sources": [
    {
      "source": {
        "type": "local",
        "path": "/work/agent-skills"
      },
      "requested_revision": "HEAD",
      "resolved_commit": "89abcdef0123456789abcdef0123456789abcdef",
      "skills": [
        {
          "identity": {
            "source": "/work/agent-skills",
            "path": "."
          },
          "name": "agent-skills",
          "path": "."
        }
      ]
    },
    {
      "source": {
        "type": "github",
        "path": "https://github.com/example/shared-skills"
      },
      "requested_revision": "main",
      "resolved_commit": "0123456789abcdef0123456789abcdef01234567",
      "skills": [
        {
          "identity": {
            "source": "https://github.com/example/shared-skills",
            "path": "review"
          },
          "name": "review",
          "path": "review"
        }
      ]
    }
  ]
}
```

`remove` has no JSON mode.

## `sync --json`

The top-level object contains:

- `manifest_path`: the supplied manifest path, or `skills.toml`.
- `destination`: the explicit target or the manifest-relative
  `.agents/skills` default.
- `created`: newly Materialized Skills ordered by destination name.
- `updated`: replaced or recreated managed Skills ordered by destination name.
- `removed`: formerly managed Skills no longer selected, ordered by destination
  name.
- `unchanged`: clean managed Skills whose recorded identity, commit, and digest
  still match, ordered by destination name.

Each classified Skill contains its Skill Identity, destination `name`, recorded
`resolved_commit`, and deterministic `sha256:` content digest:

```json
{
  "schema_version": 1,
  "manifest_path": "config/skills.toml",
  "destination": "config/.agents/skills",
  "created": [
    {
      "identity": {
        "source": "/work/agent-skills",
        "path": "review"
      },
      "name": "review",
      "resolved_commit": "89abcdef0123456789abcdef0123456789abcdef",
      "digest": "sha256:17f1f9b14f0f13e734a5d7781a9f6a7b732c3e61c1f4e10a506bbbcf502d10c4"
    }
  ],
  "updated": [],
  "removed": [],
  "unchanged": []
}
```

Synchronization supports local Source Repositories. Duplicate destination
names, unsupported sources, unavailable recorded commits, unmanaged
destination collisions, and Materialized Skill Drift without `--force` fail
without writing a JSON document.
