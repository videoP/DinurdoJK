//! Boot-time and on-demand asset mapping recovery. All artwork and controls use native Taffy/WGPU: this screen must remain usable with ZERO mounted PK3s.
//!
//! Do not hot-swap VFS roots here: the renderer, archive index and loader
//! workers receive their roots during startup. Persist and relaunch instead.
use super::App;
use std::path::{Path, PathBuf};

/// JKA's required stock archives cannot be supplied by fs_forcegame, a
/// downloaded server mod, or DinurdoJK/shared. Inspect only JKA base layers,
/// and preserve the same home/base/CD root semantics as the mounted VFS.
fn jka_base_layers(base: &Path, roots: &jka_assets::pk3::FilesystemRoots) -> Vec<PathBuf> {
    let mut dirs = Vec::with_capacity(3);
    if let Some(home) = jka_assets::pk3::game_home_root(roots, false) {
        dirs.push(home.join("base"));
    }
    // `base` is the active JKA base directory after startup's --base and
    // fs_basepath precedence. Never rediscover it from the INI here.
    dirs.push(base.to_path_buf());
    if let Some(cd) = &roots.cdpath {
        dirs.push(cd.join("base"));
    }
    dirs
}

pub(crate) fn jka_assets_present(
    base: &Path,
    roots: &jka_assets::pk3::FilesystemRoots,
) -> bool {
    let dirs = jka_base_layers(base, roots);
    (0..=3).all(|n| dirs.iter().any(|dir| dir.join(format!("assets{n}.pk3")).is_file()))
}

/// Preview the JKA base that would be active on a fresh launch without
/// CLI --base overrides, which Save & Restart deliberately drops.
fn restart_jka_base(roots: &jka_assets::pk3::FilesystemRoots, current: &Path) -> PathBuf {
    roots.basepath.as_ref().map(|root| root.join("base")).unwrap_or_else(|| {
        crate::filesystem_paths::default_base_directory().unwrap_or_else(|_| current.to_path_buf())
    })
}


use crate::ui::asset_mapping::{AssetMappingUi, FolderUi, Hit};
use winit::{event::{ElementState, Ime, MouseButton, MouseScrollDelta, WindowEvent}, keyboard::{KeyCode, PhysicalKey}};

struct FolderPicker {
    target: usize,
    current: PathBuf,
    address: String,
    folders: Vec<PathBuf>,
    error: Option<String>,
    scroll: usize,
}
impl FolderPicker {
    fn new(target:usize, preferred:&str, base:&Path)->Self {
        let path=crate::filesystem_paths::parse_root(base,preferred)
            .ok().flatten().unwrap_or_else(||base.parent().unwrap_or(base).to_path_buf());
        let mut picker=Self {target,address:path.display().to_string(),current:path,folders:Vec::new(),error:None,scroll:0};
        picker.refresh();
        picker
    }
    fn refresh(&mut self) {
        self.address=self.current.display().to_string();
        self.folders.clear();
        self.scroll=0;
        self.error=None;
        let entries=match std::fs::read_dir(&self.current) {
            Ok(e)=>e,
            Err(err)=>{self.error=Some(format!("{}: {err}",self.current.display()));return;}
        };
        self.folders=entries.flatten().filter(|e|e.file_type().is_ok_and(|ty|ty.is_dir()))
            .map(|e|e.path()).collect();
        self.folders.sort_by_key(|p|p.file_name().unwrap_or_else(||std::ffi::OsStr::new("")).to_string_lossy().to_lowercase());
    }
    fn navigate(&mut self,to:PathBuf) { self.current=to;self.refresh(); }
}

