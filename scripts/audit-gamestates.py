"""Compare initial gamestates field-for-field with the compiled OpenJK oracle.

Run under WSL after building the Windows jka-probe and reference harness:
python3 scripts/audit-gamestates.py /mnt/d/Games/JKA/GameData/base/demos
Only the first demo record is examined, even if the recording is later truncated.
"""
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    directory = Path(sys.argv[1]).resolve()
    oracle = ROOT / ".references/codec-harness/codec-oracle"
    probe = ROOT / "target/debug/jka-probe.exe"
    entries = []
    for demo in sorted(directory.glob("*.dm_26"), key=lambda path: path.name.lower()):
        reference = subprocess.run([str(oracle), str(demo)], capture_output=True, text=True, timeout=30)
        windows_path = subprocess.check_output(["wslpath", "-w", str(demo)], text=True).strip()
        rust = subprocess.run([str(probe), "gamestate-dump", windows_path], capture_output=True, text=True, timeout=30)
        match = reference.returncode == rust.returncode == 0 and reference.stdout == rust.stdout
        entry = dict(name=demo.name, sha256=hashlib.sha256(demo.read_bytes()).hexdigest(),
                     matched=match, reference_exit=reference.returncode, rust_exit=rust.returncode)
        if match:
            lines = rust.stdout.splitlines()
            entry.update(configstrings=sum(line.startswith("cs\t") for line in lines),
                         preceding_commands=sum(line.startswith("cmd\t") for line in lines),
                         baselines=sum(line.startswith("base\t") for line in lines),
                         metadata=lines[0].split("\t")[1:],
                         canonical_sha256=hashlib.sha256(rust.stdout.encode()).hexdigest())
        else:
            entry.update(reference_error=reference.stderr.strip(), rust_error=rust.stderr.strip())
            mismatch = ROOT / ".references/gamestate-mismatches"
            mismatch.mkdir(exist_ok=True)
            (mismatch / (demo.name + ".reference.tsv")).write_text(reference.stdout)
            (mismatch / (demo.name + ".rust.tsv")).write_text(rust.stdout)
        entries.append(entry)
        print(f"{'MATCH' if match else 'FAIL'} {demo.name}", flush=True)
    if not entries:
        raise ValueError("No .dm_26 files found")
    fixture_manifest = json.loads((ROOT / "crates/jka-protocol/tests/fixtures/reference.json").read_text())
    report = dict(scope="Initial gamestate only; raw configstring bytes, all 132 fields per entity baseline, metadata and consumed bits/bytes",
                  reference_revision=fixture_manifest["revision"],
                  reference_scope=fixture_manifest["scope"],
                  entity_layout=fixture_manifest["entity_layout"],
                  oracle_sha256=hashlib.sha256(oracle.read_bytes()).hexdigest(),
                  probe_sha256=hashlib.sha256(probe.read_bytes()).hexdigest(),
                  metadata_columns=["message_sequence", "reliable_acknowledge", "server_command_sequence", "client_number", "checksum_feed", "consumed_bits", "consumed_bytes"],
                  files=entries)
    (ROOT / "Development Docs/gamestate-audit.json").write_text(json.dumps(report, indent=2) + "\n")
    matched = sum(entry["matched"] for entry in entries)
    print(f"{matched}/{len(entries)} initial gamestates match.")
    if matched != len(entries):
        sys.exit(1)


if __name__ == "__main__":
    main()
