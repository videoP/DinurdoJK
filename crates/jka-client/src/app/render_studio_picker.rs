//! Embedded, non-blocking-from-the-OS Render Studio media browser.
//!
//! A Win32 GetOpenFileNameW dialog blocks the game/UI thread and can appear
//! behind an exclusive-fullscreen swapchain. File selection therefore happens
//! entirely in egui; no OS dialog, shell process or window-mode switch.
use std::{collections::BTreeSet, fs, path::{Path, PathBuf}, sync::mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StudioPickerKind { Audio, Demo }

impl StudioPickerKind {
    fn allowed(self, path: &Path) -> bool {
        let Some(ext) = path.extension().and_then(|s|s.to_str()) else {return false};
        match self {
            Self::Audio => ["wav", "mp3", "flac", "ogg"].iter().any(|v|ext.eq_ignore_ascii_case(v)),
            Self::Demo => ext.eq_ignore_ascii_case("dm_26"),
        }
    }
    pub(super) fn title(self) -> &'static str {
        match self {Self::Audio => "Import audio files", Self::Demo => "Import DM26 recordings"}
    }
    fn description(self) -> &'static str {
        match self {Self::Audio => "WAV / MP3 / FLAC / OGG", Self::Demo => "Jedi Academy demo (.dm_26)"}
    }
}

#[derive(Clone)]
struct StudioFileEntry { name: String, path: PathBuf, is_dir: bool }

pub(super) enum StudioPickerAction {
    Cancel,
    Import(StudioPickerKind, Vec<PathBuf>),
}

#[derive(Default)]
pub(super) struct StudioFilePicker {
    pub kind: Option<StudioPickerKind>,
    directory: PathBuf,
    location: String,
    filter: String,
    selected: BTreeSet<PathBuf>,
    entries: Vec<StudioFileEntry>,
    pending: Option<mpsc::Receiver<Result<Vec<StudioFileEntry>, String>>>,
    error: String,
    drives: bool,
}

impl StudioFilePicker {
    fn home() -> PathBuf {
        let home = std::env::var_os("USERPROFILE").or_else(||std::env::var_os("HOME"))
            .map(PathBuf::from).unwrap_or_else(||std::env::current_dir().unwrap_or_default());
        if home.is_dir() {home} else {std::env::current_dir().unwrap_or_default()}
    }

    pub fn open(&mut self, kind: StudioPickerKind) {
        self.kind = Some(kind);
        self.filter.clear();
        self.selected.clear();
        self.error.clear();
        self.drives = false;
        // Do not stat a disconnected drive on the render thread.
        let dir = if self.directory.as_os_str().is_empty() {Self::home()} else {self.directory.clone()};
        self.browse(dir);
    }

