# Build artifact policy

- Prefer `build.ps1 -Fast` (or `build.cmd -Fast`) for routine builds. Use `--profile fast` for direct Cargo builds, checks, tests, and Clippy. The build cache is `target/fast`; the build script copies the runnable game into `target/release`.
- Do not create feature-, task-, review-, benchmark-, or session-specific target directories. Do not override `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, or `--target-dir` to isolate work.
- Keep both `target/fast` and `target/release`. Use full release builds for shipping or final performance measurements when needed. Do not use debug or additional custom profiles unless the user explicitly requests them.
- Reuse the existing build cache. If another build holds Cargo's lock, wait or coordinate rather than creating another target directory. Worktrees must reuse the main checkout's target directory instead of creating another build cache.
- Do not copy build trees or binaries into new backup or baseline directories. Disk space is a user constraint.
- Do not delete existing target contents as part of routine work; cleanup requires an explicit user request.

## Standing memory: prevent incremental-cache growth

- Disk space is a hard user constraint. Reuse existing incremental-cache configurations as well as the shared target directory. Do not create another configuration merely for an agent, task, check, or test run.
- Keep the toolchain, fast profile, target, feature set, and compiler flags consistent with `build.ps1 -Fast`. Direct Cargo tests must use the same compiler flags as the build script. Do not switch between its path-remapping flags and no flags, or repeatedly append duplicate flags: Cargo treats changed configurations as separate caches.
- Cargo legitimately keeps separate game and test artifacts. Reuse those existing entries; do not attempt to force incompatible artifacts into one internal folder or introduce extra profiles to isolate work.
- Check available disk space before substantial compilation. Inspect existing cache configurations and running Cargo processes before assuming a fresh build is necessary. If expected headroom is insufficient, stop compilation and explain the space requirement instead of repeatedly filling the drive.
- Batch source changes before building. Reuse an already-built test executable when its sources are unchanged. Do not run successive build/check/test/Clippy variants for routine verification when the existing build and focused tests suffice.
- Rebuild again only for a material source change, a failure, or an unresolved validation concern. Keep final verification to the necessary game build and focused test run using the existing configuration.
- Never automatically delete caches to make room. Identify specific obsolete folders and their sizes, then obtain the user's explicit cleanup instruction; preserve runnable outputs and current caches.
