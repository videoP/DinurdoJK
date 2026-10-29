//! Separate single-player animation banks. Catalog scans read CFGs and GLA
//! headers only; full poses use the presenter's existing lazy GLA/rig caches.
use super::{App,Instant};
use jka_assets::{animation::{parse_animation_cfg,AnimationSet},pk3::AssetSearchPath};
use serde::{Serialize,Deserialize};
use std::sync::Arc;

const PREFIX:&str="studio-cinematic:";
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub(in crate::app) struct StudioCinematicSource {pub gla:String,pub animation:String}
impl StudioCinematicSource {
    pub fn id(&self)->String {format!("{PREFIX}{}#{}",self.gla.to_ascii_lowercase(),self.animation.to_ascii_uppercase())}
    pub fn from_emote(emote:&crate::emotes::Emote)->Option<Self> {
        if !emote.id.starts_with(PREFIX) {return None;}
        let source=Self {gla:emote.gla.clone(),animation:emote.variant_name.clone()};
        (emote.id==source.id()).then_some(source)
    }
    fn config(&self)->Result<String,String> {
        let path=jka_emotes::manifest::qpath(&self.gla)?.to_ascii_lowercase();
        let (folder,file)=path.rsplit_once('/').ok_or("Missing cinematic source folder")?;
        let set=folder.strip_prefix("models/players/_humanoid_").ok_or("Not a separate humanoid cinematic bank")?;
        if set.is_empty() || set.contains('/') || file!=format!("_humanoid_{set}.gla")
            || !self.animation.starts_with("BOTH_CIN_") {return Err("Invalid cinematic source identity".into());}
        Ok(format!("{folder}/animation.cfg"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::render_studio_timeline::{StudioAnimationClip,StudioClipPlayback,StudioProject};
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new()->Self {
            let root=std::env::temp_dir().join(format!("dinurdo-cinematic-test-{}-{}",std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            std::fs::create_dir(&root).unwrap();Self(root)
        }
        fn write(&self,path:&str,bytes:&[u8]) {
            let path=self.0.join(path);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(path,bytes).unwrap();
        }
        fn bank(&self,name:&str,cfg:&str,frames:i32) {
            self.write(&format!("models/players/_humanoid_{name}/animation.cfg"),cfg.as_bytes());
            let mut header=[0_u8;100];header[..4].copy_from_slice(b"2LGA");header[4..8].copy_from_slice(&6_i32.to_le_bytes());
            header[76..80].copy_from_slice(&frames.to_le_bytes());header[84..88].copy_from_slice(&53_i32.to_le_bytes());
            self.write(&format!("models/players/_humanoid_{name}/_humanoid_{name}.gla"),&header);
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let root=self.0.canonicalize().unwrap();let temp=std::env::temp_dir().canonicalize().unwrap();
            assert_eq!(root.parent(),Some(temp.as_path()));assert!(root.file_name().unwrap().to_string_lossy().starts_with("dinurdo-cinematic-test-"));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    fn timeline_clip(clip:&StudioCinematicClip)->StudioAnimationClip {
        StudioAnimationClip {name:clip.label.clone(),emote:Some(clip.emote().unwrap()),start:2.,first_frame:clip.first_frame,
            end_frame:clip.end_frame(),source_fps:f64::from(clip.fps),lane:0,source_first:clip.first_frame,
            source_end:clip.end_frame(),motion:vec![Default::default()],tail_frames:0,playback:StudioClipPlayback::Hold,speed_keys:vec![], blend:Default::default()}
    }
    #[test]
    fn cinematic_catalog_discovers_separate_banks_preserves_fps_and_rejects_empty_or_invalid_ranges() {
        let fixture=Fixture::new();
        fixture.bank("yavin1","BOTH_CIN_1 3 4 0 29\nBOTH_CIN_2 9 2 -1 20\nBOTH_CIN_3 0 0 -1 20\nBOTH_CIN_4 0 1 -1 20",10);
        fixture.bank("yavin2","BOTH_CIN_1 5 2 -1 20",10);
        fixture.write("models/players/_humanoid/animation.cfg",b"BOTH_CIN_1 0 8 -1 20");
        fixture.write("models/players/_humanoid_missing/animation.cfg",b"BOTH_CIN_1 0 8 -1 20");
        let mut assets=AssetSearchPath::open(&fixture.0).unwrap();let catalog=scan(&mut assets).unwrap();
        assert_eq!(catalog.clips.len(),3);assert_eq!(catalog.diagnostics.len(),1);
        let first=catalog.clips.iter().find(|c|c.set=="yavin1" && c.source.animation=="BOTH_CIN_1").unwrap();
        let second=catalog.clips.iter().find(|c|c.set=="yavin2").unwrap();
        assert_eq!(first.label,"Yavin 1 / CIN 1");assert_eq!(first.fps,29.);assert_eq!(first.first_frame,3);
        assert_eq!(first.frame_count,4);assert_eq!(first.loop_frames,0);
        assert_ne!(first.source.id(),second.source.id());
        assert_eq!(resolve(&mut assets,&first.source).unwrap().source,first.source);
        assert_eq!(StudioCinematicSource::from_emote(&first.emote().unwrap()).unwrap(),first.source);
        assert!(source_for_config("models/players/_humanoid/animation.cfg").is_none());
        assert!(StudioCinematicSource {gla:"../x.gla".into(),animation:"BOTH_CIN_1".into()}.config().is_err());
        assert_eq!(set_label("t1_danger"),"T1 Danger");
    }
    #[test]
    fn cinematic_timeline_trim_hold_repeat_and_source_identity_survive_project_serialization() {
        let fixture=Fixture::new();fixture.bank("yavin1","BOTH_CIN_11 100 200 0 20",400);
        let mut assets=AssetSearchPath::open(&fixture.0).unwrap();let source=scan(&mut assets).unwrap().clips.remove(0);
        let mut clip=timeline_clip(&source);clip.first_frame=120;clip.end_frame=200;clip.tail_frames=80;
        assert_eq!(clip.pose_time_ms(2.),1000); // trim begins 20 source frames into this animation
        let held=clip.pose_time_ms(clip.end()-0.1);clip.playback=StudioClipPlayback::Loop;
        assert_eq!(clip.pose_time_ms(6.25),clip.pose_time_ms(2.25));
        assert!(held>clip.pose_time_ms(2.25));
        assert!(!clip.preview(clip.end()-0.1).unwrap().emote.looping,"Studio owns the repeat/hold clock");
        let project:StudioProject=serde_json::from_value(serde_json::json!({
            "version":5,"model":"kyle/default","with_sabers":false,"background":"black","background_map":"",
            "camera":[],"clips":[{"name":clip.name,"emote_id":null,"cinematic":source.source,"start":2.,
                "first_frame":clip.first_frame,"end_frame":clip.end_frame,"source_fps":clip.source_fps}]
        })).unwrap();
        let saved=serde_json::to_string(&project).unwrap();let restored:StudioProject=serde_json::from_str(&saved).unwrap();
        let source=restored.clips[0].cinematic.as_ref().unwrap();
        assert_eq!(resolve(&mut assets,source).unwrap().source.id(),clip.emote.as_ref().unwrap().id);
        assert!(restored.clips[0].emote_id.is_none());
        let old:StudioProject=serde_json::from_value(serde_json::json!({"version":4,"model":"kyle/default","with_sabers":false,
            "background":"black","background_map":"","camera":[],"clips":[{"name":"BOTH_STAND1","emote_id":null,
                "start":0.,"first_frame":0,"end_frame":10,"source_fps":20.}]})).unwrap();
        assert!(old.clips[0].cinematic.is_none());
    }
    #[test]
    fn cinematic_source_preview_can_pause_on_last_frame_without_modifying_the_timeline() {
        let fixture=Fixture::new();fixture.bank("yavin1","BOTH_CIN_11 100 20 0 20",400);
        let clip=scan(&mut AssetSearchPath::open(&fixture.0).unwrap()).unwrap().clips.remove(0);
        let mut studio=super::super::render_studio::RenderStudio::default();let before=studio.edit_snapshot().signature();
        studio.cinematic_preview=Some(StudioCinematicPreview {emote:clip.emote().unwrap(),clip,playing:false,time:1.,
            started:Instant::now(),lane:0,camera:Default::default(),pivot:[88.,8.,0.]});
        let (_,preview,range,time)=studio.active_animation();assert_eq!(time,1000);assert_eq!(range,Some((100,120)));
        assert!(!preview.unwrap().emote.looping);assert_eq!(studio.edit_snapshot().signature(),before);
        let preview=studio.cinematic_preview.as_mut().unwrap();preview.toggle();assert!(preview.elapsed()<0.1);
        preview.seek(0.45);assert!(!preview.playing);assert_eq!(preview.elapsed(),0.45);
    }
    #[test]
    #[ignore="requires JKA_TEST_BASE with the stock single-player PK3 assets"]
    fn cinematic_installed_banks_validate_and_render_short_and_long_clips() {
        use crate::cgame::{ClientInfo,player_presenter::PlayerPresenter};
        let base=std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("JKA_TEST_BASE"));
        let mut assets=AssetSearchPath::open(&base).unwrap();let catalog=scan(&mut assets).unwrap();
        assert!(catalog.diagnostics.is_empty(),"{:?}",catalog.diagnostics);
        let sets:std::collections::BTreeSet<_>=catalog.clips.iter().map(|c|c.source.gla.as_str()).collect();
        assert_eq!(sets.len(),32);assert_eq!(catalog.clips.len(),377);
        let standard=jka_assets::ghoul2::parse_gla(&assets.read("models/players/_humanoid/_humanoid.gla",128*1024*1024).unwrap().unwrap().bytes).unwrap();
        for path in sets {
            let gla=jka_assets::ghoul2::parse_gla(&assets.read(path,128*1024*1024).unwrap().unwrap().bytes).unwrap();
            assert_eq!(gla.skeleton.len(),53);
            assert_eq!(gla.skeleton.iter().map(|b|(&b.name,b.parent)).collect::<Vec<_>>(),standard.skeleton.iter().map(|b|(&b.name,b.parent)).collect::<Vec<_>>(),"{path}");
        }
        let mut presenter=PlayerPresenter::new(assets,false).unwrap();presenter.set_async_loading(false);
        presenter.set_ghoul2_anim_smooth(0.);presenter.set_skinning_mode(crate::ui::Ghoul2SkinningMode::Cpu);
        let selections=[("yavin1","BOTH_CIN_11"),("kor2","BOTH_CIN_26"),("t1_rail","BOTH_CIN_1"),
            ("taspir2","BOTH_CIN_1"),("hoth3","BOTH_CIN_1")];
        for (set,name) in selections {
            let source=catalog.clips.iter().find(|c|c.set==set && c.source.animation==name).unwrap();
            for model in ["kyle/default","jan/default"] {
                let info=ClientInfo::solo_model(model);let mut clip=timeline_clip(source);
                if clip.end_frame-clip.first_frame>10 {clip.first_frame+=2;clip.end_frame-=2;}
                for local in [0.,clip.source_duration()*0.5,(clip.source_duration()-0.001).max(0.)] {
                    let time=clip.start+local;let ms=clip.pose_time_ms(time);
                    let (draws,anchors)=presenter.present_profile_player_preview(&info,[88.,0.,-22.],
                        [[1.,0.,0.],[0.,1.,0.],[0.,0.,1.]],ms,&source.source.animation,false,[1.;3],0,[0.;2],true,
                        clip.preview(time),Some((clip.first_frame,clip.end_frame))).unwrap();
                    assert!(!draws.is_empty(),"{set}/{name}/{model}");assert!(anchors[0].is_some());
                    assert!(draws.iter().flat_map(|s|s.vertices.iter()).flat_map(|v|v.position).all(f32::is_finite));
                }
            }
            println!("Validated {}: {} frames at {} FPS",source.label,source.frame_count,source.fps);
        }
    }
}
#[derive(Clone,Debug)]
pub(in crate::app) struct StudioCinematicClip {
    pub source:StudioCinematicSource,pub set:String,pub set_label:String,pub label:String,
    pub first_frame:i32,pub frame_count:i32,pub fps:f32,pub loop_frames:i32,
}
impl StudioCinematicClip {
    pub fn end_frame(&self)->i32 {self.first_frame+self.frame_count}
    pub fn duration(&self)->f64 {f64::from(self.frame_count)/f64::from(self.fps)}
    pub fn emote(&self)->Result<crate::emotes::Emote,String> {
        let mut entries=jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[{
            "id":self.source.id(),"name":self.label,"gla":self.source.gla,
            "first_frame":self.first_frame,"end_frame":self.end_frame(),"fps":self.fps,
            "loop":self.loop_frames!=-1,"variant":self.source.animation,
            "loop_start_frame":if self.loop_frames>0 {Some(self.end_frame()-self.loop_frames)}else{None}
        }]}))?;
        let mut emote=entries.remove(0);
        emote.variant_name=self.source.animation.clone();Ok(emote)
    }
}
#[derive(Clone,Debug,Default)]
pub(in crate::app) struct StudioCinematicCatalog {pub clips:Vec<StudioCinematicClip>,pub diagnostics:Vec<String>}
#[derive(Default)]
pub(in crate::app) struct StudioCinematicLibrary {
    pub key:Option<(u64,bool)>,pub token:u64,pub pending:bool,
    pub catalog:Option<Result<Arc<StudioCinematicCatalog>,String>>,pub search:String,
}
pub(in crate::app) struct StudioCinematicPreview {
    pub clip:StudioCinematicClip,pub emote:crate::emotes::Emote,pub playing:bool,
    pub time:f64,pub started:Instant,pub lane:usize,
    pub camera:super::render_studio_timeline::StudioCameraKey,pub pivot:[f32;3],
}
impl StudioCinematicPreview {
    pub fn elapsed(&self)->f64 {
        if !self.playing {return self.time.clamp(0.,self.clip.duration());}
        let t=self.time+if self.playing {self.started.elapsed().as_secs_f64()}else{0.};
        if self.emote.looping {t.rem_euclid(self.clip.duration().max(0.001))}else{t.min(self.clip.duration())}
    }
    pub fn seek(&mut self,time:f64) {self.time=time.clamp(0.,self.clip.duration());self.started=Instant::now();self.playing=false;}
    pub fn toggle(&mut self) {
        let elapsed=self.elapsed();self.time=if elapsed>=self.clip.duration(){0.}else{elapsed};
        self.started=Instant::now();self.playing=!self.playing;
    }
}

fn source_for_config(config:&str)->Option<StudioCinematicSource> {
    let folder=config.strip_suffix("/animation.cfg")?;
    let set=folder.strip_prefix("models/players/_humanoid_")?;
    if set.is_empty() || set.contains('/') {return None;}
    Some(StudioCinematicSource {gla:format!("{folder}/_humanoid_{set}.gla"),animation:String::new()})
}
fn set_label(set:&str)->String {
    set.split('_').map(|word| {
        if word.len()==2 && word.starts_with('t') && word.as_bytes()[1].is_ascii_digit() {return word.to_ascii_uppercase();}
        let mut out=String::new();let mut previous=false;
        for (i,c) in word.chars().enumerate() {
            if c.is_ascii_digit() && previous {out.push(' ');}
            out.push(if i==0 {c.to_ascii_uppercase()}else{c});previous=c.is_ascii_alphabetic();
        }out
    }).collect::<Vec<_>>().join(" ")
}
fn header_frames(bytes:&[u8])->Result<usize,String> {
    if bytes.len()<100 || &bytes[..4]!=b"2LGA" || i32::from_le_bytes(bytes[4..8].try_into().unwrap())!=6 {
        return Err("Invalid GLA header".into());
    }
    let frames=i32::from_le_bytes(bytes[76..80].try_into().unwrap());
    let bones=i32::from_le_bytes(bytes[84..88].try_into().unwrap());
    if !(1..=1_000_000).contains(&frames) || !(1..=4096).contains(&bones) {return Err("Invalid GLA frame/bone count".into());}
    Ok(frames as usize)
}
fn bank(assets:&mut AssetSearchPath,config:&str)->Result<Vec<StudioCinematicClip>,String> {
    let source=source_for_config(config).ok_or("Not a separate humanoid animation set")?;
    let cfg=assets.read(config,59999).map_err(|e|e.to_string())?.ok_or("Missing animation.cfg")?;
    let animations=parse_animation_cfg(&cfg.bytes)?;
    let header=assets.read_prefix(&source.gla,100).map_err(|e|e.to_string())?.ok_or("Missing paired GLA")?;
    let frames=header_frames(&header.bytes)?;
    let set=source.gla.split("/_humanoid_").nth(1).unwrap_or("").split('/').next().unwrap_or("").to_owned();
    let title=set_label(&set);
    let mut clips=Vec::new();
    // The stock parser validates frame counts/loop flags. Read the authored FPS
    // token as well, avoiding its integer-millisecond frameLerp quantization.
    for line in std::str::from_utf8(&cfg.bytes).map_err(|e|e.to_string())?.lines() {
        let fields:Vec<_>=line.split("//").next().unwrap_or("").split_whitespace().collect();
        if fields.len()<5 || !fields[0].starts_with("BOTH_CIN_") {continue;}
        let Some(index)=jka_assets::animation::animation_index(fields[0]) else {continue;};
        let animation=animations.get(index as i32).unwrap();
        let first=i32::from(animation.first_frame);let count=i32::from(animation.num_frames);
        let fps=fields[4].parse::<f32>().map_err(|e|e.to_string())?;
        if count==0 || (first+count) as usize>frames || !fps.is_finite() || !(1.0..=240.0).contains(&fps) {continue;}
        let mut source=source.clone();source.animation=AnimationSet::name(index as i32).unwrap().to_owned();
        let number=source.animation.strip_prefix("BOTH_CIN_").unwrap();
        let clip=StudioCinematicClip {source:source.clone(),set:set.clone(),set_label:title.clone(),
            label:format!("{title} / CIN {number}"),first_frame:first,frame_count:count,fps,loop_frames:i32::from(animation.loop_frames)};
        // Repeated cfg rows have the same identity and the last authored record wins.
        if let Some(i)=clips.iter().position(|c:&StudioCinematicClip|c.source==source) {clips[i]=clip;}else{clips.push(clip);}
    }
    Ok(clips)
}
pub(in crate::app) fn scan(assets:&mut AssetSearchPath)->Result<StudioCinematicCatalog,String> {
    let mut configs:Vec<_>=assets.names().filter(|n|source_for_config(n).is_some()).map(str::to_owned).collect();configs.sort();
    let mut catalog=StudioCinematicCatalog::default();
    for config in configs {
        match bank(assets,&config) {Ok(mut clips)=>catalog.clips.append(&mut clips),Err(error)=>catalog.diagnostics.push(format!("{config}: {error}"))}
    }
    Ok(catalog)
}
pub(in crate::app) fn resolve(assets:&mut AssetSearchPath,source:&StudioCinematicSource)->Result<StudioCinematicClip,String> {
    bank(assets,&source.config()?)?.into_iter().find(|c|c.source.gla.eq_ignore_ascii_case(&source.gla)
        && c.source.animation==source.animation).ok_or_else(||format!("Missing cinematic {} in {}",source.animation,source.gla))
}
impl App {
    pub(in crate::app) fn ensure_studio_cinematics(&mut self) {
        let key=(self.ui_catalog.generation(),self.video.allow_asset_overrides);
        let library=&mut self.render_studio.cinematics;
        if library.key!=Some(key) {library.key=Some(key);library.pending=false;library.catalog=None;library.token=library.token.wrapping_add(1);}
        if library.pending || library.catalog.is_some() {return;}
        library.pending=true;
        self.ui_catalog.request(&self.base,self.game.as_deref(),super::ui_catalog::CatalogRequest::StudioCinematics {
            token:library.token,allow_overrides:self.video.allow_asset_overrides});
    }
    pub(in crate::app) fn resolve_studio_cinematic(&mut self,source:&StudioCinematicSource)->Result<StudioCinematicClip,String> {
        let library=&self.render_studio.cinematics;
        if library.key==Some((self.ui_catalog.generation(),self.video.allow_asset_overrides)) {
            if let Some(Ok(catalog))=library.catalog.as_ref() {
                if let Some(clip)=catalog.clips.iter().find(|clip|clip.source.gla.eq_ignore_ascii_case(&source.gla)
                    && clip.source.animation==source.animation) {return Ok(clip.clone());}
            }
        }
        // Project reloads can resolve a bank directly; they never wait for a UI scan.
        let mut assets=jka_assets::pk3::AssetSearchPath::open_game(&self.base,self.game.as_deref()).map_err(|e|e.to_string())?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);resolve(&mut assets,source)
    }
    pub(in crate::app) fn preview_studio_cinematic(&mut self,source:&StudioCinematicSource) {
        let result=self.resolve_studio_cinematic(source).and_then(|clip|clip.emote().map(|emote|(clip,emote)));
        match result {
            Ok((clip,emote))=>{
                let time=self.render_studio.timeline_time();self.render_studio.timeline.seek(time);
                let lane=self.render_studio.timeline.selected_clip.and_then(|i|self.render_studio.timeline.clips.get(i)).map_or(0,|c|c.lane);
                self.render_studio.cinematic_preview=Some(StudioCinematicPreview {clip,emote,playing:true,time:0.,
                    started:Instant::now(),lane,camera:Default::default(),pivot:[88.*self.profile_preview_zoom.clamp(0.45,2.5),8.,0.]});
                self.profile_preview_key=None;
            }
            Err(error)=>self.render_studio.status=error,
        }
    }
}