    fn browse(&mut self, directory: PathBuf) {
        let Some(kind)=self.kind else {return};
        self.location=directory.to_string_lossy().into_owned();
        self.directory=directory.clone();
        self.entries.clear();
        self.error.clear();
        self.drives=false;
        let (tx,rx)=mpsc::channel();
        // Even a disconnected mapped drive must not stall the game's render/UI
        // thread. Old scans cannot overwrite newer navigation requests: dropping
        // their receivers discards stale results.
        match std::thread::Builder::new().name("studio-file-scan".into()).spawn(move || {
            let result=scan_directory(&directory,kind);
            let _=tx.send(result);
        }) {
            Ok(_)=>self.pending=Some(rx),
            Err(error)=>{self.pending=None;self.error=format!("Cannot browse folder: {error}");},
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let result=self.pending.as_ref().map(|rx|rx.try_recv());
        match result {
            Some(Ok(Ok(entries)))=>{self.entries=entries;self.pending=None;},
            Some(Ok(Err(error)))=>{self.error=error;self.pending=None;},
            Some(Err(mpsc::TryRecvError::Disconnected))=>{
                self.error="File browser worker stopped unexpectedly".into();self.pending=None;
            }
            Some(Err(mpsc::TryRecvError::Empty))=>{
                ctx.request_repaint_after(std::time::Duration::from_millis(80));
            }
            None=>{}
        }
    }

    fn go_to_location(&mut self) {
        let requested=PathBuf::from(self.location.trim().trim_matches('"'));
        if self.kind.is_some_and(|k|k.allowed(&requested)) {
            self.selected.insert(requested.clone());
            if let Some(parent)=requested.parent() {self.browse(parent.to_path_buf());}
        } else if !requested.as_os_str().is_empty() {
            // Directory validation happens on the scan worker, not in egui.
            self.browse(requested);
        } else {
            self.error="Enter a folder or supported audio/demo file path".into();
        }
    }

    pub(super) fn show(&mut self, ui:&mut egui::Ui) -> Option<StudioPickerAction> {
        let kind=self.kind?;
        self.poll(ui.ctx());
        ui.label(kind.description());
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            let up=self.directory.parent().map(Path::to_path_buf);
            if ui.add_enabled(up.is_some(),egui::Button::new("Up")).clicked() {
                if let Some(up)=up {self.browse(up);}
            }
            if ui.button("Home").clicked() {self.browse(Self::home());}
            if ui.button("Drives").clicked() {self.drives=!self.drives;}
            if ui.button("Refresh").clicked() {self.browse(self.directory.clone());}
        });
        if self.drives {
            ui.horizontal_wrapped(|ui| {
                for path in available_roots() {
                    let label=path.display().to_string();
                    if ui.button(label).clicked() {self.browse(path);}
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label("Folder");
            let field=ui.add(egui::TextEdit::singleline(&mut self.location)
                .desired_width((ui.available_width()-48.0).max(100.0)));
            if ui.button("Go").clicked() || (field.lost_focus() && ui.input(|i|i.key_pressed(egui::Key::Enter))) {
                self.go_to_location();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(egui::TextEdit::singleline(&mut self.filter)
                .desired_width(ui.available_width()).hint_text("Search this folder…"));
        });
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(238,156,105),&self.error);
        }
        ui.separator();
        if self.pending.is_some() {
            ui.horizontal(|ui|{ui.spinner();ui.label("Reading folder…");});
        }
        let filter=self.filter.to_lowercase();
        let visible:Vec<_>=self.entries.iter().filter(|entry|
            filter.is_empty() || entry.name.to_lowercase().contains(&filter)).cloned().collect();
        let mut import_one=None;
        let mut navigate=None;
        egui::ScrollArea::vertical().max_height(370.0).auto_shrink([false,false]).show(ui,|ui| {
            ui.set_min_width(ui.available_width());
            if visible.is_empty() && self.pending.is_none() && self.error.is_empty() {
                ui.label("No matching files or folders.");
            }
            for entry in visible {
                let label=format!("{} {}",if entry.is_dir {"▸"} else {"♪"},entry.name);
                let response=ui.selectable_label(!entry.is_dir && self.selected.contains(&entry.path),label)
                    .on_hover_text(entry.path.display().to_string());
                if entry.is_dir {
                    if response.clicked() {navigate=Some(entry.path);}
                } else if response.double_clicked() {
                    import_one=Some(entry.path);
                } else if response.clicked() {
                    if !self.selected.insert(entry.path.clone()) {self.selected.remove(&entry.path);}
                }
            }
        });
        if let Some(path)=navigate {self.browse(path);}
        if let Some(path)=import_one {
            self.selected.clear();
            self.selected.insert(path);
            let files=self.selected.iter().cloned().collect();
            return Some(StudioPickerAction::Import(kind,files));
        }
        ui.separator();
        let mut action=None;
        ui.horizontal(|ui| {
            ui.label(format!("{} file(s) selected",self.selected.len()));
            if !self.selected.is_empty() && ui.button("Clear selection").clicked(){self.selected.clear();}
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui| {
                if ui.button("Cancel").clicked() {action=Some(StudioPickerAction::Cancel);}
                if ui.add_enabled(!self.selected.is_empty(),egui::Button::new("Import selected"))
                    .clicked() {
                    action=Some(StudioPickerAction::Import(kind,self.selected.iter().cloned().collect()));
                }
            });
        });
        action
    }
}

/// Directory enumeration runs outside the game thread and never opens audio
/// or demo contents. Importer validates the selected path again on use.
fn scan_directory(directory:&Path,kind:StudioPickerKind) -> Result<Vec<StudioFileEntry>,String> {
    let dir=fs::read_dir(directory)
        .map_err(|e|format!("Cannot open {}: {e}",directory.display()))?;
    let mut entries=Vec::new();
    for item in dir {
        let Ok(item)=item else {continue};
        let path=item.path();
        let is_dir=path.is_dir();
        if !is_dir && (!path.is_file() || !kind.allowed(&path)) {continue;}
        entries.push(StudioFileEntry{
            name:item.file_name().to_string_lossy().into_owned(),path,is_dir,
        });
    }
    entries.sort_by(|a,b| b.is_dir.cmp(&a.is_dir)
        .then_with(||a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(entries)
}

#[cfg(windows)]
fn available_roots() -> Vec<PathBuf> {
    // Fast drive-bitmask query; unlike trying A:..Z: with exists(), this does
    // not synchronously probe disconnected network or removable drives.
    #[link(name="kernel32")]
    unsafe extern "system" {fn GetLogicalDrives()->u32;}
    let drives=unsafe{GetLogicalDrives()};
    (0u32..26).filter(|&i|drives & (1u32<<i)!=0)
        .map(|i|PathBuf::from(format!("{}:\\",(b'A'+i as u8) as char))).collect()
}

#[cfg(not(windows))]
fn available_roots() -> Vec<PathBuf> {vec![PathBuf::from("/")]}
