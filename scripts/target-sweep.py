#!/usr/bin/env python3
"""target-sweep — keep a cargo target dir to "the latest build of each kind".

    scripts/target-sweep.py run <kind> [--keep-last N] -- <command...>
    scripts/target-sweep.py sweep [--dry-run] [--protect-hours H] [--incremental-keep K]
                                  [--keep-days D] [--flycheck-max-gb G]
    scripts/target-sweep.py status

`run` executes ANY build command (cargo, `just ci-*`, nextest, dx) and
records which compilation units it used — rebuilt or fresh — as the
manifest of build kind <kind>. `sweep` deletes every unit that is not in
the latest manifest of SOME kind. So a host that builds a server, a
desktop app and the check suite keeps all three warm and nothing else:
old commits' variants, abandoned feature sets and stale test binaries go.

How "used" is detected: cargo reads `.fingerprint/<unit>/<hash-file>` for
every unit in its graph, fresh or not. Before the command we set every
fingerprint file's atime just below its mtime; under `relatime` (the Linux
default) the next read then bumps atime, so after the command a unit was
used iff a file in its fingerprint dir has atime or mtime >= the start.
This works with any command and needs no replay with --message-format=json.
It needs a filesystem that updates atime (relatime or strictatime), and it
checks that first: on `noatime` it refuses rather than delete live units.

Why not `cargo sweep --stamp/--file`: that also uses atime, but without
the reset. Under relatime a file already read since its last write does
not get a new atime, so a unit reused by two builds within 24 h looks
unused to the second one and is deleted. And `cargo sweep --maxsize`
evicts oldest-first: once a run's working set exceeds the cap it evicts
the run's OWN first-built artifacts, which is why CI rebuilt from cold.

Incremental dirs (`<profile>/incremental/<crate>-<id>`) are not named by
unit hash, so they are kept per crate name: the newest K (default 2) and
anything written within --protect-hours; the rest are orphans of old
dependency graphs.

Two policies, one tool:

  CI runner targets   `sweep --protect-hours 0`: exactly the latest build
                      of each kind the runner does, nothing else.
  local / canonical   `sweep --keep-days 7 --flycheck-max-gb 40`: also keep
                      anything ANY build used in the last 7 days — the
                      Captain's `dx serve`, an IDE, a bare `cargo run` are
                      not wrapped as kinds. `run` folds the atimes it resets
                      into .sweep/seen.json first, so wrapping a build never
                      erases that history. rust-analyzer's target/flycheck*
                      is never swept unit by unit (no kind covers it); past
                      the cap it is removed whole.

The target dir is $CARGO_TARGET_DIR, else `--target-dir`, else ./target
at the workspace root. Manifests live in <target>/.sweep/<kind>/.

`run` and `sweep` hold <target>/.sweep/lock, so one target dir serves one
of them at a time. airlock's release runners share a target dir per build
kind: cargo's own lock covers compiling, but not dx bundling, the atime
bookkeeping here, or a sweep under a build still in flight.
"""
from __future__ import annotations

import argparse
import collections
import contextlib
import fcntl
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HASH = re.compile(r"^(?:lib)?(?P<name>.+?)-(?P<hash>[0-9a-f]{16})(?:\..*)?$")
UNIT_DIRS = (".fingerprint", "build", "deps", "examples")


def log(msg: str) -> None:
    print(f"target-sweep: {msg}", file=sys.stderr, flush=True)


def find_target(arg: str | None) -> Path:
    if arg:
        return Path(arg).resolve()
    env = os.environ.get("CARGO_TARGET_DIR")
    if env:
        return Path(env).resolve()
    d = Path.cwd().resolve()
    for p in (d, *d.parents):
        if (p / "Cargo.lock").exists():
            return (p / "target").resolve()
    sys.exit("target-sweep: no Cargo.lock above the cwd; pass --target-dir")


