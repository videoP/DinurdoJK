//! Automatic, bind-space body shaping for humanoid Ghoul2 meshes.
//!
//! Unlike the optional offline Anny grid, this has no vertex/topology or model-name
//! dependency. Joints and vertices are transformed from the model's OWN bind pose,
//! and weighted anatomy regions prevent the arms/legs from inheriting torso fat.
//! This is intentionally a conservative anatomical approximation, NOT Anny's
//! fixed template topology or a semantic classifier for arbitrary creatures.
use super::{cross, dot, normalize, sub, BodyShapeParams};
use crate::ghoul2::{GlaAnimation, GlmModel};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Region { Torso, LeftArm, RightArm, LeftLeg, RightLeg, Head }

fn bone_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).flat_map(char::to_lowercase).collect()
}
fn landmark(bones: &GlaAnimation, names: &[&str]) -> Option<usize> {
    bones.skeleton.iter().position(|b| {
        let name = bone_name(&b.name);
        names.iter().any(|pattern| name == *pattern || name.ends_with(pattern))
    })
}
fn side(name: &str) -> Option<bool> {
    // true = left; never infer a side from a generic midline bone.
    if name.starts_with('l') || name.contains("left") || name.contains("bip01l") { Some(true) }
    else if name.starts_with('r') || name.contains("right") || name.contains("bip01r") { Some(false) }
    else { None }
}
fn region(name: &str) -> Region {
    let n = bone_name(name);
    if ["face", "cranium", "jaw", "eye", "brow", "lip", "head", "tongue"].iter().any(|s| n.contains(s)) {
        return Region::Head;
    }
    let arms = ["humerus", "radius", "hand", "wrist", "forearm", "upperarm", "finger", "thumb", "metacarpal", "hangtagbone"];
    let legs = ["femur", "tibia", "talus", "tarsal", "thigh", "calf", "shin", "foot", "toe", "tail"];
    // JKA names digits l_d1_j1 etc. Do not confuse other bone prefixes.
    if n == "mc5" { return Region::LeftArm; }
    if n == "mc7" { return Region::RightArm; }
    if arms.iter().any(|s| n.contains(s)) || n.starts_with("ld") || n.starts_with("rd") {
        return match side(&n) { Some(true) => Region::LeftArm, Some(false) => Region::RightArm, None => Region::Torso };
    }
    if legs.iter().any(|s| n.contains(s)) {
        return match side(&n) { Some(true) => Region::LeftLeg, Some(false) => Region::RightLeg, None => Region::Torso };
    }
    Region::Torso
}
fn smooth(x: f32) -> f32 { let t = x.clamp(0.0, 1.0); t*t*(3.0-2.0*t) }
fn bump(z: f32, at: f32, radius: f32) -> f32 {
    let d = (z-at).abs() / radius.max(0.001);
    1.0 - smooth(d)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b-a)*t }
fn dist(a: [f32;3], b: [f32;3]) -> f32 { dot(sub(a,b),sub(a,b)).sqrt() }
fn add(a: [f32;3], b: [f32;3]) -> [f32;3] { std::array::from_fn(|i|a[i]+b[i]) }
fn mul(a: [f32;3], s:f32) -> [f32;3] { a.map(|v|v*s) }
fn nearest_radial(p:[f32;3], a:[f32;3], b:[f32;3]) -> [f32;3] {
    let axis=sub(b,a);
    let t=(dot(sub(p,a),axis)/dot(axis,axis).max(1e-6)).clamp(0.0,1.0);
    sub(p,add(a,mul(axis,t)))
}

