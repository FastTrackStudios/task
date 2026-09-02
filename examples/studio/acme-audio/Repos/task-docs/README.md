# task-docs

The documentation for ACME's internal tooling, kept beside the code
that it documents. `docs/` is what ships with a release, and it is
also the `Docs` wiki on the org's server: the wiki mirrors this
directory, so a commit here is a page there and nothing is imported
twice.

This file is deliberately *outside* `docs/`, so the seed can show that
a repo-sourced wiki mirrors one path inside a repository rather than
the whole of it.

At plant time the seeder turns this folder into a git repository with
one commit on `main` and points the `Docs` wiki at it.
