"""Analyze recorded app ticks; never infer capture settings from archived cfg files."""

import argparse
import csv
import json
import math
from collections import Counter
from pathlib import Path


LIVE_SETTINGS = (
    "cl_maxpackets", "cl_commandRate", "cl_commandPacing", "cl_packetdup",
    "com_maxfps", "cl_timeNudge", "cg_nopredict", "cg_errorDecay", "rate", "snaps",
    "pred_backend", "pred_pmove_fixed", "pred_pmove_float", "pred_pmove_msec", "pred_stepslide",
)


def number(row, column):
    return float(row[column])


def vector(row, prefix):
    return tuple(number(row, prefix + axis) for axis in ("x", "y", "z"))


def difference(left, right):
    return tuple(a - b for a, b in zip(left, right))


def length(v):
    return math.sqrt(sum(x * x for x in v))


def analyze(path):
    with path.open(newline="", encoding="utf-8-sig") as handle:
        reader = csv.DictReader(handle)
        columns = reader.fieldnames or []
        rows = list(reader)
    if not rows:
        raise ValueError(f"{path}: no rows")
    if len(set(columns)) != len(columns):
        raise ValueError(f"{path}: duplicate column names")
    for index, row in enumerate(rows, 2):
        if None in row or any(value is None for value in row.values()):
            raise ValueError(f"{path}:{index}: row does not match the header")
    duration = (number(rows[-1], "t_ms") - number(rows[0], "t_ms")) / 1000
    misses = {}
    for row in rows:
        seq = int(row["miss_seq"])
        if seq:
            misses.setdefault(seq, row)
    commands = {}
    states_at_command_time = {}
    snapshots = {}
    corrections = []
    for index, row in enumerate(rows):
        command_time = int(row.get("cmd_time") or int(row["server_time"]) - int(row["cmd_age"]))
        commands.setdefault(int(row["cmd_no"]), command_time)
        states_at_command_time.setdefault(command_time, row)
        snapshots.setdefault(int(row["snap_msg"]), row)
        if index and int(row["miss_seq"]):
            previous = rows[index - 1]
            corrections.append({
                "t_ms": number(row, "t_ms"),
                "miss_units": number(row, "miss_len"),
                "new_snapshot": row["snap_msg"] != previous["snap_msg"],
                "new_command": row["cmd_no"] != previous["cmd_no"],
                "ground": int(row["comm_ground"]),
                "display_step_units": length(difference(vector(row, "disp_"), vector(previous, "disp_"))),
                "camera_step_units": length(difference(vector(row, "cam_"), vector(previous, "cam_"))),
                "view_error": vector(row, "verr_"),
            })
    # Match the first prediction of a committed command to the later server acknowledgement.
    # These are player origins, not pixel-space measurements or presented frames.
    divergences = []
    for snapshot in snapshots.values():
        predicted = states_at_command_time.get(int(snapshot["snap_cmdtime"]))
        if predicted is None:
            continue
        delta = difference(vector(snapshot, "snap_"), vector(predicted, "comm_"))
        divergence = {
            "snapshot": int(snapshot["snap_msg"]),
            "command": int(predicted["cmd_no"]),
            "command_time": int(snapshot["snap_cmdtime"]),
            "distance_units": length(delta),
            "server_minus_prediction": delta,
            "predicted_ground": int(predicted["comm_ground"]),
            "server_ground": int(snapshot["snap_ground"]),
        }
        if "comm_vx" in columns and "snap_vx" in columns:
            divergence["predicted_velocity"] = tuple(number(predicted, "comm_v" + axis) for axis in ("x", "y", "z"))
            divergence["server_velocity"] = tuple(number(snapshot, "snap_v" + axis) for axis in ("x", "y", "z"))
        divergences.append(divergence)
    command_times = list(commands.values())
    gaps = Counter(b - a for a, b in zip(command_times, command_times[1:]))
    report = {
        "file": str(path),
        "sampling": "app_tick; render stalls and final GPU poses are not measured",
        "rows": len(rows),
        "columns": len(columns),
        "duration_seconds": duration,
        "settings": {name: sorted({row[name] for row in rows}) if name in columns else "unavailable in capture"
                     for name in LIVE_SETTINGS},
        "corrections": len(misses),
        "corrections_per_second": len(misses) / duration if duration else None,
        "corrections_with_new_snapshot": sum(event["new_snapshot"] for event in corrections),
        "grounded_corrections": sum(int(row["comm_ground"]) == 1022 for row in misses.values()),
        "max_tick_ms": max(number(row, "dt_ms") for row in rows),
        "command_gaps_ms": dict(sorted(gaps.items())),
        "largest_corrections": sorted(corrections, key=lambda event: event["miss_units"], reverse=True)[:8],
        "matched_snapshots": len(divergences),
        "largest_server_divergences": sorted(divergences, key=lambda event: event["distance_units"], reverse=True)[:8],
        "model_origin_max_offset_units": max(length(difference(vector(row, "disp_"), vector(row, "rend_"))) for row in rows),
    }
    if "tx_packets" in columns and duration:
        for column in ("tx_packets", "tx_empty_packets", "tx_cmd_transmissions", "udp_sent", "udp_send_errors"):
            report[column + "_per_second"] = (number(rows[-1], column) - number(rows[0], column)) / duration
    sidecar = path.with_suffix(".json")
    if sidecar.exists():
        report["metadata"] = json.loads(sidecar.read_text(encoding="utf-8"))
        connection = (report["metadata"].get("connection_at_recording")
                      or report["metadata"].get("connection_at_mark"))
        if connection and connection.get("kind") == "solo":
            report["session_kind"] = "solo"
            report["command_gaps_ms"] = "not usercmds in Solo; cmd_no counts local snapshots"
            report["matched_snapshots"] = "not a network prediction comparison in Solo"
            report["largest_server_divergences"] = []
            report["sampling"] = "Solo app ticks; pose/animation state is recorded, GPU presented frames are not measured"
            report["settings_note"] = "Network cvars are recorded but do not set Solo physics cadence"
        if connection and "server_id" in columns:
            recorded_ids = sorted({int(row["server_id"]) for row in rows})
            report["recorded_server_ids"] = recorded_ids
            report["connection_metadata_matches_rows"] = recorded_ids == [connection.get("server_id")]
            if not report["connection_metadata_matches_rows"]:
                report["metadata_warning"] = (
                    "Connection metadata belongs to a different session than the recorded ticks; "
                    "do not use its address or server settings to identify this capture."
                )
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("captures", type=Path, nargs="*")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    paths = args.captures or [max((Path(__file__).resolve().parents[1] / "target/release/base/hitch").glob("hitch-*.csv"),
                                 key=lambda path: path.stat().st_mtime_ns)]
    text = json.dumps([analyze(path) for path in paths], indent=2)
    if args.output:
        args.output.write_text(text + "\n", encoding="utf-8")
    else:
        print(text)


if __name__ == "__main__":
    main()
