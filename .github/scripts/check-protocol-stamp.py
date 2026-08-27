#!/usr/bin/env python3
"""Assert a built .lgx's plugin carries the `logos_protocol_version` liblogos gates on.

WHY THIS EXISTS
---------------
`logos_protocol_version` is the one number that decides whether the host will
load a module: liblogos compares its MAJOR against its own before handing the
file to QPluginLoader. A module that carries the field takes the **Allow** path.
A module that does not takes **AllowLegacy**:

    Module swap carries no usable logos_protocol_version
    (pre-protocol build) - loading permissively

`swap` 0.4.4 shipped exactly that way while `swap_ui` 0.4.4 shipped stamped
0.2.0, and NOTHING went red: the build was green, `lgx verify` was green, the
manifest round-trip check (check-lgx-manifest.py) was green, the release
published, and Basecamp loaded the module anyway because the current host is
permissive. The defect only becomes visible the day the protocol major moves --
as a silent load of an incompatible module, which is precisely the failure the
gate exists to prevent.

The mechanism is worth stating, because it will recur, and it is the same shape
as issue #60: NOTHING IN THIS REPOSITORY SETS THE FIELD. logos-module-builder
stamps it in preConfigure, from the logos-protocol header the whole stack links,
ahead of cmake and moc -- so whether a module is stamped is a property of the
`logos-module-builder` pin in that module's flake.nix/flake.lock and of nothing
else. `swap-module` sat on b15a3724, which predates the stamping commit
(logos-module-builder#113) entirely, so every `swap` ever released was
unstamped, and STILL IS: no repin reaches a stamped, protocol-0.2.0, buildable
`swap`. Measured, rev by rev:

  * 33bcd1c (#113) is the first rev that stamps. It stamps 0.1.0 -- a SKEW
    against swap_ui's 0.2.0 -- and its Qt split reshapes the generated
    swap_qt_glue.h, so swap-module's own preConfigure splice
    ("private:\n    SwapImpl m_impl;", which injects the delivery adapter)
    matches nothing and the build fails.
  * 24cec35 (#119) through ~03ad946 still stamp 0.1.0, and move Qt-glue
    generation to logos-qt-generator; logos-cpp-generator drops --backend qt,
    which swap-module's preConfigure calls directly. Build fails there too.
  * logos-protocol reaches 0.2.0 only around 6ef42ea (2026-07-01) -- well past
    that removal -- and by 79aeeab the generated glue is the cdylib set
    (swap_cdylib_glue.*, swap_module_impl.cpp), which does not match
    swap-module/CMakeLists.txt's SOURCES either.
  * From 01bb03f (#175) core universal modules consume deps through the Qt-free
    `lp` header set with no fallback, and delivery_module v0.1.1 publishes
    neither `lidl` nor `headers-lp` -- evaluation fails outright.

So closing this needs a real migration of swap-module onto the cdylib glue
(plus, past 01bb03f, a coordinated delivery_module bump), not a pin bump. Until
that lands, this script REPORTS rather than gates -- see --report-only, and the
step that uses it in .github/workflows/build-modules.yml. Flip that step to the
asserting form in the same change that fixes the stamp.

So this check reads the answer out of the ARTIFACT, on every build leg, for
every module and variant. A pin that stops stamping turns the PR red and says
which pin to look at.

WHERE THE STAMP LIVES, AND WHERE IT DOES NOT
--------------------------------------------
Only in the plugin binary's embedded Qt plugin metadata (Q_PLUGIN_METADATA,
which moc writes as CBOR from the metadata.json present at configure time --
i.e. the copy the builder had just stamped). That is the copy liblogos reads.

It is NOT in the .lgx manifest.json, and -- measured on real artifacts, not
assumed -- it is NOT in the bundled `variants/<variant>/metadata.json` either:
nix-bundle-lgx packages the repo's metadata.json, which the build-tree stamp
never reaches. So a JSON-only check would report "unstamped" for a module that
is in fact stamped, and this script deliberately does not look there.

WHAT IS CHECKED
---------------
1. The plugin binary named by the manifest's per-variant `main` map carries a
   `logos_protocol_version` key in its CBOR plugin metadata, with a parseable
   dotted version as its value. Parsed as CBOR text strings rather than grepped:
   a loose byte grep would also match the field name wherever it appears in
   linked liblogos code, and report a stamp on an unstamped module.

2. Every plugin passed in one invocation carries the SAME version. The two
   modules in this repo must be built against one logos-protocol; a skew means
   `swap` and `swap_ui` would present different protocol majors to the same
   host. `--expect-version` additionally pins the value when a caller knows it.

Runs standalone, so the same command that runs in CI can be run by hand, in
either of two modes:

    # a freshly BUILT bundle, before anything installs it
    python3 .github/scripts/check-protocol-stamp.py \\
        --lgx /nix/store/...-logos-swap-module-lgx-0.4.4 \\
        --variant darwin-arm64

    # the plugins as INSTALLED, which is where the host will find them; more
    # than one is checked for agreement as well as presence
    python3 .github/scripts/check-protocol-stamp.py \\
        --plugin ~/.logos/modules/swap/swap_plugin.so \\
        --plugin ~/.logos/ui-plugins/swap_ui/swap_ui_plugin.so

`--lgx` accepts the .lgx itself or a directory containing exactly one (which is
what `nix build .#lgx-portable` produces: `$out/<name>.lgx`).

NOTE FOR THE NEXT PERSON: a .lgx is a GZIPPED TAR, not a zip (same as
check-lgx-manifest.py). This script uses python's tarfile so there is no way to
get it wrong.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import tarfile

FIELD = "logos_protocol_version"

# CBOR text-string header for the field NAME: major type 3 (0x60) + length 22.
# Anchoring on the header byte is what separates a real metadata key from the
# same 22 characters appearing as a C string literal in linked liblogos code.
FIELD_KEY = bytes([0x60 + len(FIELD)]) + FIELD.encode()

VERSION_RE = re.compile(r"^\d+\.\d+\.\d+")


def die(message):
    print(f"::error::{message}", file=sys.stderr)
    sys.exit(2)


def cbor_text_at(data, offset):
    """Decode a definite-length CBOR text string at `offset`, or None.

    Only the two lengths a semver can need: the immediate form (0x60|len, up to
    23 bytes) and the one-byte-length form (0x78). Anything else is not the
    shape moc writes here and is treated as "no value".
    """
    if offset >= len(data):
        return None
    head = data[offset]
    if 0x60 <= head <= 0x77:
        length, start = head - 0x60, offset + 1
    elif head == 0x78 and offset + 1 < len(data):
        length, start = data[offset + 1], offset + 2
    else:
        return None
    if start + length > len(data):
        return None
    try:
        return data[start : start + length].decode("utf-8")
    except UnicodeDecodeError:
        return None


def embedded_stamp(payload):
    """The `logos_protocol_version` value in a plugin's CBOR metadata, or None."""
    for match in re.finditer(re.escape(FIELD_KEY), payload):
        value = cbor_text_at(payload, match.end())
        if value is not None and VERSION_RE.match(value):
            return value
    return None


