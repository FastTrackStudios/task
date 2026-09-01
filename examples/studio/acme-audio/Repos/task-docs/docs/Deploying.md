---
title: Deploying
type: guide
---

# Deploying

The server ships as one OCI image. It carries `git`, so a deployed
server can clone the repositories its wikis mirror, and a CA bundle so
`https://` clones verify.

1. Build the image: `nix build .#task-server-image`.
2. Push it to the registry the cluster pulls from.
3. Update the GitOps tree; the cluster reconciles.

A release's documentation is in the wiki when the release is, because
the wiki mirrors the branch the release was cut from. Nobody re-imports
anything ([[Configuration]] says how often it fetches).
