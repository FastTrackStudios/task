#!/usr/bin/env python3
"""Turn a `cargo rail plan --json` (contract v9) into the checks job's gates.

Usage: ci-rail-plan.py PLAN.json  -> prints KEY=VALUE lines for $GITHUB_OUTPUT

Outputs:
  full=true|false    run everything (workspace scope, CI inputs changed, or
                     anything this script can't read exactly)
  cargo=true|false   any Cargo work at all (manifests, clippy, docs steps)
  clippy=true|false  rail requires `cargo.clippy` (with clippy evidence, a
                     docs-only change doesn't; scripts/ci-local.sh)
  tests=true|false   the test step has work
  test_filter=EXPR   nextest filterset for the affected packages ("" = all)
  wasm=true|false    the wasm check has work
  wasm_packages=P…   the packages to check for wasm32, space-separated
                     ("" = every row of .config/rail-wasm.json)
  n_packages=N       size of the test selection (for the log)

Why a filterset and not rail's `-p` list: building a subset of packages
changes Cargo's feature unification, which rebuilds dependencies with
other features and leaves a second artifact set in the warm target dir.
The test BUILD stays `--workspace` (identical units, all fresh except
what changed); only which tests RUN narrows.

Anything unexpected fails open (full=true), never narrow on a guess.
"""
import json
import sys
from pathlib import Path
from typing import NoReturn

# The browser build (`ci-wasm` in the Justfile) is rail's `wasm` variants
# work: one catalog row per package checked for wasm32, so the plan says
# which rows a change reaches through the wasm32 dependency graph.
WASM_WORK = "wasm"
WASM_CATALOG = Path(__file__).resolve().parent.parent / ".config" / "rail-wasm.json"

# Repository work declared in .config/rail.toml whose change means "run
# the full set".
FULL_RUN_WORK = {"ci-infra", "test-runner"}

# Every work id this script knows the meaning of: rail 0.32's built-ins,
# FULL_RUN_WORK and the wasm work. A required id outside it (a newer rail,
# a new rail.toml item) fails open rather than being silently ignored.
KNOWN_WORK = {
    "cargo.build", "cargo.clippy", "cargo.test", "cargo.doc", "cargo.doctest",
    "cargo.fmt", "cargo.package", "dependency-policy", "release.semver", "surface",
    WASM_WORK,
} | FULL_RUN_WORK

# The plan JSON shape this script reads (rail 0.32; docs/planning.md).
CONTRACT = 9


def emit(**kv):
    for k, v in kv.items():
        if isinstance(v, bool):
            v = str(v).lower()
        print(f"{k}={v}")


def full(reason) -> NoReturn:
    print(f"plan: {reason} -> full run", file=sys.stderr)
    emit(full=True, cargo=True, clippy=True, tests=True, test_filter="", wasm=True, wasm_packages="", n_packages="all")
    sys.exit(0)


def wasm_gate(work):
    """(has work, packages) from the `wasm` variants work; packages "" = all rows."""
    item = work.get(WASM_WORK)
    if not isinstance(item, dict):
        full(f"no `{WASM_WORK}` work in the plan (.config/rail.toml)")
    if item.get("state") != "required":
        return False, ""
    sel = (item.get("scope") or {}).get("selection") or {}
    if sel.get("kind") == "all":
        return True, ""
    try:
        rows = {v["id"]: v["cargo_roots"][0]["package"] for v in json.load(open(WASM_CATALOG))["variants"]}
        pkgs = sorted(rows[v["id"]] for v in sel["variants"])
    except Exception as e:  # noqa: BLE001 - any mismatch fails open
        full(f"cannot map the wasm variants to packages ({e})")
    return True, " ".join(pkgs)


def packages(work):
    """Package names of a required Cargo work item, or None for workspace scope.

    From the typed `selection.packages[].name`, not `cargo_args`: when two
    packages share a name, rail's `-p` spec becomes `name@version`, and a
    nextest `package(name@1.0.0)` matches nothing, so those tests would
    silently not run. Target narrowing (`selection.targets`) is ignored:
    the package is a safe superset of its targets.
    """
    scope = work.get("scope") or {}
    sel = scope.get("selection") or {}
    if scope.get("kind") != "cargo" or sel.get("kind") == "workspace":
        return None
    pkgs = sel.get("packages")
    if not isinstance(pkgs, list) or not pkgs:
        return None
    names = {n for p in pkgs if isinstance(p, dict) and isinstance(n := p.get("name"), str) and n}
    return names or None


def main():
    try:
        plan = json.load(open(sys.argv[1]))
    except Exception as e:  # noqa: BLE001 - any read failure fails open
        full(f"unreadable plan ({e})")
    if plan.get("plan_contract_version") != CONTRACT:
        full(f"plan contract {plan.get('plan_contract_version')!r}, this script reads {CONTRACT}")
    if "work" not in plan or "required" not in plan:
        full("plan has no work/required")

    required = set(plan["required"])
    work = plan["work"]

    unknown = required - KNOWN_WORK
    if unknown:
        full(f"unknown required work ({', '.join(sorted(unknown))})")
    hit = required & FULL_RUN_WORK
    if hit:
        full(f"CI inputs changed ({', '.join(sorted(hit))})")

    cargo = bool(required & {"cargo.build", "cargo.clippy", "cargo.test", "cargo.doc"})
    clippy = "cargo.clippy" in required
    wasm, wasm_pkgs = wasm_gate(work)

    if "cargo.test" not in required:
        print("plan: no tests affected", file=sys.stderr)
        emit(full=False, cargo=cargo, clippy=clippy, tests=False, test_filter="", wasm=wasm, wasm_packages=wasm_pkgs, n_packages=0)
        return

    pk = packages(work["cargo.test"])
    if pk is None:
        full("cargo.test has workspace scope")

    # `=`: an exact package name, never a glob or a substring.
    expr = " | ".join(f"package(={p})" for p in sorted(pk))
    emit(full=False, cargo=True, clippy=clippy, tests=True, test_filter=expr, wasm=wasm, wasm_packages=wasm_pkgs, n_packages=len(pk))
    print(f"plan: tests for {len(pk)} package(s): {', '.join(sorted(pk))}", file=sys.stderr)


if __name__ == "__main__":
    main()
