// Dynamic-light RT receiver controls.
struct RtShadowReceiverFlags {
    flags: vec4<u32>,
};
@group(3) @binding(15) var<uniform> rt_shadow_flags: RtShadowReceiverFlags;
