# Project Domain Hierarchy

_Charter #18 (formerly issue #18). Testable form: [`features/project/spec/project.md`](../../features/project/spec/project.md)._

How intent composes: goals, milestones, workstreams, projects, subprojects and tasks — what nests inside what, and what it means when a level is empty.

Distinct from [The Ideal Project Organization](13-project-organization.md), which defines what a single project *is*. This charter is about the ladder between them.

## Hard requirements

_To be detailed._

Prompts worth answering: Must every task ladder up to something, or is orphaned work legitimate? How deep does project nesting go, and is a subproject simply a project with a parent? What does a goal with no active work mean — a failure signal, or fine? Which levels are real entities and which are views? When a parent moves, is closed, or migrates to another org, what happens to its children? How does progress aggregate upward without becoming fiction? Can one project belong to two parents? What distinguishes a milestone from a due date?

## Non-goals

_To be detailed._

## Current state

Goals, milestones, workstreams, projects and tasks each exist as separate feature slices. Nesting is expressed by frontmatter parent links rather than folders, with uniform recursion as the agreed direction. Two competing definitions of "project" still coexist (frontmatter-driven versus directory-driven in the Files org tree) — unifying on frontmatter is a known requirement.

Related: [The Ideal Project Organization](13-project-organization.md), [The Ideal File Manager](03-file-manager.md).
