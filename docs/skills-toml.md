# `skills.toml` contract

Skill Manager persists Skill Selections in a versioned TOML manifest. The
default path is `skills.toml` in the command's working directory; every command
that reads or writes it accepts `--manifest FILE`.

## Version 1

The top-level `manifest_version` must be the integer `1`. `sources` is an array
of Source Repository tables. Each `[[sources]]` table contains:

- `type`: `local` or `github`.
- `path`: the Source Repository identity. A local path is stored relative to
  the manifest directory when possible. A GitHub path is the normalized
  `https://github.com/<owner>/<repository>` URL.
- `requested_revision`: the branch, tag, commit, or `HEAD` requested by the
  selection.
- `resolved_commit`: the Git commit resolved when the selection was written.
- `skills`: repository-relative Skill paths. `.` identifies a root Skill.

Skill names are not stored because they are derived from paths. Duplicate names
are valid: `alpha/review` and `beta/review` have different Skill Identities.
A Nested Skill is recorded independently from its parent.

This representative manifest contains root, Nested, duplicate-name, local, and
GitHub Skills across multiple Source Repositories:

```toml
manifest_version = 1

[[sources]]
type = "local"
path = "../agent-skills"
requested_revision = "v1.2.0"
resolved_commit = "0123456789abcdef0123456789abcdef01234567"
skills = [".", "nested", "alpha/review", "beta/review"]

[[sources]]
type = "github"
path = "https://github.com/example/shared-skills"
requested_revision = "main"
resolved_commit = "89abcdef0123456789abcdef0123456789abcdef"
skills = ["review", "testing/integration"]
```

Source Repository order and comments are preserved when possible. Skill paths
written by Skill Manager are deterministic. Unknown or malformed versions are
rejected rather than interpreted as version 1.
