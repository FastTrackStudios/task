---
title: Troubleshooting
type: reference
---

# Troubleshooting

## The wiki says it is stale

A repo-sourced wiki that could not fetch keeps serving the pages it
had and marks them: the wiki's source shows the last error beside the
commit it still reflects. Fix the URL or the credentials, then
`task wiki refresh-source <slug>`; the mark clears on the first
successful fetch.

## A page I edited in the app is not in the repository

Edits to a repo-sourced wiki arrive as Edit Requests. Accepting one
lands it on a branch in the repository — and, when the deployment holds
a forge token for that host, as a pull request. The page shows as
landed only once the repository has it; until the branch merges and
the mirror fetches, the wiki still shows the repository's version.

Back to [[Getting Started]].