/// The skeleton supplies landmarks; no model names, precomputed blendshapes or
/// fixed vertex indices are used. A Source rig converted to Ghoul2 qualifies by
/// exactly the same test as a retail/custom GLM.
struct Anatomy {
    original: Vec<[f32;3]>,
    kinds: Vec<Region>,
    pelvis:[f32;3], chest:[f32;3], neck:[f32;3], head:[f32;3],
    shoulders:[[f32;3];2], elbows:[[f32;3];2], wrists:[[f32;3];2],
    hips:[[f32;3];2], knees:[[f32;3];2], ankles:[[f32;3];2],
    ground:f32,
    params:BodyShapeParams,
}
impl Anatomy {
    fn build(glm:&GlmModel, gla:&GlaAnimation, native:Option<&[[f32;3]]>, params:BodyShapeParams) -> Option<Self> {
        if glm.num_bones < 20 || gla.skeleton.len() < glm.num_bones || glm.lods.is_empty() ||
            native.is_some_and(|p|p.len()!=gla.skeleton.len()) { return None; }
        let p:Vec<_> = gla.skeleton.iter().enumerate().map(|(i,b)| {
            native.map_or_else(|| std::array::from_fn(|j|b.base_pose[j][3]), |v|v[i])
        }).collect();
        let get = |names:&[&str]| landmark(gla,names).map(|i|p[i]);
        let pelvis = get(&["pelvis", "hips"])?;
        let chest = get(&["thoracic", "spine2", "chest", "upperchest"])?;
        let neck = get(&["cervical", "neck"]).unwrap_or(chest);
        let head = get(&["cranium", "head"])?;
        let shoulders = [get(&["lhumerus", "leftupperarm", "lupperarm"])?,get(&["rhumerus", "rightupperarm", "rupperarm"])?];
        let elbows = [get(&["lradius", "leftforearm", "lforearm"])?,get(&["rradius", "rightforearm", "rforearm"])?];
        let wrists = [get(&["lhand", "lefthand"])?,get(&["rhand", "righthand"])?];
        let hips = [get(&["lfemuryz", "leftthigh", "lthigh"])?,get(&["rfemuryz", "rightthigh", "rthigh"])?];
        let knees = [get(&["ltibia", "leftcalf", "lcalf"])?,get(&["rtibia", "rightcalf", "rcalf"])?];
        let ankles = [get(&["ltalus", "leftfoot", "lfoot"])?,get(&["rtalus", "rightfoot", "rfoot"])?];
        let mesh_ground = glm.lods[0].surfaces.iter().flat_map(|s|s.vertices.iter())
            .map(|v|v.position[2]).filter(|z|z.is_finite()).fold(f32::INFINITY,f32::min);
        // Meshes can include dangling capes, ornaments or sheathed equipment;
        // do not let their lowest vertex redefine the character's foot plane.
        let ankle_z=ankles[0][2].min(ankles[1][2]);
        let leg_len=(pelvis[2]-ankle_z).max(1.0);
        let ground=mesh_ground.clamp(ankle_z-0.3*leg_len, ankle_z-0.05*leg_len);
        if !ground.is_finite() || p.iter().flatten().any(|x|!x.is_finite()) ||
            !(head[2]>chest[2] && chest[2]>pelvis[2] && pelvis[2]>ground+1.0) ||
            !(ankles[0][2]<pelvis[2] && ankles[1][2]<pelvis[2]) ||
            dist(shoulders[0],shoulders[1])<2.0 || dist(hips[0],hips[1])<2.0 ||
            dist(shoulders[0],elbows[0])<2.0 || dist(hips[0],knees[0])<2.0 { return None; }
        Some(Self { original:p, kinds:gla.skeleton.iter().map(|b|region(&b.name)).collect(),
            pelvis,chest,neck,head,shoulders,elbows,wrists,hips,knees,ankles,ground,params:params.clamped() })
    }
    fn joint_changes(&self) -> bool {
        let p=self.params;
        p.height!=0.5 || p.proportions!=0.5 || p.age!=0.5
    }
    fn scales(&self) -> [f32;4] { // leg, torso, head, arm
        let p=self.params;
        let height=(p.height-0.5)*2.0;
        let proportions=(p.proportions-0.5)*2.0;
        let child=((0.5-p.age)*2.0).max(0.0);
        let elder=((p.age-0.5)*2.0).max(0.0);
        [1.0+0.24*proportions+0.28*height-0.27*child-0.04*elder,
         1.0-0.19*proportions+0.23*height-0.16*child-0.08*elder,
         1.0+0.19*child+0.04*height-0.025*elder,
         1.0+0.10*proportions+0.20*height-0.20*child-0.035*elder]
    }
    fn map_z(&self,z:f32) -> f32 {
        let [leg,torso,head,_]=self.scales();
        let hip=self.pelvis[2];let base=self.ground;
        if z<=hip { base+(z-base)*leg }
        else if z<=self.neck[2] { base+(hip-base)*leg+(z-hip)*torso }
        else { base+(hip-base)*leg+(self.neck[2]-hip)*torso+(z-self.neck[2])*head }
    }
    fn core(&self,p:[f32;3]) -> [f32;3] {
        let prop=(self.params.proportions-0.5)*2.0;
        let child=((0.5-self.params.age)*2.0).max(0.0);
        let elder=((self.params.age-0.5)*2.0).max(0.0);
        let torso_t=smooth((p[2]-self.pelvis[2])/(self.chest[2]-self.pelvis[2]).max(1.0));
        let head_t=smooth((p[2]-self.neck[2])/(self.head[2]-self.neck[2]).max(1.0));
        let width=(1.0+prop*lerp(-0.07,0.13,torso_t)-0.14*child*(1.0-head_t)
            +0.11*child*head_t).clamp(0.65,1.5);
        let depth=(1.0+0.04*prop-0.07*child+0.11*child*head_t).clamp(0.7,1.5);
        // Elderly upper-back rounding is subtle, preserving the authored facing axis (-Y).
        let forward= -elder*0.045*(self.neck[2]-self.pelvis[2])*smooth((p[2]-self.chest[2])/(self.head[2]-self.chest[2]).max(1.0));
        [self.pelvis[0]+(p[0]-self.pelvis[0])*width,
         self.pelvis[1]+(p[1]-self.pelvis[1])*depth+forward,
         self.map_z(p[2])]
    }
    fn map(&self,p:[f32;3], r:Region) -> [f32;3] {
        match r {
            Region::LeftArm|Region::RightArm => {
                let i=usize::from(r==Region::RightArm);
                let origin=self.shoulders[i];
                let old=sub(p,origin);
                let new=self.core(origin);
                let scale=self.scales()[3];
                // Centerline deformation applies to the shoulder; whole limb
                // follows at the original angle with updated length.
                add(new,[old[0]*scale,old[1]*scale,old[2]*scale])
            }
            Region::LeftLeg|Region::RightLeg => {
                let i=usize::from(r==Region::RightLeg);
                let origin=self.hips[i];
                let base=self.core(p);
                // Preserve knees/feet alignment under proportion/height morph.
                let lateral=0.025*(self.params.proportions-0.5)*2.0;
                [base[0]+(p[0]-origin[0])*lateral,base[1],base[2]]
            }
            Region::Head => {
                let skull=self.head;
                let origin=self.core(skull);
                let s=self.scales()[2];
                let old=sub(p,skull);
                add(origin,[old[0]*s,old[1]*s,old[2]*s])
            }
            Region::Torso => self.core(p),
        }
    }
    fn tissue(&self,p:[f32;3],r:Region)->[f32;3] {
        let fat=(self.params.weight-0.5)*2.0;
        let muscle=(self.params.muscle-0.5)*2.0;
        if fat==0.0 && muscle==0.0 {return p;}
        match r {
            Region::Torso => {
                let d=sub(p,self.pelvis);
                let hip=bump(p[2],self.pelvis[2]+1.0,(self.chest[2]-self.pelvis[2])*0.48);
                let belly=bump(p[2],lerp(self.pelvis[2],self.chest[2],0.38),(self.chest[2]-self.pelvis[2])*0.48);
                let chest=bump(p[2],self.chest[2],(self.chest[2]-self.pelvis[2])*0.40);
                let axial=1.0+fat*(0.21*belly+0.11*hip+0.075*chest)+muscle*(0.055*belly+0.15*chest);
                let depth=1.0+fat*(0.27*belly+0.16*hip+0.09*chest)+muscle*(0.045*belly+0.10*chest);
                [self.pelvis[0]+d[0]*axial,self.pelvis[1]+d[1]*depth,p[2]]
            }
            Region::LeftArm|Region::RightArm|Region::LeftLeg|Region::RightLeg => {
                let i=usize::from(matches!(r,Region::RightArm|Region::RightLeg));
                let (a,b,c)=if matches!(r,Region::LeftArm|Region::RightArm) {
                    (self.shoulders[i],self.elbows[i],self.wrists[i])
                }else{(self.hips[i],self.knees[i],self.ankles[i])};
                let first=nearest_radial(p,a,b);let second=nearest_radial(p,b,c);
                let radial=if dot(first,first)<=dot(second,second) {first}else{second};
                let factor=if matches!(r,Region::LeftArm|Region::RightArm) {
                    0.19*muscle+0.075*fat
                }else{0.125*muscle+0.125*fat};
                // Avoid inflating joint tips and hard silhouette transitions.
                let along=dist(p,a)/dist(a,c).max(0.001);
                let soften=lerp(0.55,1.0,smooth(along*4.0)) * lerp(1.0,0.45,smooth((along-0.8)*5.0));
                add(p,mul(radial,(factor*soften).clamp(-0.32,0.32)))
            }
            Region::Head => p,
        }
    }
    fn vertex(&self,p:[f32;3],r:Region)->[f32;3] {self.map(self.tissue(p,r),r)}
}

