---
type: resource
resource_kind: sample
slug: room-kick-48k
title: Room Kick 48k
tags:
- kick
- room
duration_secs: 2
sample_rate: 48000
content_root: ''
content_path: ''
updated_at: '2026-09-05T10:00:00Z'
source: signal
---
<!-- This directory holds what the sample *is*; the audio lives in the File Root named by `content_root` / `content_path`. Reference it as sample:room-kick-48k, a region as sample:room-kick-48k#t:0-2:400. -->
# Room Kick 48k

Deliberately two small files and no audio. A sample library is a
`Collection` of kind `Library` over `sample:<slug>` references, and it
stays cheap to subscribe to precisely because subscribing moves
manifests like this one rather than gigabytes of wavs.

The bytes belong in a File Root, where the versioning, the Peaks
waveform renditions, the selective sync and the chunked streaming
already live. Bind them by setting `content_root` and `content_path` —
`task sample save room-kick-48k --content-root <root> --content-path
'Samples/Kicks/Room Kick 48k.wav'` — after `task files` has put the file
there. Unbound, as here, is the ordinary state of a declared sample.

## Notes

_How it was captured, and what it is for._
