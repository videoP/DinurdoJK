//! Demos.
use crate::app::egui_menu::{
    frontend, ui_catalog, App, DemoConsoleFilter, DemoMetadataResult, PathBuf,
};

impl App {
    /// A visible Refresh button rescans only the currently selected game.
    /// This does not re-mount game assets or require an fs_restart.
    fn refresh_selected_demo_browser(&mut self) {
        self.demo_native_folder_open = false;
        if self.demo_browser_jk2 {
            self.jk2_demo_browser.reset();
            self.ensure_jk2_demo_catalog();
        } else {
            self.reset_demo_browser_catalog();
            self.ensure_demo_catalog();
        }
        self.publish_ui();
    }

    /// JKA and JK2 have separate archives; switching does not reset either
    /// catalog or touch Render Studio's JKA source/mod selection.
    fn set_play_demo_game(&mut self, jk2: bool) {
        if self.demo_browser_jk2 == jk2 { return; }
        self.demo_browser_jk2 = jk2;
        self.demo_native_folder_open = false;
        self.jk2_demo_browser.folder_open = false;
        if jk2 { self.ensure_jk2_demo_catalog(); } else { self.ensure_demo_catalog(); }
        self.publish_ui();
    }

    pub(in crate::app::egui_menu) fn ensure_jk2_demo_catalog(&mut self) {
        if self.jk2_demo_browser.loaded { return; }
        self.jk2_demo_browser.loaded = true;
        let Some(local_base) = jka_assets::pk3::jk2_base_directory(&self.base, &self.fs_roots) else {
            // No JK2 mode without fs_jk2basepath.
            self.jk2_demo_browser.pending = false;
            self.jk2_demo_browser.error = Some("JK2 is not configured: set fs_jk2basepath".to_owned());
            return;
        };
        self.jk2_demo_browser.pending = true;
        self.jk2_demo_browser.error = None;
        self.ui_catalog.request(&self.base, None,
            ui_catalog::CatalogRequest::Jk2DemoFolders {
                token: self.jk2_demo_browser.generation,
                local_base,
                home_base: jka_assets::pk3::game_home_root(&self.fs_roots, true).map(|p| p.join("base")),
                root: self.fs_roots.jk2cdpath.clone(),
            });
    }

    fn ensure_jk2_demo_details(&mut self) {
        let browser = &mut self.jk2_demo_browser;
        let Some(path) = browser.entries.get(browser.selected).map(|e| e.path.clone()) else { return; };
        if browser.details.contains_key(&path) || browser.details_pending.as_ref() == Some(&path) { return; }
        browser.details_pending = Some(path.clone());
        self.ui_catalog.request(&self.base, None,
            ui_catalog::CatalogRequest::Jk2DemoDetails { token: browser.generation, path });
    }

    pub(in crate::app::egui_menu) fn select_jk2_demo_folder(&mut self, folder: PathBuf) {
        if self.jk2_demo_browser.folder.as_ref() == Some(&folder) { return; }
        self.jk2_demo_browser.reset();
        let browser = &mut self.jk2_demo_browser;
        browser.folder = Some(folder.clone());
        browser.loaded = true;
        browser.pending = true;
        // Retain known folder choices rather than rescanning the whole root.
        self.ui_catalog.request(&self.base, None,
            ui_catalog::CatalogRequest::Jk2DemoRecordings { token: browser.generation, folder });
    }

    fn play_selected_jk2_demo(&mut self) {
        let Some(entry) = self.jk2_demo_browser.entries.get(self.jk2_demo_browser.selected) else { return; };
        let path = entry.path.clone();
        // This is the existing JK2 conversion/asset path, including the
        // JK2 base/CD stock package validation. Never invoke /demo (.dm_26 VFS).
        self.play_demo_file(&path);
    }

