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

## Interrupted synchronization recovery

Synchronization stages all Source Repository content before publishing
transaction metadata. Immediately before the first Synchronization Destination
mutation, Skill Manager atomically publishes
`.skill-manager-transaction/journal.json` together with durable staged content
and the intended next destination state.

Journal version 1 records:

- `journal_version`: the integer `1`.
- `owner`: the string `skill-manager`.
- `phase`: `preparing`, `committing`, or `rolling_back`.
- whether the Synchronization Destination existed before the transaction.
- every create, update, and removal operation, including its destination,
  staged content, backup location, prior observed content, and intended digest.
- the backup location for the previous destination state, when present, and
  the staged next destination state.

The transaction directory is temporary Skill Manager-owned data. It is removed
only after the selected Materialized Skills and
`.skill-manager-state.json` are durable.

Every `sync` checks for an incomplete transaction before reading the manifest
or planning new work:

- `preparing` has not changed managed destination content and is discarded.
- `committing` is completed from the recorded durable staged content.
- `rolling_back` restores the recorded backups.

Recovery is idempotent. If recovery is interrupted, the next `sync` repeats the
same recorded phase. Partially committed Materialized Skills remain owned by
the transaction and are not treated as unmanaged collisions. Entries not named
by the journal are never changed.

If destination content changed after interruption and no longer matches either
the recorded prior or intended content, recovery stops without overwriting it.
Malformed journals, unsafe paths, missing required artifacts, and unsupported
journal versions also stop synchronization without destination mutation. The
journal remains in place for diagnosis and manual repair.