pub(super) struct AssetMappingState {
    cdpath:String, jk2cdpath:String, basepath:String, jk2basepath:String, forcegame:String,
    picker:Option<FolderPicker>, feedback:Option<String>, allow_close:bool,
    /// 0..4 input fields, 5 is the folder picker address.
    focus:Option<usize>, caret:usize, selection_all:bool, scroll:f32,
}
impl AssetMappingState {
    pub fn new(roots:&jka_assets::pk3::FilesystemRoots,base:&Path)->Self {
        Self {
            cdpath:show_path(roots.cdpath.as_deref()),
            jk2cdpath:show_path(roots.jk2cdpath.as_deref()),
            basepath:roots.basepath.as_deref().unwrap_or_else(||base.parent().unwrap_or(base)).display().to_string(),
            jk2basepath:jka_assets::pk3::jk2_root_directory(base,roots).display().to_string(),
            forcegame:roots.forcegame.clone().unwrap_or_default(),
            picker:None,feedback:None,allow_close:false,focus:None,caret:0,selection_all:false,scroll:0.0,
        }
    }
    pub fn from_main_menu(roots:&jka_assets::pk3::FilesystemRoots,base:&Path)->Self {
        let mut state=Self::new(roots,base);
        state.allow_close=true;
        state
    }
    fn proposed(&self,base:&Path,original:&jka_assets::pk3::FilesystemRoots)
        ->Result<jka_assets::pk3::FilesystemRoots,String> {
        let mut roots=original.clone();
        roots.cdpath=crate::filesystem_paths::parse_root(base,&self.cdpath)?;
        roots.jk2cdpath=crate::filesystem_paths::parse_root(base,&self.jk2cdpath)?;
        roots.basepath=crate::filesystem_paths::parse_root(base,&self.basepath)?;
        roots.jk2basepath=crate::filesystem_paths::parse_root(base,&self.jk2basepath)?;
        roots.forcegame=crate::filesystem_paths::validate_game_name(&self.forcegame)?;
        Ok(roots)
    }
    fn values(&self)->[String;5] {
        [self.basepath.clone(),self.cdpath.clone(),self.jk2basepath.clone(),
            self.jk2cdpath.clone(),self.forcegame.clone()]
    }
    fn edit(&mut self,index:usize)->Option<&mut String> {
        match index {
            0=>Some(&mut self.basepath),1=>Some(&mut self.cdpath),
            2=>Some(&mut self.jk2basepath),3=>Some(&mut self.jk2cdpath),
            4=>Some(&mut self.forcegame),5=>self.picker.as_mut().map(|p|&mut p.address),
            _=>None,
        }
    }
    fn focus(&mut self,target:usize) {
        self.focus=Some(target);
        self.caret=self.edit(target).map_or(0,|s|s.len());
        self.selection_all=false;
    }
    fn replace_selection(&mut self,new:&str) {
        let Some(i)=self.focus else {return;};
        let at=self.caret;
        let all=self.selection_all;
        let mut new_caret=None;
        if let Some(s)=self.edit(i) {
            if all {s.clear();}
            let at=if all {0}else{at.min(s.len())};
            let mut at=at;
            while !s.is_char_boundary(at) {at-=1;}
            let room=4096usize.saturating_sub(s.len());
            let replacement:String=new.chars().filter(|c|!c.is_control()).take(room).collect();
            s.insert_str(at,&replacement);
            new_caret=Some(at+replacement.len());
        }
        if let Some(caret)=new_caret {self.caret=caret;self.selection_all=false;self.feedback=None;}
    }
    fn key_edit(&mut self,key:KeyCode,control:bool)->bool {
        let Some(i)=self.focus else {return false;};
        if control && key==KeyCode::KeyA {
            self.selection_all=true;
            self.caret=self.edit(i).map_or(0,|s|s.len());
            return true;
        }
        if control && key==KeyCode::KeyC {
            if self.selection_all {if let Some(s)=self.edit(i) {let _=crate::clipboard::set_text(s);}}
            return true;
        }
        if control && key==KeyCode::KeyV {
            if let Ok(value)=crate::clipboard::get_text() {self.replace_selection(&value);}
            return true;
        }
        if control && key==KeyCode::KeyX {
            if self.selection_all {
                if let Some(s)=self.edit(i){let _=crate::clipboard::set_text(s);}
                self.replace_selection("");
            }
            return true;
        }
        let old=self.caret;
        let all=self.selection_all;
        let mut caret=old;
        let Some(s)=self.edit(i) else {return false;};
        let at=old.min(s.len());
        match key {
            KeyCode::Backspace=>{
                if all {s.clear();caret=0;}
                else if at>0 {let prev=s[..at].char_indices().last().map_or(0,|(i,_)|i);
                    s.replace_range(prev..at,"");caret=prev;}
            }
            KeyCode::Delete=>{
                if all {s.clear();caret=0;}
                else if at<s.len() {let next=at+s[at..].chars().next().map_or(0,char::len_utf8);
                    s.replace_range(at..next,"");}
            }
            KeyCode::ArrowLeft=>{caret=if all {0}else{s[..at].char_indices().last().map_or(0,|(i,_)|i)};}
            KeyCode::ArrowRight=>{caret=if all {s.len()}else{at+s[at..].chars().next().map_or(0,char::len_utf8)};}
            KeyCode::Home=>{caret=0;}
            KeyCode::End=>{caret=s.len();}
            _=>return false,
        }
        self.caret=caret;
        self.selection_all=false;
        self.feedback=None;
        true
    }
    fn ui(&self,base:&Path,roots:&jka_assets::pk3::FilesystemRoots,pointer:[f32;2])->AssetMappingUi {
        let mut result=AssetMappingUi {
            values:self.values(),feedback:self.feedback.clone(),allow_close:self.allow_close,
            focus:self.focus,caret:self.caret,selection_all:self.selection_all,
            scroll:self.scroll,pointer, ..Default::default()
        };
        for (i,v) in result.values.iter().take(4).enumerate() {
            result.valid[i]=match crate::filesystem_paths::parse_root(base,v) {
                Ok(Some(root))=>Some(root.join("base").is_dir()),
                Ok(None)=>None,
                Err(_)=>Some(false),
            };
        }
        match crate::filesystem_paths::validate_game_name(&self.forcegame) {
            Ok(Some(name)) => {
                result.force_status=Some(jka_assets::pk3::dinurdojk_directory_for(base,roots)
                    .join(name).is_dir());
            }
            Err(_) => result.force_invalid=true,
            Ok(None) => {}
        }
        if let Ok(proposed)=self.proposed(base,roots) {
            let dirs=jka_base_layers(&restart_jka_base(&proposed,base),&proposed);
            result.retail=std::array::from_fn(|n|dirs.iter().any(|d|d.join(format!("assets{n}.pk3")).is_file()));
            result.jk2_status=std::array::from_fn(|idx| {
                let protocol=if idx==0 {2}else{4};
                jka_assets::pk3::missing_jk2_stock_archive(base,&proposed,protocol)
                    .map_or("READY".to_owned(),|path|format!("Missing {path}"))
            });
        } else if result.feedback.is_none() {
            result.feedback=Some("Correct the invalid path or override folder name before saving".into());
        }
        if let Some(p)=&self.picker {
            result.picker=Some(FolderUi {
                address:p.address.clone(),
                folders:p.folders.iter().map(|d|d.file_name().unwrap_or_else(||std::ffi::OsStr::new("(root)")).to_string_lossy().into_owned()).collect(),
                scroll:p.scroll,error:p.error.clone(),
            });
        }
        result
    }
}
fn show_path(value:Option<&Path>)->String {value.map_or_else(String::new,|p|p.display().to_string())}

