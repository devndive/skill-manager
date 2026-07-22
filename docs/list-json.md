# `list --json` contract

`skill-manager list --json` writes one JSON document to standard output.
Diagnostics are written to standard error, and failures write no JSON to
standard output.

## Version 1

The top-level object contains:

- `schema_version`: the integer `1`.
- `manifest_path`: the manifest path supplied to the command, or `skills.toml`
  when the default is used.
- `sources`: the selected Source Repositories, ordered by canonical source path
  and then source type.

Each source contains:

- `source.type`: `local` or `github`.
- `source.path`: the canonical Source Repository identity. Local paths are
  absolute paths resolved lexically from the manifest directory; GitHub paths
  use the normalized `https://github.com/<owner>/<repository>` form.
- `requested_revision`: the revision recorded in the manifest.
- `resolved_commit`: the commit recorded in the manifest.
- `skills`: selected Skills ordered by repository-relative path.

Each Skill contains:

- `identity.source`: the canonical Source Repository identity.
- `identity.path`: the repository-relative Skill path.
- `name`: the final component of the Skill path, or the Source Repository name
  for the root Skill (`.`).
- `path`: the repository-relative Skill path.

Example:

```json
{
  "schema_version": 1,
  "manifest_path": "skills.toml",
  "sources": [
    {
      "source": {
        "type": "github",
        "path": "https://github.com/example/skills"
      },
      "requested_revision": "main",
      "resolved_commit": "0123456789abcdef",
      "skills": [
        {
          "identity": {
            "source": "https://github.com/example/skills",
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
