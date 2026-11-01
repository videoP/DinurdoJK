// Native river foam field helpers: Random.ush/RandomPCG.ush simplex curl and
// the selected stage-16 emission criteria. Copyright Epic Games, Inc.
// All Rights Reserved. See foam_field.rs; no replacement noise is supplied.
fn river_foam_rand3d_pcg16(p:vec3<i32>)->vec3<u32> {
    var v=bitcast<vec3<u32>>(p)*1664525u+1013904223u;
    v.x+=v.y*v.z;v.y+=v.z*v.x;v.z+=v.x*v.y;
    v.x+=v.y*v.z;v.y+=v.z*v.x;v.z+=v.x*v.y;
    return v>>vec3<u32>(16u);
}
fn river_foam_gradient(r:u32)->vec3<f32> {
    // MGradientMask int3(0x8000,0x4000,0x2000), MGradientScale 1/(0x4000,0x2000,0x1000).
    return vec3<f32>(vec3<u32>(r)&vec3<u32>(0x8000u,0x4000u,0x2000u))*vec3<f32>(1.0/16384.0,1.0/8192.0,1.0/4096.0)-1.0;
}
// JacobianSimplex_ALU(v,false,1): rows are noise channels; xyz derivatives, w value.
fn river_foam_jacobian_simplex(v:vec3<f32>)->array<vec4<f32>,3> {
    let tet=floor(v+v.x/3.0+v.y/3.0+v.z/3.0);
    let base=tet-tet.x/6.0-tet.y/6.0-tet.z/6.0;
    let f=v-base;
    let g=step(f.yzx,f.xyz);let h=1.0-g.zxy;
    let a1=min(g,h)-1.0/6.0;let a2=max(g,h)-1.0/3.0;
    var corners=array<vec3<f32>,4>(base,base+a1,base+a2,base+0.5);
    var fv:array<vec3<f32>,4>;var gvec:array<array<vec3<f32>,4>,3>;var grad:array<vec4<f32>,3>;
    for(var c=0u;c<4u;c++) {
        fv[c]=v-corners[c];
        let r=river_foam_rand3d_pcg16(vec3<i32>(floor(6.0*corners[c]+0.5)));
        for(var a=0u;a<3u;a++) {gvec[a][c]=river_foam_gradient(r[a]);grad[a][c]=dot(gvec[a][c],fv[c]);}
    }
    let scale=1024.0/375.0;
    let s=saturate(2.0*vec4<f32>(dot(fv[0],fv[0]),dot(fv[1],fv[1]),dot(fv[2],fv[2]),dot(fv[3],fv[3])));
    let sv=1.0*scale+s*(-3.0*scale+s*(3.0*scale-s*scale));
    let ds=-12.0*scale+s*(24.0*scale-s*12.0*scale);
    var j:array<vec4<f32>,3>;
    for(var a=0u;a<3u;a++) {
        // mul(sv,gvec[a]) + mul(ds,grad[a]), with ds[axis][c]=s'[c]*fv[c][axis].
        var blended=vec3<f32>(0.0);var derivative=vec3<f32>(0.0);
        for(var c=0u;c<4u;c++) {blended+=sv[c]*gvec[a][c];derivative+=(ds[c]*fv[c])*grad[a][c];}
        j[a]=vec4<f32>(blended+derivative,dot(sv,grad[a]));
    }
    return j;
}
fn river_foam_curl(xyz:vec3<f32>,offset_from_seed:vec3<f32>)->vec3<f32> {
    let j=river_foam_jacobian_simplex(xyz+offset_from_seed);
    return vec3<f32>(j[1][2]-j[2][1],j[2][0]-j[0][2],j[0][1]-j[1][0]);
}
// Grid2D_SW_EmissionCriteria for nonzero-width ranges.
fn river_foam_criterion(value:f32,low:f32,high:f32,multiplier:f32)->f32 {
    return smoothstep(low,high,value)*multiplier;
}