/// All models with usable humanoid anatomy are eligible, regardless of paths or
/// whether a matching `.bodyshape` bundle was generated.
pub fn supports(glm:&GlmModel,gla:&GlaAnimation,native:Option<&[[f32;3]]>)->bool {
    Anatomy::build(glm,gla,native,BodyShapeParams::default()).is_some()
}

/// Deform every LOD once during model registration. The topology, UVs, weights,
/// surface indices, and animation files are unchanged. New joint pivots are
/// returned only for height/proportions/age, which require animation rebasing.
pub fn apply(glm:&mut GlmModel,gla:&GlaAnimation,params:BodyShapeParams,native:Option<&[[f32;3]]>)
    -> Result<Option<Vec<[f32;3]>>,String> {
    let Some(shape)=Anatomy::build(glm,gla,native,params) else {
        return Err("model has no recognizable humanoid bind anatomy".into());
    };
    if params.is_neutral(){return Ok(None);}
    for lod in &mut glm.lods {
        // Weld across material splits using the original *bit-exact* positions:
        // seam vertices may have distinct bone weights and must still meet.
        let mut weld=HashMap::<[u32;3],usize>::new();
        let mut before=Vec::<[f32;3]>::new();
        let mut sum=Vec::<[f32;3]>::new();
        let mut count=Vec::<u32>::new();
        let mut ids=Vec::with_capacity(lod.surfaces.len());
        for surf in &lod.surfaces {
            let mut surface_ids=Vec::with_capacity(surf.vertices.len());
            for vertex in &surf.vertices {
                let key=vertex.position.map(|n|if n==0.0{0}else{n.to_bits()});
                let id=*weld.entry(key).or_insert_with(|| {
                    let id=before.len();before.push(vertex.position);sum.push([0.;3]);count.push(0);id
                });
                let mut changed=[0.;3]; let mut total=0.0;
                for w in &vertex.weights {
                    if let Some(&bone)=surf.bone_references.get(w.local_bone_index) {
                        if let Some(&kind)=shape.kinds.get(bone) {
                            if w.weight>0.0 && w.weight.is_finite() {
                                let mapped=shape.vertex(vertex.position,kind);
                                for k in 0..3 {changed[k]+=w.weight*(mapped[k]-vertex.position[k]);}
                                total+=w.weight;
                            }
                        }
                    }
                }
                if total>1e-5 { for k in 0..3 { sum[id][k]+=changed[k]/total; } }
                else {let mapped=shape.vertex(vertex.position,Region::Torso);
                    for k in 0..3 {sum[id][k]+=mapped[k]-vertex.position[k];} }
                count[id]+=1;
                surface_ids.push(id);
            }
            ids.push(surface_ids);
        }
        for (s,groups) in lod.surfaces.iter_mut().zip(&ids) {
            for (v,&id) in s.vertices.iter_mut().zip(groups) {
                let inv=1.0/count[id] as f32;
                for k in 0..3 {v.position[k]+=sum[id][k]*inv;}
                if !v.position.iter().all(|x|x.is_finite()) {return Err("nonfinite procedural shape vertex".into());}
            }
        }
        // Reconstruct geometric normal deltas, but keep original hard edges and
        // authored normal detail. Seam positions use shared accumulated normals.
        let mut old_faces=vec![[0.;3];before.len()];
        let mut new_faces=vec![[0.;3];before.len()];
        for (surf,groups) in lod.surfaces.iter().zip(&ids) {
            for face in &surf.triangles {
                let [a,b,c]=face.map(|i|i as usize);
                if [a,b,c].iter().any(|&v|v>=groups.len()) {return Err("invalid procedural shape triangle".into());}
                let old=cross(sub(before[groups[b]],before[groups[a]]),sub(before[groups[c]],before[groups[a]]));
                let new=cross(sub(surf.vertices[b].position,surf.vertices[a].position),sub(surf.vertices[c].position,surf.vertices[a].position));
                for i in [a,b,c] { for k in 0..3 {old_faces[groups[i]][k]+=old[k];new_faces[groups[i]][k]+=new[k];} }
            }
        }
        for (surf,groups) in lod.surfaces.iter_mut().zip(ids) {
            for (v,id) in surf.vertices.iter_mut().zip(groups) {
                let old=normalize(old_faces[id]);let new=normalize(new_faces[id]);
                if dot(old,old)>0.0 && dot(new,new)>0.0 {
                    let sign=if dot(old,v.normal)<0.0 {-1.0}else{1.0};
                    v.normal=normalize(std::array::from_fn(|k|v.normal[k]+sign*(new[k]-old[k])));
                }
            }
        }
    }
    if shape.joint_changes() {
        Ok(Some(shape.original.iter().zip(&shape.kinds).enumerate().map(|(i,(&p,&r))| {
            // Entity origin and its root-space attachment frame must never
            // drift merely because the character became shorter/taller.
            let name=bone_name(&gla.skeleton[i].name);
            if name=="modelroot" || name=="root" {p} else {shape.map(p,r)}
        }).collect()))
    }else{Ok(None)}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ghoul2::{GlmLod, GlmSurface, GlmVertex, GlmWeight};
    use crate::skeleton::{humanoid_reference,HumanoidRig};
    fn model() -> GlmModel {
        let gla=humanoid_reference(HumanoidRig::Jka53);
        let vertices=[[-2.0,0.0,45.0],[2.0,0.0,45.0],[2.0,0.0,48.0],[-2.0,0.0,48.0]];
        let surface=GlmSurface { surface_index:0,
            vertices:vertices.iter().map(|&p|GlmVertex{position:p,normal:[0.,-1.,0.],weights:vec![GlmWeight{local_bone_index:0,weight:1.}]}).collect(),
            texcoords:vec![[0.,0.];4],triangles:vec![[0,1,2],[0,2,3]],bone_references:vec![13] };
        let ground=GlmSurface{surface_index:2,
            vertices:vec![GlmVertex{position:[0.,0.,0.],normal:[0.,0.,1.],weights:vec![GlmWeight{local_bone_index:0,weight:1.}]}],
            texcoords:vec![[0.,0.]],triangles:vec![],bone_references:vec![6]};
        GlmModel{name:"custom-any-model".into(),anim_name:"players/_humanoid/_humanoid".into(),num_bones:53,
            hierarchy:Vec::new(),lods:vec![GlmLod{surfaces:vec![surface.clone(),GlmSurface{surface_index:1,..surface},ground]}]}
    }
    #[test]
    fn automatic_detection_and_exact_neutral() {
        let gla=humanoid_reference(HumanoidRig::Jka53);
        let mut glm=model();let original=glm.clone();
        assert!(supports(&glm,&gla,None));
        assert!(apply(&mut glm,&gla,BodyShapeParams::default(),None).unwrap().is_none());
        assert_eq!(glm.lods[0].surfaces[0].vertices[0].position,original.lods[0].surfaces[0].vertices[0].position);
    }
    #[test]
    fn tissue_keeps_joint_pivots_and_seams_welded() {
        let gla=humanoid_reference(HumanoidRig::Jka53);let mut glm=model();
        let p=BodyShapeParams{weight:1.0,muscle:1.0,..Default::default()};
        assert!(apply(&mut glm,&gla,p,None).unwrap().is_none());
        for (a,b) in glm.lods[0].surfaces[0].vertices.iter().zip(&glm.lods[0].surfaces[1].vertices) {
            assert_eq!(a.position,b.position);
            assert!(a.normal.iter().all(|x|x.is_finite()));
        }
    }
    #[test]
    fn height_and_age_retarget_joint_links() {
        let gla=humanoid_reference(HumanoidRig::Jka53);let mut glm=model();
        let pivots=apply(&mut glm,&gla,BodyShapeParams{height:1.0,age:0.0,..Default::default()},None).unwrap().unwrap();
        assert_eq!(pivots.len(),53);
        let old=gla.skeleton[13].base_pose[2][3]-gla.skeleton[1].base_pose[2][3];
        assert!((pivots[13][2]-pivots[1][2]-old).abs()>0.05);
        assert!(pivots.iter().flatten().all(|x|x.is_finite()));
    }    #[test]
    fn jk2_and_converted_source_native_pivots_are_eligible() {
        let gla=humanoid_reference(HumanoidRig::Jk2_72);
        let mut glm=model();
        glm.num_bones=72;
        let original:Vec<_>=gla.skeleton.iter().map(|b|b.base_pose.map(|row|row[3])).collect();
        assert!(supports(&glm,&gla,None));
        assert!(supports(&glm,&gla,Some(&original)));
        let pivots=apply(&mut glm,&gla,
            BodyShapeParams{proportions:0.9,..Default::default()},Some(&original)).unwrap().unwrap();
        assert_eq!(pivots.len(),72);
        assert_eq!(pivots[0],original[0]);
        assert!(pivots.iter().flatten().all(|x|x.is_finite()));
    }
    #[test]
    fn malformed_or_non_humanoid_rigs_are_not_shaped() {
        let gla=humanoid_reference(HumanoidRig::Jka53);
        let mut glm=model();glm.num_bones=1;
        assert!(!supports(&glm,&gla,None));
        assert!(apply(&mut glm,&gla,BodyShapeParams::default(),None).is_err());
    }
    #[test]
    fn shape_extremes_preserve_faces_weights_and_uvs() {
        let gla=humanoid_reference(HumanoidRig::Jka53);
        let original=model();
        for p in [
            BodyShapeParams{muscle:0.,weight:0.,height:0.,age:0.,proportions:0.},
            BodyShapeParams{muscle:1.,weight:1.,height:1.,age:1.,proportions:1.},
        ] {
            let mut glm=original.clone();
            let joints=apply(&mut glm,&gla,p,None).unwrap().unwrap();
            assert_eq!(joints.len(),gla.skeleton.len());
            for (old,new) in original.lods[0].surfaces.iter().zip(&glm.lods[0].surfaces) {
                assert_eq!(old.triangles,new.triangles);
                assert_eq!(old.texcoords,new.texcoords);
                assert_eq!(old.bone_references,new.bone_references);
                for (v,w) in old.vertices.iter().zip(&new.vertices) {
                    assert_eq!(v.weights,w.weights);
                    assert!(w.position.iter().chain(w.normal.iter()).all(|x|x.is_finite()));
                }
            }
        }
    }

}