def profile_dirs(target: Path):
    """Every dir holding a .fingerprint: target/<profile>, target/<triple>/<profile>."""
    if not target.is_dir():
        return
    for a in target.iterdir():
        # rust-analyzer's own target (`flycheck0`, …) is not a build any
        # kind records: it is capped by size instead (see cmd_sweep).
        if not a.is_dir() or a.name.startswith(".") or a.name.startswith("flycheck"):
            continue
        if (a / ".fingerprint").is_dir():
            yield a
        for b in a.iterdir() if a.is_dir() else ():
            if b.is_dir() and (b / ".fingerprint").is_dir():
                yield b


def fingerprint_units(target: Path):
    for prof in profile_dirs(target):
        fp = prof / ".fingerprint"
        for u in fp.iterdir():
            m = HASH.match(u.name)
            if m and u.is_dir():
                yield prof, u, m.group("hash")


def atime_works(target: Path) -> bool:
    """Does a read bump atime here once atime < mtime? (False on noatime.)"""
    target.mkdir(parents=True, exist_ok=True)
    fd, p = tempfile.mkstemp(dir=target, prefix=".sweep-probe-")
    try:
        os.write(fd, b"x")
        os.close(fd)
        st = os.stat(p)
        os.utime(p, ns=(st.st_mtime_ns - 10**9, st.st_mtime_ns))
        with open(p, "rb") as f:
            f.read()
        return os.stat(p).st_atime_ns >= st.st_mtime_ns
    finally:
        os.unlink(p)


def reset_atimes(target: Path) -> int:
    # Before erasing them, fold the atimes into .sweep/seen.json: they are
    # the only record of builds nobody wrapped (dx serve, an IDE, a plain
    # cargo run), and `sweep --keep-days` reads them from there.
    seen_path = target / ".sweep" / "seen.json"
    try:
        seen = json.loads(seen_path.read_text())
    except (OSError, ValueError):
        seen = {}
    n = 0
    for _prof, u, h in fingerprint_units(target):
        for f in u.iterdir():
            try:
                st = f.stat()
                if st.st_atime > seen.get(h, 0):
                    seen[h] = st.st_atime
                os.utime(f, ns=(st.st_mtime_ns - 10**9, st.st_mtime_ns))
                n += 1
            except OSError:
                pass
    horizon = time.time() - 60 * 86400
    seen = {h: t for h, t in seen.items() if t >= horizon}
    seen_path.parent.mkdir(parents=True, exist_ok=True)
    tmp = seen_path.with_suffix(".tmp")
    tmp.write_text(json.dumps(seen))
    tmp.replace(seen_path)
    return n


def used_since(target: Path, t0_ns: int) -> set[str]:
    used = set()
    for _prof, u, h in fingerprint_units(target):
        try:
            for f in u.iterdir():
                st = f.stat()
                if st.st_atime_ns >= t0_ns or st.st_mtime_ns >= t0_ns:
                    used.add(h)
                    break
        except OSError:
            pass
    return used


def manifests(target: Path) -> dict[str, list[Path]]:
    root = target / ".sweep"
    out: dict[str, list[Path]] = {}
    if root.is_dir():
        for kind in sorted(root.iterdir()):
            if kind.is_dir():
                out[kind.name] = sorted(kind.glob("*.json"))
    return out


@contextlib.contextmanager
def target_lock(target: Path):
    d = target / ".sweep"
    d.mkdir(parents=True, exist_ok=True)
    with open(d / "lock", "w") as f:
        try:
            fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            log(f"waiting for {d / 'lock'}: another build is using {target}")
            fcntl.flock(f, fcntl.LOCK_EX)
            log("lock acquired")
        yield