/// Preserve normal launch arguments, but do not replay root +set commands:
/// these would override the paths just saved by the recovery UI and trap the
/// user in the missing-assets screen again. The old `--base` override is also
/// dropped; unrelated launch arguments remain intact.
fn relaunch_arguments() -> Vec<std::ffi::OsString> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut result = Vec::with_capacity(args.len());
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].to_string_lossy();
        if (flag.eq_ignore_ascii_case("+set") || flag.eq_ignore_ascii_case("+seta"))
            && args.get(i + 1).is_some_and(|name| {
                matches!(name.to_string_lossy().to_ascii_lowercase().as_str(),
                    "fs_basepath" | "fs_jk2basepath" | "fs_cdpath" |
                    "fs_jk2cdpath" | "fs_forcegame")
            })
        {
            i += (args.len() - i).min(3);
            continue;
        }
        // --base points at a concrete base/ directory, overriding fs_basepath
        // even after saving. A GUI restart must not replay that old override.
        if flag == "--base" {
            i += (args.len() - i).min(2);
            continue;
        }
        result.push(args[i].clone());
        i += 1;
    }
    result
}


impl App {
    /// UI snapshot and direct event handling never touch the mounted virtual FS.
    pub(in crate::app) fn asset_mapping_ui(&self)->Option<AssetMappingUi> {
        self.asset_mapping.as_ref().map(|s|s.ui(&self.base,&self.fs_roots,
            [self.cursor_position.0 as f32,self.cursor_position.1 as f32]))
    }
    pub(in crate::app) fn native_asset_mapping_event(&mut self,event:&WindowEvent)->bool {
        if self.asset_mapping.is_none(){return false;}
        let size=self.window.as_ref().map(|w|w.inner_size());
        let point=[self.cursor_position.0 as f32,self.cursor_position.1 as f32];
        // Expensive filesystem status checks run on state mutations, not pointer motion.
        let geometry=size.map(|size| {
            let state=self.asset_mapping.as_ref().expect("mapping");
            let mut ui=AssetMappingUi::default();
            ui.allow_close=state.allow_close;
            ui.picker=state.picker.as_ref().map(|p|FolderUi {scroll:p.scroll,folders:vec![String::new();p.folders.len()],..Default::default()});
            ui.scroll=state.scroll;
            crate::ui::asset_mapping::layout(size.width,size.height,&ui)
        });
        match event {
            WindowEvent::ModifiersChanged(modifiers)=>{self.modifiers=modifiers.state();return true;}
            WindowEvent::CursorMoved {position,..}=>{
                let next=[position.x as f32,position.y as f32];
                let ui=self.asset_mapping.as_ref().expect("mapping");
                let old=geometry.as_ref().and_then(|l|l.hit(point,&ui.hit_ui()));
                let new=geometry.as_ref().and_then(|l|l.hit(next,&ui.hit_ui()));
                self.cursor_position=(position.x,position.y);
                if old!=new {self.publish_ui();}
                return true;
            }
            WindowEvent::CursorLeft{..}=>{
                self.cursor_position=(-1.0,-1.0);
                self.publish_ui();return true;
            }
            WindowEvent::MouseWheel{delta,..}=>{
                let amount=match delta {
                    MouseScrollDelta::LineDelta(_,y)=>*y*44.0,
                    MouseScrollDelta::PixelDelta(p)=>p.y as f32,
                };
                if let (Some(l),Some(state))=(geometry.as_ref(),self.asset_mapping.as_mut()) {
                    if let (Some(modal),Some(picker))=(l.picker,state.picker.as_mut()) {
                        if crate::ui::flex::contains(modal.list,point) {
                            let step=(amount.abs()/modal.row_h).ceil().max(1.0) as usize;
                            let max=picker.folders.len().saturating_sub(l.picker_visible_rows());
                            picker.scroll=if amount<0.0 {picker.scroll.saturating_add(step).min(max)}
                                else {picker.scroll.saturating_sub(step)};
                        }
                    } else if crate::ui::flex::contains(l.viewport,point) {
                        state.scroll=(state.scroll-amount).clamp(0.0,l.scroll_max);
                    }
                }
                self.publish_ui();return true;
            }
            WindowEvent::MouseInput{state:ElementState::Pressed,button:MouseButton::Left,..}=>{
                let hit=geometry.as_ref().and_then(|l|l.hit(point,&self.asset_mapping.as_ref().expect("mapping").hit_ui()));
                match hit {
                    Some(Hit::Edit(i))=>{self.asset_mapping.as_mut().unwrap().focus(i);}
                    Some(Hit::Browse(i))=>{
                        let state=self.asset_mapping.as_mut().unwrap();
                        let value=state.values()[i].clone();
                        state.picker=Some(FolderPicker::new(i,&value,&self.base));
                        state.focus(5);
                    }
                    Some(Hit::Address)=>self.asset_mapping.as_mut().unwrap().focus(5),
                    Some(Hit::Go)=>{if let Some(p)=self.asset_mapping.as_mut().unwrap().picker.as_mut(){p.navigate(PathBuf::from(p.address.clone()));}}
                    Some(Hit::Parent)=>{if let Some(p)=self.asset_mapping.as_mut().unwrap().picker.as_mut(){
                        if let Some(parent)=p.current.parent(){p.navigate(parent.to_path_buf());}
                    }}
                    Some(Hit::Refresh)=>{if let Some(p)=self.asset_mapping.as_mut().unwrap().picker.as_mut(){p.refresh();}}
                    Some(Hit::Folder(i))=>{if let Some(p)=self.asset_mapping.as_mut().unwrap().picker.as_mut(){
                        if let Some(folder)=p.folders.get(i).cloned(){p.navigate(folder);}
                    }}
                    Some(Hit::Choose)=>{
                        let state=self.asset_mapping.as_mut().unwrap();
                        if let Some(p)=state.picker.take() {
                            if p.current.is_dir() {
                                if let Some(edit)=state.edit(p.target){*edit=p.current.display().to_string();}
                                state.focus(p.target);state.feedback=None;
                            } else {state.feedback=Some(format!("Not a directory: {}",p.current.display()));state.picker=Some(p);}
                        }
                    }
                    Some(Hit::Cancel)=>{let state=self.asset_mapping.as_mut().unwrap();state.picker=None;state.focus=None;}
                    Some(Hit::Save)=>self.save_asset_mapping(false),
                    Some(Hit::Restart)=>self.save_asset_mapping(true),
                    Some(Hit::Back)=>self.close_asset_mapping(),
                    None=>{if self.asset_mapping.as_ref().unwrap().picker.is_none(){self.asset_mapping.as_mut().unwrap().focus=None;}}
                }
                self.publish_ui();return true;
            }
            WindowEvent::Ime(Ime::Commit(value))=>{
                if let Some(state)=self.asset_mapping.as_mut(){state.replace_selection(value);}
                self.publish_ui();return true;
            }
            WindowEvent::KeyboardInput{event:key,..}=>{
                if key.state==ElementState::Released{return true;}
                let PhysicalKey::Code(code)=key.physical_key else {
                    if let Some(text)=key.text.as_ref(){self.asset_mapping.as_mut().unwrap().replace_selection(text);self.publish_ui();}
                    return true;
                };
                if code==KeyCode::Escape && !key.repeat {
                    if self.asset_mapping.as_ref().unwrap().picker.is_some() {
                        let state=self.asset_mapping.as_mut().unwrap();state.picker=None;state.focus=None;
                    } else {self.close_asset_mapping();}
                } else if code==KeyCode::Tab && !key.repeat && self.asset_mapping.as_ref().unwrap().picker.is_none() {
                    let state=self.asset_mapping.as_mut().unwrap();
                    let next=state.focus.map_or(0,|i|(i+1)%5);
                    state.focus(next);
                } else if code==KeyCode::Enter && !key.repeat && self.asset_mapping.as_ref().unwrap().picker.is_some() {
                    let state=self.asset_mapping.as_mut().unwrap();
                    if state.focus==Some(5) {if let Some(p)=state.picker.as_mut(){p.navigate(PathBuf::from(p.address.clone()));}}
                } else {
                    let control=self.modifiers.control_key();
                    let handled=self.asset_mapping.as_mut().unwrap().key_edit(code,control);
                    if !handled && !control && !self.modifiers.alt_key() {
                        if let Some(text)=key.text.as_ref() {self.asset_mapping.as_mut().unwrap().replace_selection(text);}
                    }
                }
                self.publish_ui();return true;
            }
            WindowEvent::MouseInput{..}|WindowEvent::Ime(..)=>return true,
            _=>{}
        }
        false
    }
    fn close_asset_mapping(&mut self) {
        if self.asset_mapping.as_ref().is_some_and(|s|s.allow_close) {
            self.asset_mapping=None;
            self.egui_repaint_requested=true;
        }
    }
    fn save_asset_mapping(&mut self,restart:bool) {
        let Some(state)=self.asset_mapping.as_ref() else {return;};
        let result=state.proposed(&self.base,&self.fs_roots).and_then(|proposed| {
            // Creating the explicitly selected client override is not a game
            // asset migration. It stays client-owned and is never a write root
            // for configs, demos, screenshots, or downloaded game files.
            if let Some(name)=proposed.forcegame.as_deref() {
                let directory=jka_assets::pk3::dinurdojk_directory_for(&self.base,&proposed).join(name);
                std::fs::create_dir_all(&directory).map_err(|error|
                    format!("Cannot create client override {}: {error}",directory.display()))?;
            }
            // Existing INI store and no live VFS remount. Root paths are never
            // redirected into base/ and forcegame never becomes a write root.
            crate::filesystem_paths::save(&self.base,&proposed)?;
            Ok(proposed)
        });
        match result {
            Ok(proposed)=>{
                self.fs_roots=proposed.clone();
                let feedback=if restart {
                    if !jka_assets_present(&restart_jka_base(&proposed,&self.base),&proposed) {
                        Some("Saved, but JKA assets0.pk3 through assets3.pk3 are still missing. Check fs_basepath or fs_cdpath.".to_owned())
                    } else {
                        match std::env::current_exe().and_then(|exe|
                            std::process::Command::new(exe).args(relaunch_arguments()).spawn()) {
                            Ok(_)=>{self.quit_requested=true;None}
                            Err(e)=>Some(format!("Paths saved, but could not restart: {e}")),
                        }
                    }
                } else {Some("Paths saved to DinurdoJK/DinurdoJK-paths.ini; restart to activate the new roots.".into())};
                if let Some(state)=self.asset_mapping.as_mut(){state.feedback=feedback;}
            }
            Err(error)=>{if let Some(state)=self.asset_mapping.as_mut(){state.feedback=Some(format!("Cannot save: {error}"));}}
        }
    }
}
impl AssetMappingState {
    /// Minimal hit-test data; never touches disks on every hover event.
    fn hit_ui(&self)->AssetMappingUi {
        AssetMappingUi {allow_close:self.allow_close,scroll:self.scroll,
            picker:self.picker.as_ref().map(|p|FolderUi {scroll:p.scroll,folders:vec![String::new();p.folders.len()],..Default::default()}),
            ..Default::default()}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_jka_set_may_be_split_across_jka_base_and_jka_cd_base() {
        let dir = std::env::temp_dir().join(format!("djk-map-roots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let jka = dir.join("JKA GameData");
        let jk2 = dir.join("JK2 GameData");
        let cd = dir.join("JKA Retail");
        for target in [jka.join("base"), jk2.join("base"), cd.join("base")] {
            std::fs::create_dir_all(target).unwrap();
        }
        for n in [0, 1] {
            std::fs::write(jka.join("base").join(format!("assets{n}.pk3")), b"JKA").unwrap();
        }
        std::fs::write(cd.join("base/assets2.pk3"), b"JKA").unwrap();
        std::fs::write(jk2.join("base/assets3.pk3"), b"JK2 not JKA").unwrap();
        let mut roots = jka_assets::pk3::FilesystemRoots::default();
        roots.cdpath = Some(cd.clone());
        roots.jk2basepath = Some(jk2);
        assert!(!jka_assets_present(&jka.join("base"), &roots));
        std::fs::write(cd.join("base/assets3.pk3"), b"JKA").unwrap();
        assert!(jka_assets_present(&jka.join("base"), &roots));
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn recognizes_only_base_assets_not_forcegame_mod_pk3s() {
        let root = std::env::temp_dir().join(format!("djk-asset-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("japro")).unwrap();
        std::fs::write(root.join("japro/assets0.pk3"), b"not JKA retail").unwrap();
        let base = root.join("base");
        let mut roots = jka_assets::pk3::FilesystemRoots::default();
        roots.forcegame = Some("japro".into());
        assert!(!jka_assets_present(&base, &roots));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("assets0.pk3"), b"stock").unwrap();
        assert!(!jka_assets_present(&base, &roots));
        for n in 1..=3 { std::fs::write(base.join(format!("assets{n}.pk3")), b"stock").unwrap(); }
        assert!(jka_assets_present(&base, &roots));
        std::fs::remove_dir_all(root).unwrap();
    }
}