    fn native_jk2_demo_browser_ui(&self) -> crate::ui::demo_browser::DemoBrowserUi {
        use crate::ui::demo_browser::{BrowserLine as L, DemoBrowserUi, LineKind as K};
        let b = &self.jk2_demo_browser;
        let current = b.entries.get(b.selected);
        let mut ui = DemoBrowserUi {
            jk2: true,
            folder: b.folder.as_ref().and_then(|path| b.folders.iter().find(|f| &f.path == path))
                .map(|f| f.label.clone()).unwrap_or_else(|| "base/demos".to_owned()),
            folders: b.folders.iter().map(|f| f.label.clone()).collect(),
            folder_open: b.folder_open,
            folder_scroll: b.folder_scroll,
            names: b.entries.iter().map(|r| r.filename.clone()).collect(),
            selected: b.selected,
            tab: b.inspector_tab,
            list_scroll: b.list_scroll,
            detail_scroll: b.detail_scroll,
            title: current.map(|r| r.filename.clone()).unwrap_or_else(|| "NO DEMO SELECTED".to_owned()),
            status: b.error.clone().unwrap_or_else(|| {
                if b.pending { "Scanning JK2 recordings...".to_owned() }
                else if b.folders.is_empty() { "No JK2 recordings in fs_jk2basepath or fs_jk2cdpath".to_owned() }
                else if b.entries.is_empty() { "No JK2 recordings in this folder".to_owned() }
                else { "Reading selected JK2 recording...".to_owned() }
            }),
            pointer: [self.cursor_position.0 as f32, self.cursor_position.1 as f32],
            ..Default::default()
        };
        let Some(path) = current.map(|e| &e.path) else { return ui; };
        let Some(result) = b.details.get(path) else { return ui; };
        let info = match result {
            Ok(info) => info,
            Err(err) => { ui.status = format!("JK2 metadata: {err}"); return ui; }
        };
        ui.summary = format!("{}  /  {}  /  {} snapshots", info.protocol,
            if info.map.is_empty() { "unknown map" } else { &info.map }, info.snapshots);
        let value = |name: String, right: String| L::new(K::Value, name, "", right);
        let heading = |name: &str| L::new(K::Heading, name, "", "");
        let note = |name: &str| L::new(K::Note, name, "", "");
        ui.lines = match b.inspector_tab {
            0 => vec![
                heading("JK2 RECORDING"),
                value(format!("VERSION  {}", info.protocol), format!("SIZE  {:.1} MB", info.bytes as f64 / 1048576.0)),
                value(format!("DURATION  {}", Self::format_demo_time(info.duration_ms.min(i64::from(i32::MAX)) as i32)), format!("MAP  {}", info.map)),
                value(format!("SNAPSHOTS  {}", info.snapshots), format!("DROPPED  {}", info.dropped)),
                value(format!("GAMESTATES  {}", info.segments), format!("RECORDS  {}", info.records)),
                heading("SERVER / MOD"),
                value(format!("SERVER  {}", info.server), format!("GAME  {}", info.mod_name)),
                heading("SOURCE"),
                note(&path.display().to_string()),
            ],
            1 => vec![heading("JK2 / PLAYERS"), note("Detailed player roster is not indexed for JK2 recordings yet.")],
            2 => vec![heading("JK2 / MATCH"), note("Round and duel indexing is currently JKA-only.")],
            3 => vec![heading("JK2 / CONSOLE"), note("Console timeline extraction is currently JKA-only.")],
            _ => vec![heading("JK2 / REFERENCED ASSETS"), note("JK2 assets use fs_jk2basepath, then fs_jk2cdpath/base.")],
        };
        ui
    }

    pub(in crate::app::egui_menu) fn poll_demo_metadata_jobs(&mut self) {
        let mut updated = false;
        while let Ok(done) = self.demo_metadata_rx.try_recv() {
            if done.generation != self.demo_metadata_generation {
                continue;
            }
            self.demo_metadata_inflight.remove(&done.demo_name);
            match done.result {
                Ok(metadata) => {
                    self.demo_metadata_errors.remove(&done.demo_name);
                    self.demo_metadata_cache.insert(done.demo_name, metadata);
                }
                Err(error) => {
                    self.demo_metadata_errors.insert(done.demo_name, error);
                }
            }
            self.egui_repaint_requested = true;
            updated = true;
        }
        if updated && self.frontend_page == crate::app::FrontendPage::PlayDemo { self.publish_ui(); }
    }

    pub(in crate::app::egui_menu) fn ensure_demo_metadata(&mut self, demo_name: &str) {
        self.poll_demo_metadata_jobs();
        if self.demo_metadata_cache.contains_key(demo_name)
            || self.demo_metadata_errors.contains_key(demo_name)
            || self.demo_metadata_inflight.contains(demo_name)
        {
            return;
        }

        let demo_name = demo_name.to_owned();
        self.demo_metadata_inflight.insert(demo_name.clone());
        let base = self.base.clone();
        let game = self.demo_browser_game_path();
        let tx = self.demo_metadata_tx.clone();
        let generation = self.demo_metadata_generation;
        let worker_demo_name = demo_name.clone();
        if let Err(error) = self.background_jobs.cpu(move || {
            let result = frontend::index_demo_metadata(&base, game.as_deref(), &worker_demo_name);
            let _ = tx.send(DemoMetadataResult { generation, demo_name: worker_demo_name, result });
        }) {
            self.demo_metadata_inflight.remove(&demo_name);
            self.demo_metadata_errors.insert(demo_name, format!("metadata queue: {error}"));
        }
    }