def cmd_run(a) -> int:
    target = find_target(a.target_dir)
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", a.kind):
        sys.exit("target-sweep: kind must be [A-Za-z0-9_.-]+")
    if not a.command:
        sys.exit("target-sweep: run needs a command after --")
    if not atime_works(target):
        log(f"{target} does not update atime (noatime?): running WITHOUT recording")
        return subprocess.call(a.command)
    t_reset = time.time()
    n = reset_atimes(target)
    t0 = time.time_ns()
    log(f"kind={a.kind}: reset {n} fingerprint atimes in {time.time() - t_reset:.1f}s")
    rc = subprocess.call(a.command)
    used = used_since(target, t0)
    if rc != 0 and not a.record_failed:
        log(f"command failed (rc={rc}); manifest for {a.kind} NOT updated")
        return rc
    d = target / ".sweep" / a.kind
    d.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%S", time.gmtime(t0 / 1e9))
    rec = {
        "kind": a.kind,
        "started": t0 / 1e9,
        "finished": time.time(),
        "rc": rc,
        "cwd": os.getcwd(),
        "command": a.command,
        "units": sorted(used),
    }
    tmp = d / f".{stamp}.json.tmp"
    tmp.write_text(json.dumps(rec))
    tmp.replace(d / f"{stamp}.json")
    for old in sorted(d.glob("*.json"))[: -a.keep_last]:
        old.unlink()
    log(f"kind={a.kind}: recorded {len(used)} units ({d / (stamp + '.json')})")
    return rc


def tree_size(p: Path) -> int:
    if p.is_file() or p.is_symlink():
        return p.lstat().st_blocks * 512
    s = 0
    for r, _ds, fs in os.walk(p):
        for f in fs:
            try:
                s += os.lstat(os.path.join(r, f)).st_blocks * 512
            except OSError:
                pass
    return s


def newest_mtime(p: Path) -> float:
    m = p.lstat().st_mtime
    if p.is_dir():
        for r, ds, fs in os.walk(p):
            for f in fs + ds:
                try:
                    m = max(m, os.lstat(os.path.join(r, f)).st_mtime)
                except OSError:
                    pass
    return m


def remove(p: Path, dry: bool) -> int:
    s = tree_size(p)
    if not dry:
        if p.is_dir() and not p.is_symlink():
            shutil.rmtree(p, ignore_errors=True)
        else:
            try:
                p.unlink()
            except OSError:
                pass
    return s


def newest_atime_shallow(p: Path) -> float:
    """Newest atime/mtime of the files directly in a fingerprint dir."""
    m = 0.0
    try:
        for f in p.iterdir():
            st = f.stat()
            m = max(m, st.st_atime, st.st_mtime)
    except OSError:
        pass
    return m


