---
type: resource
resource_kind: sample
slug: single-master-loop
title: Single Master Loop
tags:
- loop
- master
duration_secs: 8
sample_rate: 12000
content_root: ''
content_path: ''
content_id: ''
updated_at: '2026-09-21T10:00:00Z'
source: signal
---
<!-- This directory holds what the sample *is*; the audio lives in the File Root named by `content_root` / `content_path`, pinned to the exact bytes by `content_id`. Reference it as sample:single-master-loop. -->
# Single Master Loop

The bound twin of `room-kick-48k`. That one is deliberately unbound, the
ordinary state of a declared sample; this one shows the other end of the
split, the way an app reaches it.

When the seed is planted it asks the Files lanes for a store of its own
(`RootsService::create`, directory `signal/samples`) and saves the single's
master into it through the upload lane, create-only. Then it binds this
manifest to the result: `content_root` and `content_path` say where the
audio is, and `content_id` pins the exact bytes that save reported. If
someone overwrites `Loops/Single master.wav` later, a reader resolving
`sample:single-master-loop` learns the path has moved on and can still
fetch the recording this manifest named.

Committed unbound because a root's id is minted at plant time. The seed
fills the three fields in.

## Notes

_What it is for._
