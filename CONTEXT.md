# Skill Management

Skill Management identifies coding-agent skills in user-provided repositories and lets users choose among the discovered skills.

## Language

**Source Repository**:
A repository supplied by a user as the origin from which Skills are discovered.
_Avoid_: Skill repository, catalog

**Skill**:
A directory containing a `SKILL.md` file, together with its entire directory subtree. Its name is the name of that directory.
_Avoid_: Plugin, extension

**Skill Identity**:
The combination of a Source Repository and a Skill's repository-relative directory path. Skill names are display labels and do not need to be unique.
_Avoid_: Skill name

**Nested Skill**:
A Skill whose directory is inside another Skill's subtree. Both remain independently discoverable, and their nesting relationship is retained.
_Avoid_: Sub-skill

**Skill Selection**:
The persisted subset of discovered Skills chosen by the user for subsequent use. Selection does not mean installation.
_Avoid_: Installed skills
