#!/usr/bin/env python3
"""Stamp the CI build number into the package version, in the CI checkout only.

Cargo.toml keeps the human-chosen MAJOR.MINOR (its patch number is ignored). A
release is built as MAJOR.MINOR.<commits on this branch>, so every push to main
is a strictly newer, reproducible version without anyone editing a file. The
change is never committed: it is applied to the runner's working tree before the
build, so the binary's CARGO_PKG_VERSION, the release tag and the signed
manifest all agree, and `cargo --locked` still sees a matching Cargo.lock.

    python scripts/stamp_ci_version.py            # stamp in place
    python scripts/stamp_ci_version.py --dry-run  # print what would be stamped
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

TOML = Path("crates/jka-client/Cargo.toml")
LOCK = Path("Cargo.lock")


def commit_count():
    out = subprocess.run(["git", "rev-list", "--count", "HEAD"], check=True,
                         capture_output=True, text=True).stdout.strip()
    count = int(out)
    if count < 1:
        raise SystemExit("no commits found")
    if subprocess.run(["git", "rev-parse", "--is-shallow-repository"], check=True,
                      capture_output=True, text=True).stdout.strip() == "true":
        raise SystemExit("shallow checkout: use actions/checkout with fetch-depth: 0, "
                         "otherwise the commit count (and so the version) is wrong")
    return count


def replace_once(path, pattern, make_new):
    raw = path.read_bytes()
    match = re.search(pattern, raw)
    if not match:
        raise SystemExit(f"{path}: package version not found")
    new = make_new(match)
    return raw[:match.start()] + new + raw[match.end():], match


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    count = commit_count()

    # [package] is the first table; its first `version = "x.y.z"` line is the package's.
    toml_bytes, match = replace_once(
        TOML, rb'(?m)^version = "(\d+)\.(\d+)\.\d+"',
        lambda m: b'version = "%s.%s.%d"' % (m.group(1), m.group(2), count))
    version = "%s.%s.%d" % (match.group(1).decode(), match.group(2).decode(), count)

    lock_bytes, _ = replace_once(
        LOCK, rb'(name = "DinurdoJK"\r?\nversion = ")[^"]+(")',
        lambda m: m.group(1) + version.encode() + m.group(2))

    print(f"version {version}")
    if args.dry_run:
        return 0
    TOML.write_bytes(toml_bytes)
    LOCK.write_bytes(lock_bytes)
    return 0


if __name__ == "__main__":
    sys.exit(main())
