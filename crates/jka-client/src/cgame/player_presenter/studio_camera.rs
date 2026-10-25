//! Pose-only attachment queries. These share the preview's animation evaluator,
//! without skinning meshes, emitting FX or advancing simulation history.
use crate::cgame::ClientInfo;
use super::{PlayerPresenter, Ghoul2Animator, model_bolt_matrix_timed,
    saber_hand_bolt_matrix_timed, saber_name_is_removed, transform_jka_model_point,
    transform_jka_model_vector, normalize_vec3};

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProfileAttachmentFrame {
    /// Renderer coordinates, matching Camera rather than the Z-up asset space.
    pub position: [f32;3],
    pub axis: [[f32;3];3],
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires JKA_TEST_BASE stock assets; DJK_EMOTE_TEST_BASE optionally tests a packed emote"]
    fn studio_camera_attachments_match_rendered_player_and_blades() {
        use crate::cgame::player_presenter::PlayerFxRequest;
        let base=std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("JKA_TEST_BASE"));
        let assets=jka_assets::pk3::AssetSearchPath::open(&base).unwrap();
        let mut presenter=PlayerPresenter::new(assets,false).unwrap();
        presenter.set_async_loading(false);
        presenter.set_ghoul2_anim_smooth(0.);
        presenter.set_skinning_mode(crate::ui::Ghoul2SkinningMode::Cpu);
        let mut emotes=vec![None];
        if let Some(path)=std::env::var_os("DJK_EMOTE_TEST_BASE") {
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(30);
            let catalog=loop {
                if let Ok(catalog)=crate::emotes::catalog(std::path::Path::new(&path)) {break catalog;}
                assert!(std::time::Instant::now()<deadline,"emote catalog timeout");
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let root=catalog.iter().find(|e|e.id=="EID_DanceMoves").expect("DanceMoves bundle");
            let variant=root.variants.get(crate::emotes::default_variant_index(root)).unwrap_or(root).clone();
            emotes.push(Some(crate::emotes::prepare_visuals(variant).unwrap().0));
        }
        let origin=[88.,5.,-18.];let (s,c)=37_f32.to_radians().sin_cos();
        let axis=[[c,s,0.],[-s,c,0.],[0.,0.,1.]];
        let near=|a:[f32;3],b:[f32;3]|assert!((glam::Vec3::from_array(a)-glam::Vec3::from_array(b)).length()<0.002,"{a:?} != {b:?}");
        for model in ["kyle/default","jan/default"] {
            let mut info=ClientInfo::solo_model(model);
            info.saber_name="single_1".into();info.saber2_name="single_2".into();
            let names=presenter.profile_attachment_names(&info,None).unwrap();
            assert!(names.iter().any(|n|n=="cranium"));assert!(names.iter().any(|n|n=="*r_hand"));
            for emote in &emotes {
                let animation=if emote.is_some(){"External test clip"}else{"BOTH_WALK_DUAL"};
                for time in [125,750,1800,125] {
                    let preview=emote.clone().map(|emote|crate::emotes::Preview {emote,elapsed_ms:time});
                    let range=emote.as_ref().map(|e|(e.first_frame+2,e.end_frame-1));
                    let (draws,anchors)=presenter.present_profile_player_preview(&info,origin,axis,time,animation,
                        true,[1.;3],0,[0.2,-0.1],true,preview.clone(),range).unwrap();
                    assert!(!draws.is_empty());
                    let requests=presenter.drain_fx_requests();
                    for (anchor,bone) in [(0,"*head_top"),(3,"*r_hand"),(4,"*l_hand")] {
                        let frame=presenter.sample_profile_attachment(&info,origin,axis,time,animation,[0.2,-0.1],
                            preview.as_ref(),range,None,bone).unwrap().unwrap();
                        near(frame.position,crate::scene::render_position(anchors[anchor].unwrap()));
                    }
                    let mut blades=0;
                    for request in requests {
                        if let PlayerFxRequest::SaberBlade {origin:base,direction,length,saber_num,blade_num,..}=request {
                            let tag=format!("*blade{}",blade_num+1);let tip=format!("@tip{}",blade_num+1);
                            for (name,expected) in [(tag,base),(tip,std::array::from_fn(|i|base[i]+direction[i]*length))] {
                                let frame=presenter.sample_profile_attachment(&info,origin,axis,time,animation,[0.2,-0.1],
                                    preview.as_ref(),range,Some(saber_num as usize),&name).unwrap().unwrap();
                                near(frame.position,crate::scene::render_position(expected));
                            }
                            blades+=1;
                        }
                    }
                    assert!(blades>=2,"both saber slots must be tested");
                    assert!(presenter.drain_fx_requests().is_empty(),"pose queries must not emit FX");
                    assert_eq!(presenter.stage_time_ms,time,"queries must not advance simulation");
                    assert!(presenter.sample_profile_attachment(&info,origin,axis,time,animation,[0.;2],
                        preview.as_ref(),range,None,"missing_camera_bone").unwrap().is_none());
                }
            }
        }
    }
}
impl ProfileAttachmentFrame {
    pub fn point(&self, offset: [f32;3], local: bool) -> [f32;3] {
        std::array::from_fn(|i| self.position[i] + if local {
            (0..3).map(|j|self.axis[j][i]*offset[j]).sum::<f32>()
        } else {offset[i]})
    }
    pub fn offset(&self, point: [f32;3], local: bool) -> [f32;3] {
        let delta=std::array::from_fn::<_,3,_>(|i|point[i]-self.position[i]);
        if local {
            let matrix=glam::Mat3::from_cols_array_2d(&self.axis);
            if matrix.determinant().abs()>1e-8 {(matrix.inverse()*glam::Vec3::from_array(delta)).to_array()}
            else {[0.;3]}
        } else {delta}
    }
}

