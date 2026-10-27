---
"wiki-live" = "patch"
---

Fixed wiki pushes recording the wrong committer on a machine that sets GIT_COMMITTER_NAME or GIT_COMMITTER_EMAIL: the editor who pushed is now always the committer.