def cmd_sweep(a) -> int:
    target = find_target(a.target_dir)
    ms = manifests(target)
    keep: set[str] = set()
    for kind, files in ms.items():
        for f in files:
            keep |= set(json.loads(f.read_text())["units"])
    if not keep and a.keep_days is None:
        log(f"no manifests under {target}/.sweep — refusing to sweep (record a build first, or pass --keep-days)")
        return 2
    cutoff = time.time() - a.protect_hours * 3600
    used_cutoff = time.time() - a.keep_days * 86400 if a.keep_days is not None else None
    try:
        seen = json.loads((target / ".sweep" / "seen.json").read_text())
    except (OSError, ValueError):
        seen = {}
    freed = collections.Counter()
    for prof in list(profile_dirs(target)):
        # A unit is protected if its fingerprint dir was written recently:
        # an in-flight or unrecorded build (another user, dx, rust-analyzer).
        # With --keep-days, also if any build READ it within that many days
        # (cargo reads every fingerprint in its graph, fresh or not; under
        # relatime that bumps atime at most once a day — the right grain).
        recent = set()
        for u in (prof / ".fingerprint").iterdir():
            m = HASH.match(u.name)
            if not m:
                continue
            if newest_mtime(u) >= cutoff or (
                used_cutoff is not None
                and max(newest_atime_shallow(u), seen.get(m.group("hash"), 0)) >= used_cutoff
            ):
                recent.add(m.group("hash"))
        live_names = set()
        for sub in UNIT_DIRS:
            d = prof / sub
            if not d.is_dir():
                continue
            for e in d.iterdir():
                m = HASH.match(e.name)
                if not m:
                    continue
                h = m.group("hash")
                if h in keep or h in recent:
                    live_names.add(m.group("name").replace("-", "_"))
                    continue
                freed[sub] += remove(e, a.dry_run)
        inc = prof / "incremental"
        if inc.is_dir():
            by_crate = collections.defaultdict(list)
            for e in inc.iterdir():
                if e.is_dir():
                    by_crate[e.name.rsplit("-", 1)[0]].append((newest_mtime(e), e))
            for crate, dirs in by_crate.items():
                dirs.sort(reverse=True)
                k = a.incremental_keep if crate in live_names else 0
                for i, (m, e) in enumerate(dirs):
                    if i < k or m >= cutoff:
                        continue
                    freed["incremental"] += remove(e, a.dry_run)
    # rust-analyzer's flycheck target: a check-only build of the same graph,
    # rebuilt by the IDE in the background. No kind records it, so it is
    # bounded by size: past the cap it goes whole and the IDE rebuilds it
    # (one cold `cargo check`, ~3.5 min on THEBATTLESHIP).
    if a.flycheck_max_gb is not None:
        for fc in sorted(target.glob("flycheck*")):
            if fc.is_dir():
                s = tree_size(fc)
                if s > a.flycheck_max_gb * 1e9:
                    freed[fc.name] += remove(fc, a.dry_run)
    verb = "would free" if a.dry_run else "freed"
    tot = sum(freed.values())
    detail = ", ".join(f"{k} {v / 1e9:.1f} GB" for k, v in sorted(freed.items()))
    log(f"{verb} {tot / 1e9:.1f} GB from {target} ({detail or 'nothing'}); "
        f"kept {len(keep)} recorded units from {len(ms)} kind(s): {', '.join(ms)}")
    return 0


def cmd_status(a) -> int:
    target = find_target(a.target_dir)
    for kind, files in manifests(target).items():
        for f in files:
            r = json.loads(f.read_text())
            age = (time.time() - r["started"]) / 3600
            print(f"{kind:24} {f.name}  {len(r['units']):6} units  {age:6.1f} h ago  rc={r['rc']}  {' '.join(r['command'])[:80]}")
    return 0


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--target-dir")
    sp = p.add_subparsers(dest="cmd", required=True)
    r = sp.add_parser("run", help="run a build and record the units it used as <kind>")
    r.add_argument("kind")
    r.add_argument("--keep-last", type=int, default=1, help="manifests kept for this kind (default 1: the latest build)")
    r.add_argument("--record-failed", action="store_true", help="record even if the command fails")
    r.add_argument("command", nargs=argparse.REMAINDER)
    s = sp.add_parser("sweep", help="delete units not used by the latest build of any kind")
    s.add_argument("--dry-run", action="store_true")
    s.add_argument("--protect-hours", type=float, default=2.0, help="never delete units written this recently (default 2)")
    s.add_argument("--incremental-keep", type=int, default=2, help="incremental dirs kept per live crate (default 2)")
    s.add_argument("--keep-days", type=float, help="also keep every unit any build used within this many days "
                   "(local trees, where dx/IDE/plain cargo builds are not wrapped as kinds)")
    s.add_argument("--flycheck-max-gb", type=float, help="delete target/flycheck* (rust-analyzer) whole when bigger than this")
    sp.add_parser("status", help="list the recorded kinds")
    a = p.parse_args()
    if a.cmd == "run" and a.command and a.command[0] == "--":
        a.command = a.command[1:]
    if a.cmd == "status":
        return cmd_status(a)
    with target_lock(find_target(a.target_dir)):
        return {"run": cmd_run, "sweep": cmd_sweep}[a.cmd](a)


if __name__ == "__main__":
    sys.exit(main())
