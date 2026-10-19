#!/bin/bash
# Render emotes offline on the empty preview stage (no map) with the in-client
# `emote_gallery` command, several client instances in parallel, and build one
# contact sheet per emote (rows = time samples, each frame = four angles).
#
#   scripts/emote-gallery.sh <testbase dir> <out dir> <first> <count> [samples] [instances]
#   e.g. scripts/emote-gallery.sh target/emote-testbase target/gallery 1 200 6 8
#        scripts/emote-gallery.sh target/emote-testbase target/gallery 12,14,22 - 6 4
#        scripts/emote-gallery.sh target/emote-testbase target/gallery EID_Twist,EID_Scribe - 0 2
#   (a comma list in place of <first> renders those Profile numbers, or those
#   emote ids when it starts with EID_; count is ignored)
#
# The test base needs DinurdoJK.exe, its DLLs and a base/ folder with the game
# PK3s and the emote library. Nothing in a real install is touched. Each
# instance runs from its own folder (hard-linked exe) so logs do not collide.
set -u
BASEDIR=$(cd "$1" && pwd); OUT=$2; FIRST=$3; COUNT=$4; SAMPLES=${5:-6}; N=${6:-8}
mkdir -p "$OUT"
BASE_W=$(cygpath -w "$BASEDIR/base")
pids=(); dirs=()
MODE=nums; [[ "$FIRST" == EID_* ]] && MODE=ids
if [[ "$FIRST" == *,* || $MODE == ids ]]; then
    IFS=',' read -ra NUMS <<< "$FIRST"
    per=$(( (${#NUMS[@]} + N - 1) / N ))
else
    per=$(( (COUNT + N - 1) / N ))
fi
for ((k = 0; k < N; k++)); do
    if [[ "$FIRST" == *,* || $MODE == ids ]]; then
        slice=("${NUMS[@]:$(( k * per )):$per}")
        (( ${#slice[@]} == 0 )) && break
        ARGS=($MODE "$(IFS=,; echo "${slice[*]}")" "$SAMPLES")
    else
        start=$(( FIRST + k * per )); left=$(( FIRST + COUNT - start ))
        (( left <= 0 )) && break
        ARGS=("$start" "$(( per < left ? per : left ))" "$SAMPLES")
    fi
    dir="$BASEDIR/instance-$k"; mkdir -p "$dir"; rm -f "$dir/latest.log"
    for f in "$BASEDIR"/DinurdoJK.exe "$BASEDIR"/*.dll; do
        ln -f "$f" "$dir/" 2>/dev/null || cp -f "$f" "$dir/"
    done
    (cd "$dir" && DJK_HEADLESS=1 ./DinurdoJK.exe --base "$BASE_W" +set com_maxfps 60 +emote_gallery "${ARGS[@]}" > gallery.out 2>&1) &
    pids+=($!); dirs+=("$dir")
    sleep 2   # stagger startup (shared config and shader caches)
done
echo "instances: ${#pids[@]} x ~$per emotes"
# Wait for every instance's done marker (or its exit), then close it.
runs=()
for i in "${!pids[@]}"; do
    log="${dirs[$i]}/latest.log"
    for _ in $(seq 1 14400); do
        grep -aq "EMOTE GALLERY DONE\|emote_gallery: no matching emotes\|RENDERER ERROR" "$log" 2>/dev/null && break
        kill -0 "${pids[$i]}" 2>/dev/null || break
        sleep 1
    done
    run=$(grep -a "EMOTE GALLERY DONE" "$log" 2>/dev/null | tail -1 | sed 's/^EMOTE GALLERY DONE //')
    [ -n "$run" ] && runs+=("$run") || echo "instance $i: no result ($(grep -ao 'no matching emotes\|RENDERER ERROR.*' "$log" | tail -1); see $log)"
done
# Close only this run's instances (never a game the user has open).
for dir in "${dirs[@]}"; do
    exe=$(cygpath -w "$dir/DinurdoJK.exe")
    powershell.exe -NoProfile -Command "Get-CimInstance Win32_Process -Filter \"Name='DinurdoJK.exe'\" | Where-Object { \$_.ExecutablePath -eq '$exe' } | ForEach-Object { Stop-Process -Id \$_.ProcessId -Force }" > /dev/null 2>&1
done
wait 2>/dev/null
python - "$OUT" "${runs[@]}" <<'PY'
import sys, os, re, json, glob, collections
from PIL import Image, ImageDraw
out, runs = sys.argv[1], sys.argv[2:]
emotes = []
for run in runs:
    run = run.strip()
    report = json.load(open(os.path.join(run, 'report.json')))
    emotes += report['emotes']
    by = collections.defaultdict(list)
    for f in sorted(glob.glob(os.path.join(run, '*', '*.png'))):
        m = re.match(r'(\d+)_(.+)_(\d+)$', os.path.basename(os.path.dirname(f)))
        if m: by[(int(m.group(1)), m.group(2))].append(f)
    info = {e['number']: e for e in report['emotes']}
    for (num, eid), files in sorted(by.items()):
        e = info.get(num, {})
        tiles = []
        for f in files:
            im = Image.open(f).convert('RGB'); w, h = im.size
            tiles.append(im.crop((0, int(h * 0.18), w, int(h * 0.82))).resize((640, int(640 * 0.64 * h / w))))
        cols = 3
        tw, th, head = 640, tiles[0].size[1], 46
        rows = (len(tiles) + cols - 1) // cols
        sheet = Image.new('RGB', (tw * cols, head + th * rows), (16, 16, 16))
        d = ImageDraw.Draw(sheet)
        d.text((6, 4), '#%d %s  "%s"  %.1fs  audio=%s  fx=%s(gpu~%s) props=%d  samples=%d' % (num, eid, e.get('title', ''), e.get('seconds', 0), e.get('audio_status', '?'), e.get('effect_renderers', 0), e.get('gpu_approximated', 0), len(e.get('props', [])), len(tiles) - 1), fill=(255, 255, 0))
        warn = '; '.join(e.get('warnings', []) + ([e['error']] if e.get('error') else []))[:220]
        d.text((6, 22), warn or (e.get('presentation_detail') or 'no warnings').replace(chr(10), ' / ')[:220], fill=(255, 120, 120) if warn else (140, 200, 140))
        secs = e.get('seconds', 0)
        for i, t in enumerate(tiles):
            x, y = tw * (i % cols), head + th * (i // cols)
            sheet.paste(t, (x, y))
            label = 'before' if i == 0 else '%.2fs' % (secs * (i - 1) / max(len(tiles) - 2, 1))
            d.text((x + 6, y + 4), label, fill=(255, 255, 255))
        sheet.save(os.path.join(out, '%04d_%s.jpg' % (num, eid)), quality=88)
emotes.sort(key=lambda e: e['number'])
json.dump({'emotes': emotes}, open(os.path.join(out, 'report.json'), 'w'), indent=1)
print('sheets:', len(glob.glob(os.path.join(out, '*.jpg'))), 'emotes:', len(emotes), 'runs:', len(runs))
PY
