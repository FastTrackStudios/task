# The Ideal Project Organization

_Charter #13 (formerly issue #13). Testable form: [`features/project/spec/project.md`](../../features/project/spec/project.md)._

What a project *is*, how projects nest, and how everything else hangs off them.

## Hard requirements

_To be detailed._

Prompts worth answering: What makes something a project rather than a task or a note? How deep does nesting go, and is a subproject just a project? What does a project type determine — layout, sync, permissions, lifecycle? What travels with a project when it moves between orgs? What does "done" mean, and who says so? How do tasks, time, files, and money all attach without four different notions of the same project?

## Non-goals

_To be detailed._

## Current state

Projects are `type: project` markdown pages with a frontmatter UUID that survives renames. Two competing definitions currently exist — frontmatter-driven (`scan_vault`) versus directory-driven (the Files org tree hardcodes `Projects/` and `Albums/`) — and unifying on frontmatter is a known requirement, since arbitrary nesting cannot be expressed by two hardcoded directory names.

Related: [The Ideal File Manager](03-file-manager.md).
