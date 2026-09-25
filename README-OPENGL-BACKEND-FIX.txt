DinurdoJK WGPU backend cleanup
==============================

Changed files:
  Cargo.lock
  build.ps1
  crates/jka-client/Cargo.toml

What changed:
- The direct wgpu dependency now disables default features and enables only:
  std, wgsl, dx12, vulkan, static-dxc.
- smaa 0.20.0 is still used, including its required GLSL shader-input feature.
  build.ps1 creates a repo-local vendor/smaa-0.20.0 from the Cargo cache (or,
  on a fresh machine, the exact crates.io 0.20.0 archive) and changes only its
  wgpu dependency to default-features = false.
- Cargo.lock treats smaa 0.20.0 as the local path package.

Expected final WGPU features include dx12, vulkan, wgsl, glsl and static-dxc,
but NOT gles, metal or webgpu. GLSL is shader input translated by Naga; it is
not the OpenGL rendering backend.

After building, verify with:
  cargo tree -e features -i wgpu@29.0.4

and optionally inspect the release EXE imports. opengl32.dll/WGL should no
longer be present due to WGPU's GLES backend.