def resolve_lgx(path):
    if os.path.isdir(path):
        found = sorted(f for f in os.listdir(path) if f.endswith(".lgx"))
        if len(found) != 1:
            die(f"expected exactly one .lgx in {path}, found {found or 'none'}")
        return os.path.join(path, found[0])
    return path


def read_member(tar, name, lgx_path):
    try:
        member = tar.getmember(name)
    except KeyError:
        die(f"{lgx_path} contains no {name}")
    handle = tar.extractfile(member)
    if handle is None:
        die(f"{lgx_path}: {name} is not a regular file")
    return handle.read()


def stamp_from_lgx(lgx_arg, variant):
    """(label, plugin display name, stamp) for one variant inside a .lgx."""
    lgx_path = resolve_lgx(lgx_arg)
    if not os.path.isfile(lgx_path):
        die(f"no such .lgx: {lgx_path}")
    try:
        tar = tarfile.open(lgx_path, "r:gz")
    except tarfile.ReadError as exc:
        die(
            f"could not read {lgx_path} as a gzipped tar ({exc}). "
            "A .lgx is tar+gzip -- if this looks like a zip, the bundler changed format."
        )
    with tar:
        manifest = json.loads(read_member(tar, "manifest.json", lgx_path))
        main_map = manifest.get("main")
        if not isinstance(main_map, dict) or variant not in main_map:
            die(
                f"{lgx_path}: manifest.json `main` has no entry for variant {variant!r} "
                f"(got {json.dumps(main_map, ensure_ascii=False)}), so the plugin binary "
                f"to inspect cannot be named. check-lgx-manifest.py covers the shape of "
                f"this field."
            )
        name = f"variants/{variant}/{main_map[variant]}"
        return manifest.get("name", lgx_path), name, embedded_stamp(read_member(tar, name, lgx_path))