    pub(in crate::app::egui_menu) fn ensure_demo_catalog(&mut self) {
        if self.demo_catalog_loaded {
            return;
        }

        self.demo_catalog_loaded = true;
        self.demo_catalog_pending = true;
        self.demo_entries.clear();
        self.demo_catalog_error = None;
        self.ui_catalog.request(&self.base, None,
            ui_catalog::CatalogRequest::DemoGames { token: self.demo_catalog_generation });
    }

    pub(in crate::app) fn demo_browser_game_path(&self) -> Option<PathBuf> {
        let game = self.demo_game_filter.trim();
        if game.is_empty() || game.eq_ignore_ascii_case("base") {
            None
        } else {
            Some(self.base.parent().unwrap_or(&self.base).join(game))
        }
    }

    pub(in crate::app::egui_menu) fn reset_demo_browser_catalog(&mut self) {
        self.demo_catalog_loaded = false;
        self.demo_catalog_pending = false;
        self.demo_catalog_generation = self.demo_catalog_generation.wrapping_add(1);
        self.demo_catalog_error = None;
        self.demo_entries.clear();
        self.demo_selected = 0;
        self.demo_native_list_scroll = 0;
        self.demo_native_detail_scroll = 0;
        self.demo_native_folder_scroll = 0;
        self.demo_metadata_generation = self.demo_metadata_generation.wrapping_add(1);
        self.demo_metadata_cache.clear();
        self.demo_metadata_errors.clear();
        self.demo_metadata_inflight.clear();
    }

    pub(in crate::app::egui_menu) fn format_demo_time(ms: i32) -> String {
        let total_seconds = ms.max(0) / 1000;
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        if hours > 0 {
            format!("{hours}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes}:{seconds:02}")
        }
    }

