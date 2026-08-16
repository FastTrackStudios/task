# airlock: the external build volume

airlock (the headless Mac mini that builds, signs and uploads the iOS and
macOS apps) has a 228 GB internal disk that fills chronically. In August
2026 a 2 TB external SSD was added and the build-heavy directories moved
onto it.

## Layout

| path | lives on | how |
|---|---|---|
| `~/fts` | SSD | symlink → `/Volumes/build-disk/home/fts` |
| `~/.cargo` | SSD | symlink → `/Volumes/build-disk/home/cargo` |
| `~/task-macos-deploy` | SSD | symlink → `/Volumes/build-disk/home/task-macos-deploy` |
| runner `_work` | SSD | **not** a symlink — the runner is configured with `--work /Volumes/build-disk/home/runner-work` |
| `/nix` | internal | deliberately left alone |
| `~/Library` | internal | Xcode caches + simulator runtimes; prune via Xcode |

Result: internal free space went 22 GB → 65 GB, Data volume 88% → 64%.

## The thing that will bite you: TCC

**A launchd-spawned process cannot read `/Volumes/build-disk` unless it has
Full Disk Access.** macOS TCC (`SystemPolicyRemovableVolumes`) blocks it.

This is nasty to diagnose because the denial is *silent*:

- The GitHub Actions runner logs `Validating directory permissions for:
  '/Volumes/build-disk/home/runner-work'` and then **stops** — no
  exception, no error, 0% CPU, and the service reports "Started".
- GitHub shows the runner as **offline**, so jobs queue forever.
- **Every check over SSH succeeds**, because sshd has the access the
  LaunchAgent does not. So the evidence available through your shell
  actively points away from the cause.

Test it in the failing process's own context, not yours:

```sh
launchctl submit -l tcctest -- /bin/sh -c \
  'ls /Volumes/build-disk > /tmp/tcctest.out 2>&1; echo EXIT=$? >> /tmp/tcctest.out'
sleep 3; cat /tmp/tcctest.out; launchctl remove tcctest
```

`Operation not permitted` means TCC, whatever the ownership bits say.

### The fix

Grant Full Disk Access to the runner binary. **This requires the GUI** —
Screen Sharing into airlock, then System Settings → Privacy & Security →
Full Disk Access → add:

```
/Users/rat/actions-runner/bin/Runner.Listener
```

TCC permissions pass to child processes, so `Runner.Worker`, `xcodebuild`
and cargo all inherit it. `tccutil` can only *reset* grants, never create
them, and editing `TCC.db` by hand needs SIP disabled — there is no
supported SSH-only path. MDM with a PPPC payload is the alternative if you
ever manage more than one Mac.

**FDA is per-binary.** If the runner self-updates and replaces
`Runner.Listener`, the grant may not follow it. If the runner silently
stops taking jobs after an update, check this first.

### Two dead ends, recorded so nobody repeats them

- **Ownership.** External volumes mount with ownership disabled, so
  `diskutil enableOwnership /Volumes/build-disk` is correct and worth
  doing — but it is **not** what fixes the runner. It was enabled, and the
  runner still hung identically.
- **The symlink.** Runners are known to dislike a symlinked `_work`, and
  pointing `--work` at an absolute path is the right call anyway — but the
  hang was byte-for-byte the same either way.

Both looked well-supported and both were wrong.

## Automount

Every path above depends on `/Volumes/build-disk` being mounted. After a
reboot or power cut where it does not mount, the runner fails in exactly
the silent way described above, and the three symlinks dangle with
confusing "no such file" errors rather than an obvious "disk missing".

Add an `/etc/fstab` entry (`vifs`) or a LaunchDaemon that mounts it before
the runner agent starts.

## Reclaiming more space

`~/Library` is the remaining large consumer (~26 GB), mostly Xcode
simulator runtimes. Two were mounted at ~98% full:

- `iOS 24A5380i` — 17 GB
- `watchOS 23T570` — 8.5 GB

Delete unused runtimes through Xcode → Settings → Platforms rather than
relocating them.
