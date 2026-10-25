#!/usr/bin/env python3
"""Compare DinurdoJK lamp-shadow A/B render_perf_capture CSVs; stdlib only.

Usage: python analyze_lamp_shadow_ab.py renderperf-a.csv renderperf-b.csv [...]
"""
import csv
import math
import statistics
import sys
from pathlib import Path

FIELDS = [
    "frame_ms", "gpu_pass_frame_ms", "gpu_pass_world_ms",
    "gpu_pass_local_shadow_static_ms", "gpu_pass_local_shadow_moving_ms",
    "cpu_lamp_moving_encode_ms", "lamp_dynamic_faces",
    "lamp_entity_face_pairs", "lamp_bsp_face_pairs", "lamp_static_cache_rebuilds",
]

def quantile(values, fraction):
    if not values: return float('nan')
    values = sorted(values)
    index = (len(values)-1)*fraction
    lo = math.floor(index)
    return values[lo] + (values[math.ceil(index)]-values[lo])*(index-lo)

def read(path):
    with path.open(newline='', encoding='utf-8-sig') as stream:
        rows = list(csv.DictReader(stream))
    if not rows: raise ValueError(f'{path}: no frames')
    modes = {r.get('lamp_shadow_ab_mode','') for r in rows}
    if len(modes)>1: print(f'WARNING {path}: multiple A/B modes in one capture: {modes}',file=sys.stderr)
    result={}
    for field in FIELDS:
        samples=[]
        for row in rows:
            try: value=float(row.get(field,''))
            except (ValueError, TypeError): continue
            if math.isfinite(value): samples.append(value)
        result[field] = (statistics.median(samples),quantile(samples,.95),len(samples)) if samples else (float('nan'),float('nan'),0)
    return (next(iter(modes), '?'),len(rows),result)

def main(paths):
    if not paths:
        print(__doc__);return 2
    baseline=None
    for path in paths:
        mode,n,stats=read(Path(path))
        if mode=='0.0' or mode=='0': baseline = baseline or stats
        print(f'\n{Path(path).name} mode={mode} frames={n}')
        for name,(med,p95,count) in stats.items():
            ratio=''
            if baseline and baseline[name][2] and math.isfinite(med) and med>0:
                ratio=f' vs A={med/baseline[name][0]:.3f}x' if baseline[name][0]>0 else ''
            print(f'  {name:37} median={med:9.4f}  p95={p95:9.4f}  n={count:5}{ratio}')
    if not baseline: print('\nTIP: include a baseline CSV captured with r_lampShadowAB 0.')
    return 0

if __name__=='__main__': sys.exit(main(sys.argv[1:]))