    pub(in crate::app::egui_menu) fn demo_gametype_name(gametype: i32) -> &'static str {
        match gametype {
            0 => "FFA",
            1 => "Holocron",
            2 => "Jedi Master",
            3 => "Duel",
            4 => "Power Duel",
            5 => "Single Player",
            6 => "Team FFA",
            7 => "Siege",
            8 => "CTF",
            9 => "CTY",
            _ => "Unknown",
        }
    }

    pub(in crate::app::egui_menu) fn demo_team_summary(teams: &[i32]) -> String {
        teams
            .iter()
            .map(|team| match team {
                1 => "red",
                2 => "blue",
                3 => "spectator",
                _ => "free",
            })
            .collect::<Vec<_>>()
            .join("/")
    }


    /// Native Play Demo data, serialized only while the page is visible.
    /// No egui dependencies. All statuses come from the existing async indexer.
    pub(in crate::app) fn native_demo_browser_ui(&self) -> crate::ui::demo_browser::DemoBrowserUi {
        if self.demo_browser_jk2 { return self.native_jk2_demo_browser_ui(); }
        use crate::ui::demo_browser::DemoBrowserUi;
        let mut browser = DemoBrowserUi {
            folder: if self.demo_game_filter.is_empty() { "base".to_owned() } else { self.demo_game_filter.clone() },
            folders: self.demo_game_choices.clone(),
            folder_open: self.demo_native_folder_open,
            folder_scroll: self.demo_native_folder_scroll,
            names: self.demo_entries.iter().map(|e| e.demo_name.clone()).collect(),
            selected: self.demo_selected,
            tab: self.demo_inspector_tab as usize,
            list_scroll: self.demo_native_list_scroll,
            detail_scroll: self.demo_native_detail_scroll,
            missing_only: self.demo_assets_missing_only,
            console_filter: match self.demo_console_filter { DemoConsoleFilter::All=>0, DemoConsoleFilter::Print=>1, DemoConsoleFilter::Chat=>2, DemoConsoleFilter::Event=>3 },
            title: self.demo_entries.get(self.demo_selected).map(|e| e.demo_name.clone()).unwrap_or_else(|| "NO DEMO SELECTED".to_owned()),
            status: self.demo_catalog_error.clone().unwrap_or_else(|| if self.demo_catalog_pending { "Scanning demo folders...".to_owned() } else if self.demo_entries.is_empty() { "No recordings in this folder".to_owned() } else { "Indexing selected recording...".to_owned() }),
            pointer: [self.cursor_position.0 as f32, self.cursor_position.1 as f32],
            ..Default::default()
        };
        let Some(metadata) = self.demo_metadata_cache.get(&browser.title) else {
            if let Some(error)=self.demo_metadata_errors.get(&browser.title) { browser.status=format!("Metadata scan failed: {error}"); }
            return browser;
        };
        browser.summary = Self::demo_browser_summary(metadata);
        browser.lines = Self::demo_browser_inspector_lines(
            metadata, self.demo_inspector_tab, self.demo_assets_missing_only,
            self.demo_console_filter,
        );
        browser
    }

    /// Summary header shared by the native page and Studio modal.
    pub(in crate::app::egui_menu) fn demo_browser_summary(metadata: &frontend::DemoMetadata) -> String {
        let missing=metadata.referenced_assets.iter().filter(|a|!a.found_locally).count();
        format!("{}  /  {}  /  {} players  /  {} missing assets",
            Self::demo_gametype_name(metadata.gametype),Self::format_demo_time(metadata.duration_ms),
            metadata.players.len(),missing)
    }

    /// Single source of presentation content for Play Demo's native Taffy/WGPU
    /// inspector and Render Studio's large egui-hosted details window. Both
    /// consume the same BrowserLine records (including asset colors and seek
    /// markers); only the surface renderer differs.
    pub(in crate::app::egui_menu) fn demo_browser_inspector_lines(
        metadata: &frontend::DemoMetadata,
        tab: frontend::DemoInspectorTab,
        missing_only: bool,
        console_filter: DemoConsoleFilter,
    ) -> Vec<crate::ui::demo_browser::BrowserLine> {
        use crate::ui::demo_browser::{BrowserLine as L, LineKind as K};
        let fmt=Self::format_demo_time;
        let missing=metadata.referenced_assets.iter().filter(|a|!a.found_locally).count();
        let mut lines=Vec::new();
        let heading=|name:&str|L::new(K::Heading,name,"","");
        let entry=|name:String,sub:String,right:String|L::new(K::Entry,name,sub,right);
        let value=|name:String,right:String|L::new(K::Value,name,"",right);
        match tab {
            frontend::DemoInspectorTab::Overview => {
                lines.push(heading("RECORDING"));
                lines.push(value(format!("DURATION  {}",fmt(metadata.duration_ms)),format!("SIZE  {:.1} MB",metadata.size_bytes as f64/1048576.0)));
                lines.push(value(format!("MOD  {}",metadata.fs_game),format!("PROTOCOL  {}",metadata.protocol)));
                lines.push(value(format!("MODE  {}",Self::demo_gametype_name(metadata.gametype)),format!("KILLS  {}",metadata.kill_count)));
                lines.push(value(format!("SNAPSHOTS  {} ({:.1}/s)",metadata.snapshot_count,metadata.average_snapshot_rate),format!("PLAYERS  {}",metadata.players.len())));
                lines.push(value(format!("MAPS  {}",metadata.maps.len()),format!("ROUNDS / DUELS  {} / {}",metadata.rounds.len(),metadata.duels.len())));
                lines.push(heading("SERVER / RECORDED POV"));
                lines.push(value(metadata.server_name.clone().unwrap_or_else(||"Unknown server".into()),
                    metadata.recorded_player_name.clone().unwrap_or_else(||format!("Client {:?}",metadata.recorded_client_num))));
                lines.push(L::new(K::Note,format!("SOURCE  {}",metadata.source),"",""));
                lines.push(heading("MAPS"));
                for map in &metadata.maps {lines.push(entry(map.map_name.clone(),"Referenced BSP".to_owned(),fmt(map.duration_ms)));}
                lines.push(heading("LOCAL ASSETS"));
                lines.push(value(format!("{} referenced assets",metadata.referenced_assets.len()),format!("{} missing",missing)));
                if missing>0 {lines.push(L::new(K::Note,"Open REFERENCED ASSETS to see the missing resources.","",""));}
            }
            frontend::DemoInspectorTab::Players => {
                lines.push(heading("PLAYER IDENTITIES / CUMULATIVE PRESENCE"));
                for player in &metadata.players {
                    lines.push(entry(player.name.clone(),format!("{}  ·  {}  ·  {} - {}",
                        player.model,Self::demo_team_summary(&player.teams),fmt(player.first_seen_ms),fmt(player.last_seen_ms)),fmt(player.duration_ms)));
                }
            }
            frontend::DemoInspectorTab::Rounds => {
                lines.push(heading("ROUNDS"));
                for round in &metadata.rounds {
                    let score=if let Some([r,b])=round.team_scores {format!("Red {r} / Blue {b}")}
                        else if let Some((name,score))=&round.leader {format!("Leader {name} {score}")}
                        else {String::new()};
                    lines.push(entry(format!("ROUND {}  /  {}",round.index,round.map_name),
                        format!("{} players  ·  {} kills  ·  {}",round.players,round.kills,score),
                        format!("{} +{}",fmt(round.start_ms),fmt(round.duration_ms))));
                }
                lines.push(heading("DUELS"));
                for duel in &metadata.duels {
                    lines.push(entry(format!("DUEL {}  /  {}",duel.index,duel.players.join(" vs ")),
                        format!("{}  ·  Winner: {}",duel.kind.label(),duel.winner.as_deref().unwrap_or("Unknown")),
                        format!("{} +{}",fmt(duel.start_ms),fmt(duel.duration_ms))));
                }
            }
            frontend::DemoInspectorTab::Console => {
                lines.push(heading("MESSAGES"));
                for msg in metadata.console.iter().filter(|msg|console_filter.matches(&msg.kind)) {
                    let mut row=entry(msg.text.clone(),msg.kind.label().to_owned(),fmt(msg.elapsed_ms));
                    row.seek_ms=Some(msg.elapsed_ms);
                    lines.push(row);
                }
            }
            frontend::DemoInspectorTab::Assets => {
                lines.push(L::new(K::Note,"Status from recorded mod + base / loose files and PK3s", "", ""));
                for kind in [frontend::DemoAssetKind::Map,frontend::DemoAssetKind::PlayerModel,
                    frontend::DemoAssetKind::WorldModel,frontend::DemoAssetKind::Npc,frontend::DemoAssetKind::Saber] {
                    let items:Vec<_>=metadata.referenced_assets.iter().filter(|a|a.kind==kind && (!missing_only || !a.found_locally)).collect();
                    if missing_only && items.is_empty() {continue;}
                    lines.push(heading(&format!("{}  /  {}",kind.label(),items.len())));
                    for asset in items {
                        let mut line=entry(asset.name.clone(),kind.label().to_owned(),
                            if asset.found_locally{"FOUND".into()}else{"MISSING".into()});
                        line.present=Some(asset.found_locally);
                        lines.push(line);
                    }
                }
                if missing_only && missing==0 {lines.push(L::new(K::Note,"No missing references detected.","",""));}
            }
        }
        lines
    }

    /// Native page prep executes even though the egui Play Demo widget tree is absent.
    pub(in crate::app) fn update_native_demo_browser(&mut self) {
        if self.demo_browser_jk2 {
            self.ensure_jk2_demo_catalog();
            if !self.jk2_demo_browser.pending { self.ensure_jk2_demo_details(); }
            return;
        }
        self.ensure_demo_catalog();
        if self.demo_catalog_pending || self.demo_entries.is_empty() { return; }
        self.demo_selected=self.demo_selected.min(self.demo_entries.len()-1);
        let name=self.demo_entries[self.demo_selected].demo_name.clone();
        self.ensure_demo_metadata(&name);
    }


}

