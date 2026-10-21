# SMAA vendor

The client uses the published **smaa 0.21.0** release from crates.io. Its Rust
implementation, GLSL shaders, lookup tables, README and license are unchanged.
Only `Cargo.toml` and `Cargo.toml.orig` disable WGPU's default features. The
upstream `glsl` input feature remains enabled; the client selects Vulkan/DX12.

Published archive:
`https://static.crates.io/crates/smaa/smaa-0.21.0.crate`

Archive SHA-256:
`70f1733b8fc96f93ad2ed99e81f6a3f1fc58a23ad1d8d764ed70016911005c27`

The source is included in this checkout, so normal builds are offline. Both
PowerShell builders call `scripts/ensure-smaa-vendor.ps1`. If the vendor is
missing, the helper copies Cargo's cached upstream source, or downloads and
verifies the pinned archive before installing it from a staging directory.
Already-correct manifests are not rewritten. Incomplete or incorrect vendor
trees cause an explicit error. GLSL input support and WGPU 30 are checked.

# wgpu-core vendor

`wgpu-core-30.0.1` is the crates.io release with one change in
`src/device/queue.rs` (`prepare_surface_texture_for_present`, marked
"DinurdoJK patch"). It is wired in through `[patch.crates-io]` in the root
`Cargo.toml`.

wgpu 30 added a step to `Queue::present` that moves the surface texture to
PRESENT and skips its own queue submit when nothing changed. PRESENT is not an
"ordered" texture use on Vulkan, so the tracker always reports a
PRESENT -> PRESENT transition. The skip never fires, and every frame pays a
redundant barrier plus a second `vkQueueSubmit`: about 20 us per present.
That is about 6% FPS on mp/ffa3 at roughly 2500 fps. The patch drops that no-op
transition. Clearing a never-rendered surface still works as upstream intends.

Measured 2026-10-11 with a bare clear+present loop (Vulkan, Immediate):
wgpu 29 present 72 us, wgpu 30 present 93 us, wgpu 30 patched present 72 us.
Remove the vendor and the patch entry once upstream wgpu fixes this.

# wgpu-hal vendor

`wgpu-hal-30.0.1` is the crates.io release with one change in
`src/vulkan/descriptor.rs` (`create_descriptor_pool`, marked "DinurdoJK
patch"). It is wired in through `[patch.crates-io]` in the root `Cargo.toml`.

wgpu-core's `Device::new` always creates an empty bind group layout, and
wgpu-hal's `register_layout` creates a descriptor pool up front for every new
layout type. For an empty layout every pool-size count is zero, so
`vkCreateDescriptorPool` was called with `poolSizeCount = 0`, which is invalid
usage. Some Intel Vulkan drivers (`igvk64.dll`, `iigd_dch` package) fault inside
that call, crashing every launch with `0xC0000005` at `igvk64.dll+0x6181c6`
during renderer init (minidumps 2026-10-12). The patch adds one placeholder
`UNIFORM_BUFFER` pool size when the list would be empty.
Remove the vendor and the patch entry once upstream wgpu fixes this.

Second change, in `src/vulkan/swapchain/native.rs` (`create_swapchain`, marked
"DinurdoJK patch"): on Windows, upstream creates an acquire fence for every
swapchain and makes each `acquire_texture` wait on it on the CPU (added for
vsync pacing, gfx-rs/wgpu#8310 and #8354). With IMMEDIATE/MAILBOX that wait
blocked the render thread until the presentation engine released the image:
on an Intel UHD 630 window the GPU worked ~2 ms per frame while acquire took
~7 ms, about half of TaystJK's Vulkan FPS with the same present mode and image
count. TaystJK passes only a semaphore. The patch keeps the fence for FIFO and
FIFO_RELAXED only. `JKA_VK_ACQUIRE_FENCE=1` restores upstream behaviour.

Third change, diagnostic only, in `src/vulkan/adapter.rs` (`open_with_callback`)
and `src/vulkan/device.rs` (`create_render_pipeline`, `log_shader_stats`), marked
"DinurdoJK patch": with `DINURDO_VK_SHADER_STATS=1` the device enables
`VK_KHR_pipeline_executable_properties`, creates render pipelines with
`CAPTURE_STATISTICS`, and prints each stage's driver statistics to stderr
(`[VK SHADER STATS] <label> FS: Register Count=128, Binary Size=...`). This is how
shader register pressure is measured without Nsight (NVIDIA reports registers;
it offers no internal representations). Without the variable nothing changes.
Example: `blue-water-gpu` test `surface_register_stats`.

# winit vendor

`winit-0.30.13` is the crates.io release with one change in
`src/platform_impl/windows/event_loop.rs` (`handle_raw_input`, marked
"DinurdoJK patch"). It is wired in through `[patch.crates-io]` in the root
`Cargo.toml`.

Upstream tests `has_flag(usFlags, MOUSE_MOVE_RELATIVE)`, but
`MOUSE_MOVE_RELATIVE` is 0, so the test is always true and
`MOUSE_MOVE_ABSOLUTE` raw reports (Remote Desktop, NoMachine, VMs, tablets:
`lLastX/lLastY` are 0..65535 positions) were delivered as
`DeviceEvent::MouseMotion` deltas. On a remote session the view spun at
"sensitivity 999999999" and pinned the pitch. The patch converts absolute
reports to the pixel delta from the previous absolute position (reset on focus)
and leaves relative reports untouched.
Remove the vendor and the patch entry once upstream winit fixes this.

wgpu-core also has a draw-validation patch in `src/command/render.rs`
(`VertexState`) and `src/command/bind.rs` (`BindGroupManager::last_assigned`),
both marked "DinurdoJK patch". wgpu 30 defers `set_vertex_buffer` to the next
draw. Each draw runs `VertexState::validate`, which scans all 16 vertex slots
and the bind-group slots, and `VertexState::flush`, which scans all 16 slots
again even when nothing changed. Render passes are replayed in
`CommandEncoder::finish`, so that is where the cost shows up. The patch keeps
two bitmasks of assigned and dirty slots, and scans the bind-group slots from
the end. Validation and the HAL calls are unchanged.

Measured 2026-10-14 with a headless AMJH3TE-shaped microbench (Vulkan, RTX 3060,
900 draws, 48 pipelines, 640 bind groups, 40 multi-draws): `finish` took
311-315 us on wgpu 29, 326-333 us on wgpu 30, and 315-319 us patched.
Remove this patch once upstream wgpu tracks vertex slots without per-draw scans.

wgpu-core also has a second DinurdoJK patch in `src/instance.rs`:
`Instance::new` logs "failed to create <backend> backend" at `warn` instead of
`debug`, so the reason a backend is unavailable reaches latest.log (otherwise
`create_surface` only reports "any enabled backend: {}").
