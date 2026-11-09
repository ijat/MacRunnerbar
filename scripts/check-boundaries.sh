#!/bin/bash
# scripts/check-boundaries.sh - the architecture rules of CLAUDE.md, derived from what cargo and rustc
# resolve (never from hand-parsed manifests or sources). Fails closed: a workspace member this script
# does not know is an offender.
#
#   1. Dependencies point inward: core depends on no workspace crate (testkit only as a dev-dependency);
#      testkit, github and macos depend on core alone (and testkit as a dev-dependency).
#   2. core's own dependencies are serde and tracing, nothing else (no HTTP client, no UI, no OS crate).
#   3. Only the app depends on lune-ui, directly or through anything else.
#   4. Only the github adapter, and the app that builds it, pull in the HTTP client (ureq).
#   5. `unsafe` only in the macos crate: every other crate compiles with `-F unsafe_code`.
set -euo pipefail
cd "$(dirname "$0")/.."

meta="$(cargo metadata --format-version 1 --locked)"

printf '%s' "$meta" | python3 -c '
import json, sys

m = json.load(sys.stdin)
pk = {p["id"]: p for p in m["packages"]}
nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
members = {pk[i]["name"]: i for i in m["workspace_members"]}

APP = "macrunnerbar-app"
# workspace crate -> workspace crates it may depend on (normal and build kinds), and as dev-dependencies
NORMAL = {
    "macrunnerbar-core": set(),
    "macrunnerbar-testkit": {"macrunnerbar-core"},
    "macrunnerbar-github": {"macrunnerbar-core"},
    "macrunnerbar-macos": {"macrunnerbar-core"},
}
DEV = {
    "macrunnerbar-core": {"macrunnerbar-testkit"},
    "macrunnerbar-testkit": set(),
    "macrunnerbar-github": {"macrunnerbar-testkit"},
    "macrunnerbar-macos": {"macrunnerbar-testkit"},
}
CORE_EXTERNAL = {"serde", "tracing"}
offenders = []

def bad(msg):
    offenders.append(msg)

def direct(pid):
    """(name, kind) of every direct dependency as resolved; kind is normal, build or dev."""
    out = []
    for d in nodes[pid]["deps"]:
        name = pk[d["pkg"]]["name"]
        kinds = d.get("dep_kinds") or []
        if not kinds:
            owner = pk[pid]["name"]
            bad(f"{owner}: dependency {name} has no resolved kind (unrecognised form)")
        for k in kinds:
            out.append((name, k.get("kind") or "normal"))
    return out

def closure(pid, with_dev_root):
    """Every package reachable from pid through normal and build dependencies (plus pid dev ones)."""
    seen, stack = set(), []
    for d in nodes[pid]["deps"]:
        for k in d.get("dep_kinds") or [{"kind": "unknown"}]:
            if (k.get("kind") or "normal") in ("normal", "build") or with_dev_root:
                stack.append(d["pkg"])
    while stack:
        cur = stack.pop()
        if cur in seen:
            continue
        seen.add(cur)
        for d in nodes[cur]["deps"]:
            for k in d.get("dep_kinds") or [{"kind": "unknown"}]:
                if (k.get("kind") or "normal") in ("normal", "build"):
                    stack.append(d["pkg"])
    return {pk[i]["name"] for i in seen}

known = set(NORMAL) | {APP}
for name in members:
    if name not in known:
        bad(f"{name}: an unrecognised workspace member; add it to scripts/check-boundaries.sh with its allowed dependencies")

if APP not in members:
    bad(f"{APP} is missing from the workspace")

for name, pid in members.items():
    if name not in known:
        continue
    deps = direct(pid)
    ws_deps = [(n, k) for n, k in deps if n in members]
    if name != APP:
        for n, k in ws_deps:
            allowed = DEV[name] if k == "dev" else NORMAL[name]
            if n not in allowed:
                bad(f"{name} must not depend on {n} ({k}): dependencies point inward only")
    # lune-ui: only the app, directly or not.
    with_lune = sorted(n for n in closure(pid, True) if n.startswith("lune-ui"))
    if name != APP and with_lune:
        bad(f"{name} depends on {with_lune[0]}: only {APP} may depend on lune-ui")
    if name == APP and not any(n == "lune-ui" for n, _ in deps):
        bad(f"{APP} should depend on lune-ui directly")
    # the HTTP client: the github adapter, and the app that builds it.
    if name not in ("macrunnerbar-github", APP) and "ureq" in closure(pid, True):
        bad(f"{name} pulls in ureq: only macrunnerbar-github (and the app, which builds it) may")

core = members.get("macrunnerbar-core")
if core:
    for n, k in direct(core):
        if n in members:
            continue
        if k == "dev":
            continue
        if n not in CORE_EXTERNAL:
            bad(f"macrunnerbar-core depends on {n}: only {sorted(CORE_EXTERNAL)} are allowed")
    forbidden = {"wgpu", "winit", "ureq", "rustls", "tray-icon", "libc"}
    hit = sorted((closure(core, False) & forbidden) | {n for n in closure(core, False) if n.startswith(("lune-ui", "macrunnerbar-"))})
    if hit:
        bad(f"macrunnerbar-core reaches {hit} through its normal dependencies")

if offenders:
    print("boundary violations:", file=sys.stderr)
    for o in offenders:
        print("  - " + o, file=sys.stderr)
    sys.exit(1)
print("dependency boundaries ok")
'

# `unsafe` only in macrunnerbar-macos: compile every other crate's own target with unsafe_code forbidden,
# whatever its manifest or attributes say. (`cargo rustc` passes the flag to that crate only.)
for crate in macrunnerbar-core macrunnerbar-testkit macrunnerbar-github; do
  cargo rustc --locked -q -p "$crate" --lib -- -F unsafe_code
done
cargo rustc --locked -q -p macrunnerbar-app --bin macrunnerbar -- -F unsafe_code
echo "unsafe only in macrunnerbar-macos: ok"
echo "boundaries ok"