impl App {
    /// Direct winit input for the native Play Demo screen. No egui widgets or
    /// egui hit testing are involved; input and WGPU share the Taffy rectangles.
    pub(in crate::app) fn native_demo_browser_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
        use winit::keyboard::{KeyCode, PhysicalKey};
        use crate::ui::{demo_browser, flex};
        if !self.front_end || self.frontend_page != crate::app::FrontendPage::PlayDemo
            || self.overlay != crate::ui::OverlayMode::Game || self.video_confirmation.is_some()
            || self.loading.is_some() || self.connection_modal_active()
        { return false; }
        let Some(window) = &self.window else { return false; };
        let size = window.inner_size();
        let l = demo_browser::layout(size.width, size.height);
        let point = [self.cursor_position.0 as f32, self.cursor_position.1 as f32];
        if self.demo_browser_jk2 { return self.native_jk2_demo_browser_event(event, l, point); }
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let next = (position.x, position.y);
                // Hover is native as well: publish new geometry only when the
                // pointer changes, not on render-thread frame production.
                if self.cursor_position != next {
                    self.cursor_position = next;
                    self.publish_ui();
                }
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 34.0) as f32,
                };
                if amount.abs() <= f32::EPSILON { return true; }
                let step = (amount.abs().ceil() as usize).saturating_mul(3).min(40);
                if self.demo_native_folder_open && flex::contains([l.folder[0], l.folder[1]+l.folder[3], l.folder[2], 31.0*12.0],point) {
                    let max=self.demo_game_choices.len().saturating_sub(12);
                    self.demo_native_folder_scroll=if amount < 0.0 {self.demo_native_folder_scroll.saturating_add(step).min(max)}
                        else {self.demo_native_folder_scroll.saturating_sub(step)};
                } else if flex::contains(l.list, point) {
                    let max = self.demo_entries.len().saturating_sub(l.list_visible());
                    self.demo_native_list_scroll = if amount < 0.0 {
                        self.demo_native_list_scroll.saturating_add(step).min(max)
                    } else {self.demo_native_list_scroll.saturating_sub(step)};
                } else if flex::contains(l.content, point) {
                    let lines = self.native_demo_browser_ui().lines.len();
                    let max = lines.saturating_sub(l.detail_visible());
                    self.demo_native_detail_scroll = if amount < 0.0 {
                        self.demo_native_detail_scroll.saturating_add(step).min(max)
                    } else {self.demo_native_detail_scroll.saturating_sub(step)};
                }
                self.publish_ui();
                true
            }
            WindowEvent::MouseInput {state:ElementState::Pressed,button:MouseButton::Left,..} => {
                let mut changed=true;
                if let Some(index) = (0..2).find(|&i| flex::contains(l.game_tab_rect(i),point)) {
                    self.set_play_demo_game(index == 1);
                } else if flex::contains(l.refresh, point) {
                    self.refresh_selected_demo_browser();
                } else if self.demo_native_folder_open {
                    let selected=self.demo_game_choices.iter().skip(self.demo_native_folder_scroll).take(12).enumerate().find(|(index,_)|
                        flex::contains(l.folder_item(*index),point)).map(|(_,name)|name.clone());
                    self.demo_native_folder_open=false;
                    if let Some(folder)=selected {
                        let new_filter=if folder.eq_ignore_ascii_case("base") {String::new()}else{folder};
                        if self.demo_game_filter!=new_filter {
                            self.demo_game_filter=new_filter;
                            self.demo_native_list_scroll=0;
                            self.demo_native_detail_scroll=0;
                            self.reset_demo_browser_catalog();
                            self.ensure_demo_catalog();
                        }
                    }
                } else if flex::contains(l.folder,point) {
                    self.demo_native_folder_open=true;
                    self.demo_native_folder_scroll=0;
                } else if let Some(index)=(0..demo_browser::TABS.len()).find(|&i|flex::contains(l.tab_rect(i),point)) {
                    self.demo_inspector_tab=frontend::DemoInspectorTab::ALL[index];
                    self.demo_native_detail_scroll=0;
                } else if self.demo_inspector_tab==frontend::DemoInspectorTab::Assets
                    && flex::contains(l.missing,point) {
                    self.demo_assets_missing_only=!self.demo_assets_missing_only;
                    self.demo_native_detail_scroll=0;
                } else if self.demo_inspector_tab==frontend::DemoInspectorTab::Console
                    && flex::contains(l.filters,point) {
                    if let Some(index)=(0..4).find(|&i|flex::contains(l.filter_rect(i),point)) {
                        self.demo_console_filter=match index {1=>DemoConsoleFilter::Print,2=>DemoConsoleFilter::Chat,3=>DemoConsoleFilter::Event,_=>DemoConsoleFilter::All};
                        self.demo_native_detail_scroll=0;
                    }
                } else if flex::contains(l.list_rows(),point) {
                    let b=l.list_rows();
                    let index=self.demo_native_list_scroll+((point[1]-b[1])/demo_browser::ROW).floor() as usize;
                    if index<self.demo_entries.len() {
                        self.demo_selected=index;
                        self.demo_native_detail_scroll=0;
                        let name=self.demo_entries[index].demo_name.clone();
                        self.ensure_demo_metadata(&name);
                    }
                } else if flex::contains(l.play,point) {
                    if !self.demo_entries.is_empty() { self.play_selected_demo(); }
                } else if flex::contains(l.detail_rows(),point) && self.demo_inspector_tab==frontend::DemoInspectorTab::Console {
                    let rows=self.native_demo_browser_ui().lines;
                    let b=l.detail_rows();
                    let index=self.demo_native_detail_scroll+((point[1]-b[1])/demo_browser::ROW).floor() as usize;
                    if let Some(ms)=rows.get(index).and_then(|line|line.seek_ms) {self.play_selected_demo_at(ms);}
                } else { changed=false; }
                if changed {self.publish_ui();}
                true
            }
            WindowEvent::MouseInput {..} => true,
            WindowEvent::KeyboardInput {event:key,..} if key.state==ElementState::Pressed && !key.repeat => {
                let PhysicalKey::Code(code)=key.physical_key else{return true;};
                match code {
                    KeyCode::Escape | KeyCode::Backquote | KeyCode::PrintScreen => return false,
                    KeyCode::Enter | KeyCode::NumpadEnter => {
                        if !self.demo_entries.is_empty() {self.play_selected_demo();}
                    }
                    KeyCode::ArrowDown => {
                        if !self.demo_entries.is_empty() {
                            self.demo_selected=(self.demo_selected+1).min(self.demo_entries.len()-1);
                            self.demo_native_list_scroll=self.demo_native_list_scroll.max(self.demo_selected+1-l.list_visible().min(self.demo_selected+1));
                            self.demo_native_detail_scroll=0;
                            let name=self.demo_entries[self.demo_selected].demo_name.clone();
                            self.ensure_demo_metadata(&name);
                        }
                    }
                    KeyCode::ArrowUp => {
                        self.demo_selected=self.demo_selected.saturating_sub(1);
                        self.demo_native_list_scroll=self.demo_native_list_scroll.min(self.demo_selected);
                        self.demo_native_detail_scroll=0;
                        if let Some(item)=self.demo_entries.get(self.demo_selected) {
                            let name=item.demo_name.clone(); self.ensure_demo_metadata(&name);
                        }
                    }
                    KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                        let step=if code==KeyCode::ArrowRight {1}else{4};
                        let tab=(self.demo_inspector_tab as usize+step)%5;
                        self.demo_inspector_tab=frontend::DemoInspectorTab::ALL[tab];
                        self.demo_native_detail_scroll=0;
                    }
                    KeyCode::PageDown => self.demo_native_detail_scroll=self.demo_native_detail_scroll.saturating_add(l.detail_visible()),
                    KeyCode::PageUp => self.demo_native_detail_scroll=self.demo_native_detail_scroll.saturating_sub(l.detail_visible()),
                    _ => return true,
                }
                self.publish_ui();
                true
            }
            WindowEvent::KeyboardInput {..} | WindowEvent::Ime(..) => true,
            _ => false,
        }
    }
}