def stamp_from_plugin(path):
    """(label, plugin display name, stamp) for a plugin binary on disk."""
    if not os.path.isfile(path):
        die(f"no such plugin binary: {path}")
    with open(path, "rb") as handle:
        return os.path.basename(path), path, embedded_stamp(handle.read())


def unstamped_failure(name):
    return (
        f"{name} carries no {FIELD} in its embedded Qt plugin metadata. "
        f"liblogos reads that block before loading -- not metadata.json on disk "
        f"and not the .lgx manifest -- so this module takes the AllowLegacy path "
        f'("carries no usable {FIELD} (pre-protocol build) - loading '
        f'permissively") and its compatibility is never checked at all. Nothing '
        f"in this repository sets the field: logos-module-builder stamps it in "
        f"preConfigure from the logos-protocol header, ahead of moc. So this "
        f"module's `logos-module-builder` pin predates the stamping commit "
        f"(logos-module-builder#113), or has been moved back behind it. Do NOT "
        f"hand-write the field -- the builder overwrites metadata.json on the "
        f"next build. See this file's header for what a fix actually costs: for "
        f"`swap` no repin is sufficient, because every stamping rev also needs "
        f"the cdylib-glue migration."
    )


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--lgx", help="path to a built .lgx, or the dir containing it")
    ap.add_argument(
        "--variant", help="variant directory inside the .lgx, e.g. darwin-arm64 (with --lgx)"
    )
    ap.add_argument(
        "--plugin",
        action="append",
        default=[],
        metavar="PATH",
        help="an installed plugin binary; repeatable, and repeats must agree",
    )
    ap.add_argument("--label", default=None, help="name for this check in log output")
    ap.add_argument(
        "--expect-version",
        default=None,
        help="require this exact stamp value on every plugin checked",
    )
    ap.add_argument(
        "--report-only",
        action="store_true",
        help=(
            "print the findings as ::notice and exit 0. For the period while "
            "`swap` is known to build unstamped: failing every PR on a defect "
            "that needs someone else's release would gate the repo on work it "
            "does not control, but the answer still belongs in every run's log."
        ),
    )
    args = ap.parse_args()

    if bool(args.lgx) == bool(args.plugin):
        die("pass exactly one of --lgx (with --variant) or one or more --plugin")
    if args.lgx and not args.variant:
        die("--lgx needs --variant")

    if args.lgx:
        bundle_label, name, stamp = stamp_from_lgx(args.lgx, args.variant)
        label = args.label or f"{bundle_label} ({args.variant})"
        checked = [(name, stamp)]
    else:
        label = args.label or "installed plugins"
        checked = [(name, stamp) for _, name, stamp in map(stamp_from_plugin, args.plugin)]

    print(f"=== protocol stamp check: {label} ===")

    failures = []
    for name, stamp in checked:
        if stamp is None:
            failures.append(unstamped_failure(name))
        else:
            print(f"  [ ok ] {name}: {FIELD} {stamp}")

    # Presence is per plugin; agreement is the point of checking several at once.
    found = {stamp for _, stamp in checked if stamp is not None}
    if len(found) > 1:
        failures.append(
            f"protocol SKEW across the plugins checked: {sorted(found)}. Both modules "
            f"must be built against ONE logos-protocol, or they present different "
            f"protocol majors to the same host. The version comes from each module's "
            f"own logos-module-builder pin, so this means the two pins disagree about "
            f"logos-protocol."
        )

    if args.expect_version is not None:
        print(f"  expected : {args.expect_version}")
        for name, stamp in checked:
            if stamp is not None and stamp != args.expect_version:
                failures.append(
                    f"{name}: {FIELD} is {stamp!r}, expected {args.expect_version!r}."
                )

    print()
    if failures:
        level = "notice" if args.report_only else "error"
        print(f"{'REPORTED' if args.report_only else 'FAILED'}: {label} protocol "
              f"stamp check")
        print()
        for failure in failures:
            print(f"  * {failure}")
            print(f"::{level}::{label}: {failure}")
        return 0 if args.report_only else 1

    print(f"OK: {label} takes the protocol gate's Allow path")
    return 0


if __name__ == "__main__":
    sys.exit(main())