impl PlayerPresenter {
    pub(crate) fn profile_attachment_model_id(&mut self,info:&ClientInfo)->Result<usize,String> {
        Ok(std::sync::Arc::as_ptr(&self.load_model(info)?) as usize)
    }
    pub(crate) fn profile_attachment_names(&mut self, info: &ClientInfo, saber: Option<usize>)
        -> Result<Vec<String>,String> {
        let mut names = if let Some(slot)=saber {
            let name=if slot==0 {&info.saber_name}else{&info.saber2_name};
            if name.is_empty() || saber_name_is_removed(name) {return Err("No saber equipped in this slot".into());}
            let definition=self.saber_definitions.definition_or_default(name);
            let model=self.load_saber_model(&definition)?;
            let mut names=vec!["@hilt".into()];
            names.extend((1..=definition.num_blades).map(|i|format!("@tip{i}")));
            names.extend(model.gla.skeleton.iter().map(|b|b.name.clone()));
            names.extend(model.glm.hierarchy.iter().filter(|s|s.name.starts_with('*')).map(|s|s.name.clone()));
            names
        } else {
            let model=self.load_model(info)?;
            let mut names=vec!["@origin".into()];
            names.extend(model.gla.skeleton.iter().map(|b|b.name.clone()));
            names.extend(model.glm.hierarchy.iter().filter(|s|s.name.starts_with('*')).map(|s|s.name.clone()));
            names
        };
        names.sort_by_key(|s|s.to_ascii_lowercase());names.dedup();Ok(names)
    }

    pub(crate) fn sample_profile_attachment(&mut self, info: &ClientInfo,
        origin: [f32;3], axis: [[f32;3];3], time_ms: i32, animation: &str,
        head_look: [f32;2], emote: Option<&crate::emotes::Preview>, range: Option<(i32,i32)>,
        saber: Option<usize>, name: &str,
    ) -> Result<Option<ProfileAttachmentFrame>,String> {
        let convert=|matrix: jka_assets::ghoul2::Matrix3x4| ProfileAttachmentFrame {
            position: crate::scene::render_position(transform_jka_model_point(
                [matrix[0][3],matrix[1][3],matrix[2][3]],axis,origin)),
            axis: std::array::from_fn(|i|crate::scene::render_position(normalize_vec3(
                transform_jka_model_vector([matrix[0][i],matrix[1][i],matrix[2][i]],axis)))),
        };
        if saber.is_none() && name=="@origin" {
            return Ok(Some(convert([[1.,0.,0.,0.],[0.,1.,0.,0.],[0.,0.,1.,0.]])));
        }
        let model=self.load_model(info)?;
        let pose=self.profile_preview_pose(&model,origin,axis,time_ms,animation,head_look,emote,range,false)?;
        let Some(slot)=saber else {
            return Ok(model_bolt_matrix_timed(&mut self.perf,&model.glm,&model.gla,&pose,name)?.map(convert));
        };
        let saber_name=if slot==0 {&info.saber_name}else{&info.saber2_name};
        if saber_name.is_empty() || saber_name_is_removed(saber_name) {return Ok(None);}
        let definition=self.saber_definitions.definition_or_default(saber_name);
        let Some(hand)=saber_hand_bolt_matrix_timed(&mut self.perf,&model,&pose,
            if slot==0 {"*r_hand"}else{"*l_hand"})? else {return Ok(None)};
        if name=="@hilt" {return Ok(Some(convert(hand)));}
        let hilt=self.load_saber_model(&definition)?;
        let mut animator=Ghoul2Animator::new(&hilt.animation_gla);
        let hilt_pose=hilt.bind_animation_pose(animator.evaluate_pose(&hilt.animation_gla,time_ms,hand)?)?;
        let tip=name.strip_prefix("@tip").and_then(|i|i.parse::<usize>().ok())
            .filter(|&i|i>0 && i<=definition.num_blades).map(|i|i-1);
        let blade_index=tip.or_else(||name.strip_prefix("*blade").and_then(|i|i.parse::<usize>().ok())
            .filter(|&i|i>0 && i<=definition.num_blades).map(|i|i-1));
        let tag=tip.map_or_else(||name.to_owned(),|i|format!("*blade{}",i+1));
        let mut matrix=model_bolt_matrix_timed(&mut self.perf,&hilt.glm,&hilt.gla,&hilt_pose,&tag)?;
        if matrix.is_none() && blade_index.is_some() {
            matrix=model_bolt_matrix_timed(&mut self.perf,&hilt.glm,&hilt.gla,&hilt_pose,"*flash")?;
            // Same legacy staff attachment fallback as the visible saber.
            if blade_index==Some(1) && definition.saber_type.eq_ignore_ascii_case("SABER_STAFF") {
                if let Some(m)=matrix.as_mut() {for row in m {row[0] = -row[0];row[3] -= row[0]*16.;}}
            }
        }
        if let (Some(m),Some(i))=(matrix.as_mut(),tip) {
            let direction=normalize_vec3([-m[0][0],-m[1][0],-m[2][0]]);
            let length=definition.blade(i).length;
            for a in 0..3 {m[a][3]+=direction[a]*length;}
        }
        Ok(matrix.map(convert))
    }
}
