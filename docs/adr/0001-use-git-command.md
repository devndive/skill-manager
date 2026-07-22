# Use the Git command for repository access

The Rust CLI will invoke the installed `git` executable to resolve revisions and inspect repository trees for both local and remote sources. This keeps Git behavior consistent and mature without taking on libgit2 packaging costs or the complexity of a pure-Rust Git implementation; the trade-off is that users must have a compatible Git executable installed.
