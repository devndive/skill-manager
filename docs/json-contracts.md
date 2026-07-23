# Versioned JSON contracts

`discover --json`, `select --json`, and `list --json` each write one JSON
document to standard output. Diagnostics are written to standard error.
Failures return a non-zero status and write no JSON to standard output.

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
