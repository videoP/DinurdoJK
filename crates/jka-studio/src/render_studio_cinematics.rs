//! Identity of a separate single-player cinematic animation bank.
use serde::{Deserialize, Serialize};

const PREFIX:&str="studio-cinematic:";
#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub struct StudioCinematicSource {pub gla:String,pub animation:String}
impl StudioCinematicSource {
    pub fn id(&self)->String {format!("{PREFIX}{}#{}",self.gla.to_ascii_lowercase(),self.animation.to_ascii_uppercase())}
    pub fn from_emote(emote:&jka_emotes::manifest::Emote)->Option<Self> {
        if !emote.id.starts_with(PREFIX) {return None;}
        let source=Self {gla:emote.gla.clone(),animation:emote.variant_name.clone()};
        (emote.id==source.id()).then_some(source)
    }
    pub fn config(&self)->Result<String,String> {
        let path=jka_emotes::manifest::qpath(&self.gla)?.to_ascii_lowercase();
        let (folder,file)=path.rsplit_once('/').ok_or("Missing cinematic source folder")?;
        let set=folder.strip_prefix("models/players/_humanoid_").ok_or("Not a separate humanoid cinematic bank")?;
        if set.is_empty() || set.contains('/') || file!=format!("_humanoid_{set}.gla")
            || !self.animation.starts_with("BOTH_CIN_") {return Err("Invalid cinematic source identity".into());}
        Ok(format!("{folder}/animation.cfg"))
    }
}

