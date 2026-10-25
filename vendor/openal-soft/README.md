# OpenAL Soft runtime

The local source archive is upstream OpenAL Soft **1.25.2**, matching JAEnhanced
PR #10 at a5d7c7d307b541a3387f760553392834d9e8ced0. SHA-256:
`fb27e5839aa11f0e5b9d33756965291fad5d6909ab928ea1f796f4a1a6877894`.

Source: https://github.com/kcat/openal-soft/archive/refs/tags/1.25.2.tar.gz

`scripts/ensure-openal-runtime.ps1` builds this archive with the PR's EAX-enabled
shared-library configuration. It reuses `target/fast/build/openal-soft` and
writes `target/fast/OpenALSoft.x86_64.dll`. `build.ps1` stages the DLL beside the
game and includes the upstream license and four app-local MSVC runtime DLLs
(including `msvcp140_atomic_wait.dll`). No SDL2 dependency is introduced.

This matches the upstream source and feature settings, not the byte identity
of a DLL compiled by another machine/toolchain. The unmodified source archive
includes upstream's LGPL 2.0 license and component notices.
