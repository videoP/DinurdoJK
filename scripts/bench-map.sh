#!/bin/bash
# Launch a map in the release client, run scripted console commands, and print
# the key log lines (FPS samples, perf lines, viewpos, panics).
#
#   scripts/bench-map.sh <timeout-seconds> <MAP> [+command ...]
#
# Example (FPS at a spot; z = eye height - 37):
#   scripts/bench-map.sh 400 AMJH3TE +set r_perftrace 1 \
#       +setviewpos 140 -458 -5 71 15 +perfsample 3 settle +perfsample 5 run1 +quit
#
# Safety, learned the hard way:
# - The exe and DLLs are copied to a scratch folder and run from there, so
#   latest.log never collides with a game you have open.
# - Any `set` command rewrites target/release/base/DinurdoJK.cfg. The cfg is
#   backed up first and restored on exit (also on Ctrl-C / timeout).
#   If the script is hard-killed, check the cfg with the backup printed below.
# - Screenshots taken by THIS run (newer than a marker touched at start) are
#   moved into <scratch>/shots. Nothing older is ever touched: the user's own
#   screenshots live in the same folder.
#
# Environment:
#   JKA_BENCH_DIR  scratch folder (default: $TMPDIR or /tmp + /dinurdojk-bench)
#   JKA_BENCH_CFG  a cfg file to use for this run only (e.g. r_backend "dx12";
#                  r_reflectionQuality / r_backend are restart-latched, so they
#                  must be changed in the cfg, not with +set)

set -u
if [ $# -lt 2 ]; then
    sed -n '2,25p' "$0"
    exit 2
fi
T=$1
MAP=$2
shift 2

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REL="$REPO/target/release"
BASE="$REL/base"
WORK="${JKA_BENCH_DIR:-${TMPDIR:-/tmp}/dinurdojk-bench}"
RUN="$WORK/run"
SHOTS="$WORK/shots"
CFG="$BASE/DinurdoJK.cfg"
BACKUP="$WORK/DinurdoJK.cfg.backup"

if [ ! -f "$REL/DinurdoJK.exe" ]; then
    echo "No $REL/DinurdoJK.exe - build first (powershell -File build.ps1 -Fast)"
    exit 1
fi
mkdir -p "$RUN" "$SHOTS" "$BASE/screenshots"

cp -f "$REL/DinurdoJK.exe" "$REL"/*.dll "$RUN/"
cp -f "$CFG" "$BACKUP"
trap 'cp -f "$BACKUP" "$CFG"' EXIT
echo "cfg backup: $BACKUP"
if [ -n "${JKA_BENCH_CFG:-}" ]; then
    cp -f "$JKA_BENCH_CFG" "$CFG"
fi

MARK="$WORK/run_marker"
touch "$MARK"
STAMP=$(date +%H%M%S)
rm -f "$RUN/latest.log"

# Some launches stall after render init on a cold cache: keep the timeout long
# and rerun. Single runs vary +/-15%, so compare several launches.
(cd "$RUN" && timeout "$T" ./DinurdoJK.exe --base "$BASE" --map "$MAP" "$@" > run.out 2>&1)
echo "exit=$?"

cp -f "$BACKUP" "$CFG"

find "$BASE/screenshots" -maxdepth 1 -name 'shot*.jpg' -newer "$MARK" | while read -r f; do
    mv "$f" "$SHOTS/${STAMP}_$(basename "$f")"
    echo "shot: $SHOTS/${STAMP}_$(basename "$f")"
done

echo "log: $RUN/latest.log"
grep -aE "PERF SAMPLE|^\(-?[0-9]+ |LOADED|SETVIEWPOS|LOCAL SERVER|panic|PANIC|Out of Memory" "$RUN/latest.log" | cut -c1-260
