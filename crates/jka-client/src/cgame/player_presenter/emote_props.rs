//! Emote props follow the final player mesh pose, including model retargeting
//! and ground/contact correction. No protocol entities or server animation IDs.
use super::{PlayerPresenter,PlayerModelAsset,CosmeticDraw,Matrix3x4,transform_jka_model_point,transform_jka_model_vector};
impl PlayerPresenter {
    pub(super) fn emote_prop_draws(active:&crate::emotes::Preview,model:&PlayerModelAsset,pose:&[Matrix3x4],axis:[[f32;3];3],origin:[f32;3],entity_num:u16)->Vec<CosmeticDraw> {
        let mut draws=Vec::new();
        let emote=&active.emote;
        let frames=(emote.end_frame-emote.first_frame).max(1) as usize;
        let loop_start=emote.loop_start_frame.map(|n|(n-emote.first_frame).max(0) as usize).or(emote.looping.then_some(0));
        let seconds=jka_emotes::presentation::clock(active.elapsed_ms,emote.fps,frames,loop_start);
        for prop in &emote.props {
            if !prop.intervals.iter().any(|span|seconds>=span[0]&&seconds<span[1]) {continue;}
            let Some(index)=model.gla.skeleton.iter().position(|b|b.name.eq_ignore_ascii_case(&prop.bone)) else{continue};
            let Some(skin)=pose.get(index) else{continue};
            let Some(mut relative)=prop.sample_elapsed_interpolated(active.elapsed_ms,emote.fps,loop_start,crate::emotes::interpolation()) else{continue};
            let bind=&model.gla.skeleton[index].base_pose;
            // The track translation is a rest-space offset from the source
            // anchor. Rebase that offset onto THIS model's bone bind position.
            for r in 0..3{relative[r][3]+=bind[r][3];}
            let placed=jka_assets::ghoul2::multiply_3x4(skin,&relative);
            let p=[placed[0][3],placed[1][3],placed[2][3]];
            let prop_origin=transform_jka_model_point(p,axis,origin);
            let prop_axis=std::array::from_fn(|c|transform_jka_model_vector([placed[0][c]*prop.scale,placed[1][c]*prop.scale,placed[2][c]*prop.scale],axis));
            if !prop_origin.iter().chain(prop_axis.iter().flatten()).all(|n|n.is_finite()){continue;}
            let skin_pose=prop.skin_pose.as_ref().map(|clip|std::sync::Arc::new(clip.sample_unrooted_interpolated(active.elapsed_ms,emote.fps,loop_start,crate::emotes::interpolation())));
            if prop.skin_track.is_some()&&skin_pose.is_none(){continue;}
            draws.push(CosmeticDraw{entity_num,model:prop.model.clone(),origin:prop_origin,axis:prop_axis,rgba:[1.;4],custom_shader:None,skin_pose,face_camera:None});
        }
        draws
    }
}
