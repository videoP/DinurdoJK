# Build artifact policy

- Prefer `build.ps1 -Fast` (or `build.cmd -Fast`) for routine builds. Use `--profile fast` for direct Cargo builds, checks, tests, and Clippy. The build cache is `target/fast`; the build script copies the runnable game into `target/release`.
- Do not create feature-, task-, review-, benchmark-, or session-specific target directories. Do not override `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, or `--target-dir` to isolate work.
- Keep both `target/fast` and `target/release`. Use full release builds for shipping or final performance measurements when needed. Do not use debug or additional custom profiles unless the user explicitly requests them.
- Reuse the existing build cache. If another build holds Cargo's lock, wait or coordinate rather than creating another target directory. Worktrees must reuse the main checkout's target directory instead of creating another build cache.
- Do not copy build trees or binaries into new backup or baseline directories. Disk space is a user constraint.
- Do not delete existing target contents as part of routine work; cleanup requires an explicit user request.
