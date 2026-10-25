# Bevy Solari importance-resampling port

Source checkout: `D:\Code\bevy-main`

Source file: `crates/bevy_solari/src/realtime/initial_path.wesl`

Function: `sample_light_ris` (weighted reservoir selection and normalization),
and its caller `generate_nee_candidate` (trace selected sample visibility).

Source file SHA-256 at port time:
`6a68039676956716c03c6b3eaeb6fe28a068f3c58d6a5a39d6d538f8ced6e67e`.
The supplied checkout has no Git metadata.

License: MIT, copied verbatim alongside this file from Bevy Solari.

Adapted code: `crates/jka-client/src/rt_solari_ris.wgsl` and
`rt_sampled_light_loops` in `renderer/lighting/ray_tracing.rs`.
JKA enumerates groups of existing material/emitter contributions instead of
sampling random light tiles. The original inverse-PDF and candidate-MIS factors
cancel for this enumeration; selection and final unbiased weighting retain
Solari's equations. Signed JKA light contributions use absolute-channel luminance.
The first candidate's target is evaluated lazily only when a second nonzero
candidate competes; a singleton has selection probability and weight one.

This port covers direct-light importance resampling. It does not import Solari's
ReSTIR temporal/spatial passes, world GI cache, or DLSS reconstruction.
