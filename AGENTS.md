# DinurdoJK agent instructions

## Project and scope

- DinurdoJK is a Rust/WGPU Jedi Academy / JK2 client. Preserve protocol, gameplay, rendering, and mod compatibility unless the task explicitly changes them.
- Read the affected types, callers, neighboring modules, and existing caches before changing an implementation. Inspect the working tree first and preserve unrelated work.
- For parity work, identify the applicable game/mod behavior and reference implementation. Use OpenJK, TaystJK, and applicable JK2MV/MVSDK sources; if they disagree, state which behavior this path must preserve instead of mixing them silently.
- Keep durable repository rules here. Add a nested `AGENTS.md` only for rules specific to that subtree, and read applicable nested files before editing there. A file under `Development Docs/Patch20` does not govern sibling source trees under `crates/`.
- Keep task findings, measurements, and investigation order in audit documents, not in permanent instructions. See [RUST_AUDIT.md](RUST_AUDIT.md) for the November 11, 2026 handoff review; recheck findings against current code before acting on them.

## Build artifact policy

- Prefer `build.ps1 -Fast` (or `build.cmd -Fast`) for routine builds. Use `--profile fast` for direct Cargo builds, checks, tests, and Clippy. The build cache is `target/fast`. Without `-Output` the build script copies nothing: the exe stays in `target/<mode>/DinurdoJK.exe`. Pass `-Output target\release` to update the game folder.
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

## Rust ownership and architecture

- Treat unnecessary work, unclear ownership, duplicated behavior, and broken invariants as audit targets. Trait counts, function length, and counts of `clone`, `collect`, or `mem::take` are search aids, not proof of poor code or slow execution.
- Before cloning to resolve a borrow conflict, consider a narrower borrow, disjoint fields, a scoped borrow, or a small borrowed view. Retain owned data when it must outlive the borrow, cross a worker/snapshot boundary, or remain independent of later mutation.
- Judge copies by their data and frequency. An `Arc` or WGPU handle clone does not deep-copy the payload, but it is not free. A vector/string clone copies owned data; `Arc::make_mut` may copy a shared payload. Do not replace every clone with `Arc`.
- Use `mem::take` for deliberate ownership transfer or scratch reuse. Check restoration on early returns and errors when capacity must survive. Scratch must not be overwritten while a submitted or queued snapshot still owns its data.
- For repeated frame/entity/particle work, first remove unnecessary work or borrow existing data. Reuse scratch where lifetimes permit. Add a cache only with a defined key, invalidation, lifetime, and memory bound; account for retained capacity and old snapshots.
- Use concrete functions and types for cohesive implementation details. Add traits for interchangeable behavior, useful generic reuse, or a clear capability boundary. Choose static or dynamic dispatch for the actual call path; neither a trait quota nor a blanket `dyn Trait` ban is useful.
- Extract large functions in cohesive steps with explicit inputs, outputs, and ordering. Separate structural refactors from behavior fixes and performance changes. Do not add synchronization, interior mutability, or `unsafe` merely to bypass an ownership problem.

## Performance and compatibility review

- Distinguish a source-confirmed operation from a suspected bottleneck and a measured result. Report the type, call path, frequency, and relevant data size; do not claim FPS or compile-time improvements without measurements.
- Preserve multiplayer frame time, input latency, and the minimal renderer path. Keep lazy pipeline specialization and existing prewarming policy; avoid unnecessary shader variants or eager compilation.
- Visibility and feature toggles do not necessarily stop simulation, interactions, environment updates, or resource preparation. Trace every consumer before gating work.
- Preserve authored material stages, tcMods, shader time, tint, glow, animation, winding, lighting, and PVS behavior when sharing or caching geometry. Retain diagnostic and inspector traceability.
- Use existing diagnostics first. Benchmark the affected scenario with the same map, camera/demo, settings, backend, build profile, and warm-up. Compare CPU/GPU time as relevant and frame-time distributions, not FPS alone. State what was not measured.

## Verification and delivery

- Documentation-only and read-only audit tasks do not require compilation. For source changes, use the necessary focused verification under the build/cache policy above; honor explicit task-specific no-build requests and report unrun checks.
- Keep each cleanup small enough to review and validate independently. Prefer existing focused tests; add regression coverage for meaningful behavior or ownership risks rather than tests that only mirror the implementation. Do not run successive build/check/test/Clippy variants without a reason.
- In this checkout, edit files directly when the task calls for changes. Produce incremental patches or ZIPs when requested or when working from an archive without checkout access; packaging is not a default requirement.
- Report the problem, changed files, preserved behavior, verification actually performed, and unresolved risks. An allocation removed in source is not automatically a demonstrated frame-time improvement.
