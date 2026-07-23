# Synchronization destination state

Every successful Skill Synchronization writes
`.skill-manager-state.json` in the Synchronization Destination. The file is
owned by Skill Manager and records which top-level entries are Materialized
Skills; unrelated destination entries remain unmanaged.

## Version 1

The top-level object contains:

- `state_version`: the integer `1`.
- `owner`: the string `skill-manager`.
- `managed_skills`: Materialized Skills ordered by destination name.

Each managed Skill contains:

- `name`: its flat top-level destination directory.
- `identity`: the canonical Source Repository and repository-relative Skill
  path.
- `resolved_commit`: the exact commit used for materialization.
- `digest`: a deterministic `sha256:` digest of the tracked subtree.

```json
{
  "state_version": 1,
  "owner": "skill-manager",
  "managed_skills": [
    {
      "identity": {
        "source": "/work/agent-skills",
        "path": "review"
      },
      "name": "review",
      "resolved_commit": "89abcdef0123456789abcdef0123456789abcdef",
      "digest": "sha256:17f1f9b14f0f13e734a5d7781a9f6a7b732c3e61c1f4e10a506bbbcf502d10c4"
    }
  ]
}
```

The digest processes tracked regular files in Git tree order. For each file it
hashes the big-endian 64-bit byte length and bytes of the Skill-relative path,
then the length and bytes of the Git file mode, then the length and bytes of
the blob contents. This framing makes names, modes, boundaries, and contents
unambiguous and independent of the current worktree.

The state file is not part of `skills.toml`; using `--target` never changes the
manifest or persists a destination choice.

Repeated synchronization compares each recorded digest with the current
on-disk Materialized Skill before changing the destination. Matching identity,
resolved commit, state, and digest allow the Skill to be reported unchanged
without Source Repository access. A missing managed directory is not drift and
is recreated when still selected.

Digest mismatches are Materialized Skill Drift. Without `--force`, any drifted
managed entry stops the complete reconciliation before mutation. With
`--force`, only entries already listed in `managed_skills` may be replaced or
removed. Destination entries absent from state remain unmanaged and protected.