impl App {
    /// Native input uses the same Taffy hitboxes for either archive.
    fn native_jk2_demo_browser_event(
        &mut self, event: &winit::event::WindowEvent,
        layout: crate::ui::demo_browser::Layout,
        point: [f32; 2],
    ) -> bool {
        use crate::ui::{demo_browser, flex};
        use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
        use winit::keyboard::{KeyCode, PhysicalKey};
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position=(position.x,position.y);
                self.publish_ui();
                true
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let amount=match delta { MouseScrollDelta::LineDelta(_,y)=>*y,MouseScrollDelta::PixelDelta(p)=>(p.y/34.0) as f32 };
                let step=(amount.abs().ceil() as usize).saturating_mul(3).min(40);
                let b=&mut self.jk2_demo_browser;
                if b.folder_open && flex::contains([layout.folder[0],layout.folder[1]+layout.folder[3],layout.folder[2],34.0*12.0],point) {
                    let max=b.folders.len().saturating_sub(12);
                    b.folder_scroll=if amount<0.0 {b.folder_scroll.saturating_add(step).min(max)}else{b.folder_scroll.saturating_sub(step)};
                } else if flex::contains(layout.list,point) {
                    let max=b.entries.len().saturating_sub(layout.list_visible());
                    b.list_scroll=if amount<0.0 {b.list_scroll.saturating_add(step).min(max)}else{b.list_scroll.saturating_sub(step)};
                } else if flex::contains(layout.content,point) {
                    let max=self.native_jk2_demo_browser_ui().lines.len().saturating_sub(layout.detail_visible());
                    let b=&mut self.jk2_demo_browser;
                    b.detail_scroll=if amount<0.0 {b.detail_scroll.saturating_add(step).min(max)}else{b.detail_scroll.saturating_sub(step)};
                }
                self.publish_ui();true
            }
            WindowEvent::MouseInput { state:ElementState::Pressed,button:MouseButton::Left,.. } => {
                if let Some(index)=(0..2).find(|&i|flex::contains(layout.game_tab_rect(i),point)) {
                    self.set_play_demo_game(index==1);
                } else if flex::contains(layout.refresh,point) {
                    self.refresh_selected_demo_browser();
                } else if self.jk2_demo_browser.folder_open {
                    let selected=self.jk2_demo_browser.folders.iter().skip(self.jk2_demo_browser.folder_scroll).take(12)
                        .enumerate().find(|(i,_)|flex::contains(layout.folder_item(*i),point))
                        .map(|(_,f)|f.path.clone());
                    self.jk2_demo_browser.folder_open=false;
                    if let Some(path)=selected { self.select_jk2_demo_folder(path); }
                } else if flex::contains(layout.folder,point) {
                    self.jk2_demo_browser.folder_open=true;
                    self.jk2_demo_browser.folder_scroll=0;
                } else if let Some(index)=(0..demo_browser::TABS.len()).find(|&i|flex::contains(layout.tab_rect(i),point)) {
                    self.jk2_demo_browser.inspector_tab=index;
                    self.jk2_demo_browser.detail_scroll=0;
                } else if flex::contains(layout.list_rows(),point) {
                    let rows=layout.list_rows();
                    let index=self.jk2_demo_browser.list_scroll+((point[1]-rows[1])/demo_browser::ROW).floor() as usize;
                    if index<self.jk2_demo_browser.entries.len() {
                        self.jk2_demo_browser.selected=index;
                        self.jk2_demo_browser.detail_scroll=0;
                        self.ensure_jk2_demo_details();
                    }
                } else if flex::contains(layout.play,point) { self.play_selected_jk2_demo(); }
                self.publish_ui(); true
            }
            WindowEvent::MouseInput {..} => true,
            WindowEvent::KeyboardInput {event:key,..} if key.state==ElementState::Pressed && !key.repeat => {
                let PhysicalKey::Code(code)=key.physical_key else{return true;};
                let b=&mut self.jk2_demo_browser;
                match code {
                    KeyCode::Escape | KeyCode::Backquote | KeyCode::PrintScreen => return false,
                    KeyCode::Enter | KeyCode::NumpadEnter => self.play_selected_jk2_demo(),
                    KeyCode::ArrowDown => {
                        if !b.entries.is_empty() {
                            b.selected=(b.selected+1).min(b.entries.len()-1);
                            b.list_scroll=b.list_scroll.max(b.selected+1-layout.list_visible().min(b.selected+1));
                            b.detail_scroll=0;
                            self.ensure_jk2_demo_details();
                        }
                    }
                    KeyCode::ArrowUp => {
                        b.selected=b.selected.saturating_sub(1);
                        b.list_scroll=b.list_scroll.min(b.selected);
                        b.detail_scroll=0;
                        self.ensure_jk2_demo_details();
                    }
                    KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                        b.inspector_tab=(b.inspector_tab+if code==KeyCode::ArrowRight {1}else{4})%5;
                        b.detail_scroll=0;
                    }
                    KeyCode::PageDown => b.detail_scroll=b.detail_scroll.saturating_add(layout.detail_visible()),
                    KeyCode::PageUp => b.detail_scroll=b.detail_scroll.saturating_sub(layout.detail_visible()),
                    _ => return true,
                }
                self.publish_ui();true
            }
            WindowEvent::KeyboardInput {..} | WindowEvent::Ime(..) => true,
            _ => false,
        }
    }
}

#[cfg(test)]
mod shared_inspector_tests {
    use super::*;
    use crate::app::frontend::{DemoAssetKind, DemoAssetReference, DemoInspectorTab, DemoMetadata};
    use crate::ui::demo_browser::LineKind;

    #[test]
    fn missing_only_shares_the_exact_same_asset_rows_with_studio() {
        let mut metadata = DemoMetadata::default();
        metadata.referenced_assets = vec![
            DemoAssetReference { kind: DemoAssetKind::Map, name: "mp/duel2".into(), found_locally: true },
            DemoAssetReference { kind: DemoAssetKind::Npc, name: "models/players/wampa/model.glm".into(), found_locally: false },
            DemoAssetReference { kind: DemoAssetKind::Saber, name: "single_1".into(), found_locally: false },
        ];
        let lines = App::demo_browser_inspector_lines(&metadata, DemoInspectorTab::Assets, true, DemoConsoleFilter::All);
        assert_eq!(lines.iter().filter(|line|line.present==Some(false)).count(), 2);
        assert!(!lines.iter().any(|line|line.title=="mp/duel2"));
        assert!(lines.iter().any(|line|line.title=="NPC MODELS  /  1" && line.kind==LineKind::Heading));
        assert!(lines.iter().any(|line|line.title=="single_1" && line.right=="MISSING"));
    }

    #[test]
    fn overview_and_assets_read_the_same_map_reference() {
        let mut metadata = DemoMetadata::default();
        metadata.referenced_assets.push(DemoAssetReference {
            kind: DemoAssetKind::Map, name: "mp/ffa3".into(), found_locally: false,
        });
        let rows = App::demo_browser_inspector_lines(&metadata, DemoInspectorTab::Assets, false, DemoConsoleFilter::All);
        assert!(rows.iter().any(|line|line.title=="mp/ffa3" && line.present==Some(false)));
    }
}
