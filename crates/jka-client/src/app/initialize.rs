//! Initialize.
use crate::app::{
    config, egui_pages, hitch, mpsc, normalize_anti_aliasing, quality, scene, server_browser,
    spawn_map_loader, ui, ui_catalog, App, Arc, AssetFilter, BTreeMap, BindKey, Bindings,
    BrowserUiState, Camera, ChatLogRangeFilter, ChatMode, ClientInfo, ConsoleSize,
    DemoConsoleFilter, DemoViewMode, EntityGraphUi, ShaderRemapUi, EventLoopProxy, ForcedPlayerModels,
    FrontendPage, HashMap, HashSet, Instant, KeyCode, MapLoadingState, ModifiersState, OverlayMode,
    PathBuf, PerfStats, PhysicsMapMesh, ProfileStudioCamera, SpectatorCameraState, ThirdPersonCameraState,
    UserEvent, VecDeque, VideoSection, VideoSettings, ViewLatchMode,
    DEMO_EXPORT_DEFAULT_FPS, DEMO_FREE_SPEED_DEFAULT_INDEX, DemoVideoPreset,
};

impl App {
    pub fn new(
        base: PathBuf,
        game: Option<PathBuf>,
        initial_source: scene::MapSource,
        launch_map: bool,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self, String> {
        let loader_tx = spawn_map_loader(base.clone(), proxy.clone())?;
        let log_rx = crate::logging::subscribe();
        let settings_dir = jka_assets::pk3::active_game_directory(&base, game.as_deref());
        let extensions = crate::extensions::Manager::spawn(base.clone(), game.clone())?;
        // External LUTs must be listed before the saved r_colorLut is parsed.
        crate::color_lut::scan_external(&base, game.as_deref());
        crate::startup_timeline::mark(crate::startup_timeline::Milestone::LutScanReady);
        let config_path = settings_dir.join("DinurdoJK.cfg");
        let mut video = config::load_video_settings(&config_path);
        crate::logging::set_developer_level(video.developer_level);
        crate::logging::set_renderer_verbose_level(video.renderer_verbose);
        let audio = config::load_audio_settings(&config_path);
        let mut presentation =
            config::load_client_presentation_settings(&config_path);
        ui::movement_components::normalize(&mut presentation.strafe_helper.flags,
            &mut presentation.japro.speedometer.flags);
        // OpenJK/TaystJK mirror cg_drawTeamOverlay into the read-only CVAR_USERINFO
        // key `teamoverlay`; the server uses it to decide whether to send tinfo.
        presentation.network.team_overlay = presentation.team_overlay.mode > 0;
        let forced_player_models = match ForcedPlayerModels::parse(&presentation.force_model) {
            Ok(parsed) => parsed,
            Err(error) => {
                eprintln!("Ignoring invalid cg_forceModel from config: {error}");
                presentation.force_model = "0".to_owned();
                None
            }
        };
        // AO modes are mutually exclusive so startup/config A/B tests never pay for both.
        if video.static_bsp_ao {
            video.ssao = false;
            // BAKED is the hybrid HQ path; keep the user's bake tuning.
            video.static_bsp_ao_lightmap = true;
        }
        // AA is a single user-facing choice. Resolve conflicting MSAA/FXAA/TAA
        // settings so disabled modes do not keep hidden cost.
        let aa_normalized = normalize_anti_aliasing(&mut video);
        let mut bindings = Bindings::load(&config_path);
        // OpenJK's scoreboard is normally on Tab. Preserve an existing Tab bind,
        // but make +scores the default when this config has no Tab assignment.
        let tab = BindKey::Keyboard(KeyCode::Tab);
        if bindings.get(tab).is_none() {
            bindings.set(tab, "+scores");
        }
        crate::startup_timeline::mark(crate::startup_timeline::Milestone::ConfigReady);
        let fps_cap_input = video.fps_cap.to_string();
        let default_video = VideoSettings::default();
        crate::emotes::set_cubic_interpolation(video.emote_cubic);
        crate::emotes::set_effects_enabled(video.emote_effects);
        let sun_custom_initialized = video.sun_override
            || (video.sun_yaw - default_video.sun_yaw).abs() > 0.0001
            || (video.sun_pitch - default_video.sun_pitch).abs() > 0.0001
            || (video.sun_intensity - default_video.sun_intensity).abs() > 0.0001
            || video.sun_color != default_video.sun_color;
        // If no native client config exists yet, persist the effective defaults
        // shortly after startup. Also rewrite
        // configs that stacked multiple AA methods.
        let mut config_dirty = !config_path.is_file() || aa_normalized;
        let fresh_install = !config_path.is_file();
        let initial_label = initial_source.label();
        let initial_loading =
            launch_map.then(|| MapLoadingState::new(0, initial_label.clone(), game.as_deref()));
        let front_end = !launch_map;
        let (server_browser_tx, server_browser_rx) = server_browser::spawn()?;
        let mut server_browser =
            BrowserUiState::new(&settings_dir, presentation.master_servers.clone());
        server_browser.apply_filters_string(&presentation.server_filters);
        let (demo_metadata_tx, demo_metadata_rx) = mpsc::channel();
        let (race_ghost_tx, race_ghost_rx) = mpsc::channel();
        let (race_ghost_web_tx, race_ghost_web_rx) = mpsc::channel();
        let ui_catalog = ui_catalog::UiCatalog::spawn()?;
        let background_jobs = super::background_jobs::BackgroundJobs::spawn()?;
        crate::startup_timeline::mark(crate::startup_timeline::Milestone::WorkersReady);
        let (background_result_tx, background_result_rx) = mpsc::channel();
        let mut strafe_trails =
            crate::strafe_trail::Manager::spawn(presentation.strafe_trail.clone())?;
        if presentation.strafe_trail.log_name.trim() != "0"
            && !presentation.strafe_trail.log_name.trim().is_empty()
        {
            if let Err(error) = strafe_trails.set_log_name(
                jka_assets::pk3::active_game_directory(&base, game.as_deref()),
                &presentation.strafe_trail.log_name,
            ) {
                eprintln!("Ignoring invalid cg_logStrafeTrail from config: {error}");
            }
        }
        let mut windows_timer_resolution = crate::windows_timer::TimerResolutionGuard::default();
        if video.timer_resolution_1ms {
            if let Err(error) = windows_timer_resolution.set_enabled(true) {
                eprintln!("1 ms Windows timer request disabled: {error}");
                video.timer_resolution_1ms = false;
                config_dirty = true;
            }
        }
        // Windows Credential Manager enumeration is optional startup UI data.
        // Do not block window creation on this potentially slow OS call.
        let accounts_proxy = proxy.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("credential-catalog".to_owned())
            .spawn(move || {
                let result = crate::credential_store::list();
                crate::startup_timeline::mark(crate::startup_timeline::Milestone::CredentialsReady);
                let _ = accounts_proxy.send_event(UserEvent::SavedAccountsReady(result));
            })
        {
            eprintln!("Could not start saved-account enumeration: {error}");
        }
        let saved_accounts = Vec::new();
        let account_credential_error = None;
        crate::emotes::start_catalog(&base);
        // Main-menu sign lettering (Star Jedi outlines). Rare work: one thread.
        {
            let (base, game) = (base.clone(), game.clone());
            let _ = std::thread::Builder::new().name("neon-font".into()).spawn(move || {
                let Ok(mut assets) = jka_assets::pk3::AssetSearchPath::open_game(&base, game.as_deref()) else { return };
                for path in crate::neon_menu::font::OUTLINE_FONT_PATHS {
                    if let Ok(Some(asset)) = assets.read(path, 8 << 20) {
                        match crate::neon_menu::font::install_outline_font(&asset.bytes) {
                            Ok(()) => println!("Neon menu lettering: {path}"),
                            Err(error) => eprintln!("Neon menu lettering {path}: {error}"),
                        }
                        return;
                    }
                }
            });
        }
        let emote_wheel = super::emote_wheel::EmoteWheel::load(&base);
        let mut app = Self {
            extensions,
            updater: crate::updater::State::load(&settings_dir),
            base,
            startup_game: game.clone(),
            game,
            map_name: initial_label.clone(),
            map_editor: None,
            entity_graph: None,
            entity_marker_cache: None,
            entity_marker_placement_sig: 0,
            entity_graph_ui: EntityGraphUi::default(),
            source_map_edit_on_load: false,
            map_edit_preview_request_id: None,
            map_edit_preview_pending_collision: None,
            map_edit_preview_dirty: false,
            map_edit_toolbar_rect: None,
            initial_source,
            front_end,
            frontend_page: FrontendPage::Main,
            game_launch_screen: presentation.game_launch_screen,
            frontend_skip_chrome_fade: presentation.game_launch_screen != config::GameLaunchScreen::Main,
            neon_menu: Default::default(),
            solo_maps: Vec::new(),
            solo_map_selected: 0,
            solo_catalog_loaded: false,
            solo_catalog_error: None,
            solo_levelshot_texture: None,
            solo_levelshot_texture_map: None,
            source_maps: Vec::new(),
            source_map_selected: 0,
            source_map_catalog_loaded: false,
            source_map_catalog_error: None,
            source_map_levelshot_texture: None,
            source_map_levelshot_texture_map: None,
            asset_viewer_entries: Vec::new(),
            asset_viewer_cache: None,
            asset_viewer_catalog_loaded: false,
            asset_viewer_catalog_error: None,
            asset_viewer_selected: None,
            asset_viewer_filter: AssetFilter::All,
            asset_viewer_search: String::new(),
            asset_viewer_folder: String::new(),
            asset_viewer_folder_suggestion: 0,
            asset_viewer_shader_file: String::new(),
            asset_viewer_detail_path: None,
            asset_viewer_detail: None,
            asset_viewer_shader_name: None,
            asset_viewer_model_yaw: 180.0,
            asset_viewer_model_pitch: 0.0,
            asset_viewer_model_zoom: 1.0,
            screenshot_entries: Vec::new(),
            screenshot_catalog_loaded: false,
            screenshot_catalog_error: None,
            screenshot_selected: None,
            screenshot_search: String::new(),
            screenshot_game_filter: String::new(),
            screenshot_preview_texture: None,
            screenshot_preview_path: None,
            screenshot_preview_pending: None,
            screenshot_preview_error: None,
            pending_screenshot_jump: None,
            startup_screenshot_select: None,
            chat_log_browser_entries: Vec::new(),
            chat_log_browser_catalog_loaded: false,
            chat_log_browser_catalog_error: None,
            chat_log_browser_selected: None,
            chat_log_browser_detail_path: None,
            chat_log_browser_detail: None,
            chat_log_browser_mod_filter: String::new(),
            chat_log_browser_server_filter: String::new(),
            chat_log_browser_search: String::new(),
            chat_log_browser_range: ChatLogRangeFilter::All,
            chat_log_browser_date_from: String::new(),
            chat_log_browser_date_to: String::new(),
            chat_log_browser_show_global: true,
            chat_log_browser_show_team: true,
            chat_log_browser_show_located: true,
            chat_log_browser_show_voice: true,
            chat_log_browser_show_session: true,
            asset_preview_player_presenter: None,
            asset_preview_entity_presenter: None,
            asset_preview_model_key: None,
            asset_preview_shader_key: None,
            asset_preview_fx: None,
            menu_props: None,
            menu_look_play: false,
            menu_look_oneshot: false,
            asset_preview_viewport_key: None,
            profile_background_requested: None,
            profile_background_available: false,
            profile_background_maps: None,
            profile_models: Vec::new(),
            profile_sabers: Vec::new(),
            profile_catalog_loaded: false,
            profile_catalog_error: None,
            profile_selected_section: 1,
            profile_studio_started: Instant::now(),
            profile_studio_last_frame: Instant::now(),
            profile_studio_spin_velocity: 0.0,
            profile_studio_camera: ProfileStudioCamera::default(),
            profile_studio_head_look: [0.0, 0.0],
            profile_studio_world_anchors: [None; 7],
            profile_body_highlight: None,
            profile_studio_cosmetic_group: 0,
            profile_studio_show_saber: true,
            profile_studio_skin_visible: [true; 4],
            profile_model_search: String::new(),
            profile_model_staged: None,
            profile_model_icon_textures: HashMap::new(),
            crosshair_image_textures: HashMap::new(),
            profile_name_input: presentation.network.name.clone(),
            profile_force_preview_pending: None,
            profile_force_preview_active: None,
            profile_force_preview_started: Instant::now(),
            profile_preview_yaw: 180.0,
            profile_preview_zoom: 1.0,
            profile_preview_started: Instant::now(),
            profile_preview_key: None,
            profile_defer_send: false,
            profile_userinfo_pending: false,
            last_profile_wire: None,
            console_buffer_running: false,
            console_userinfo_dirty: false,
            japro_userinfo_update_debounce: 0,
            // TaystJK's modification-count baselines start at -1, so the first
            // active jaPRO CGame frame publishes both mirror cvars once.
            japro_camera_userinfo_dirty: true,
            japro_net_userinfo_dirty: true,
            profile_force_pending: false,
            profile_dynamic_models: Arc::new(Vec::new()),
            profile_preview_active: false,
            render_studio: Default::default(),
            demo_entries: Vec::new(),
            demo_catalog_pending: false,
            demo_catalog_generation: 0,
            demo_selected: 0,
            demo_game_choices: Vec::new(),
            demo_game_filter: String::new(),
            demo_catalog_loaded: false,
            demo_catalog_error: None,
            demo_metadata_cache: BTreeMap::new(),
            demo_metadata_errors: BTreeMap::new(),
            demo_metadata_generation: 0,
            demo_metadata_inflight: HashSet::new(),
            demo_metadata_tx,
            demo_metadata_rx,
            race_ghost_load_generation: 0,
            race_ghost_tx,
            race_ghost_rx,
            race_ghost_alpha: presentation.race_ghost_alpha,
            race_ghost_name: presentation.race_ghost_name,
            race_ghost_trail: presentation.race_ghost_trail,
            race_ghost_velocity_delta: presentation.race_ghost_velocity_delta,
            race_ghost_distance_delta: presentation.race_ghost_distance_delta,
            race_ghost_demo_base_url: presentation.race_ghost_demo_base_url.clone(),
            race_ghost_demo_base_url_input: presentation.race_ghost_demo_base_url.clone(),
            race_ghost_web_generation: 0,
            race_ghost_web_tx,
            race_ghost_web_rx,
            race_ghost_web_courses: Vec::new(),
            race_ghost_web_demos: Vec::new(),
            race_ghost_web_selected_course: None,
            race_ghost_web_suggested_course: None,
            race_ghost_web_active_map: None,
            race_ghost_web_active_style: None,
            race_ghost_web_catalog_pending: false,
            race_ghost_web_demos_pending: false,
            race_ghost_web_pending_demos: HashSet::new(),
            race_ghost_web_error: None,
            ui_catalog,
            strafe_trails,
            strafe_trail_filter: String::new(),
            strafe_trail_name_input: String::new(),
            strafe_trail_log_input: presentation.strafe_trail.log_name.clone(),
            strafe_trail_slot: crate::strafe_trail::DEFAULT_SLOT,
            demo_console_filter: DemoConsoleFilter::All,
            demo_launch_seek_ms: None,
            demo_view_mode: DemoViewMode::Authoritative,
            demo_free_follow_anchor: None,
            demo_view_outside_authoritative: false,
            demo_free_speed_index: DEMO_FREE_SPEED_DEFAULT_INDEX,
            server_browser_tx,
            server_browser_rx,
            server_browser,
            game_session: None,
            water_wetness: crate::water_wetness::WetnessTracker::default(),
            siege_dialog: None,
            siege_round_message: None,
            siege_dialog_return_overlay: OverlayMode::None,
            siege_dialog_opened_overlay: false,
            live_without_world: false,
            demo_without_world: false,
            disconnect_notice: None,
            server_password_prompt: None,
            missing_map_prompt: None,
            demo_missing_map_prompt: None,
            live_missing_map_authorized: false,
            live_auto_download_requested: false,
            live_http_base: None,
            live_download: None,
            live_join_ui: None,
            net: None,
            live_cgame_prep_rx: None,
            live_cgame_prepared: None,
            pending_live_gamestate: false,
            rcon: None,
            pending_live_server_commands: VecDeque::new(),
            live_join_timing: None,
            network: presentation.network.clone(),
            saved_accounts,
            account_credential_error,
            account_editor: None,
            show_other_accounts: false,
            japro_cg: presentation.japro,
            camera_edit: None,
            vote_menu: egui_pages::VoteMenuState::default(),
            live_input: crate::net::LiveInput::default(),
            emote_net_rx: crate::emote_net::Receiver::default(),
            live_buttons: HashSet::new(),
            live_button_sources: HashMap::new(),
            predictor: crate::net::Predictor::default(),
            hitch: hitch::HitchRecorder::default(),
            reconnect_target: None,
            demo_recording: None,
            map_movement: None,
            center_print: None,
            last_center_refresh: Instant::now(),
            scoreboard: None,
            scores_showing: false,
            last_scores_request: None,
            last_command_at: None,
            next_command_time: None,
            live_provisional: None,
            stringed: None,
            startup_commands: Vec::new(),
            audio,
            menu_emote_sound: None,
            emote_wheel,
            emote_gallery: None,
            emote_gallery_request: None,
            emote_gallery_scene: Arc::new(Vec::new()),
            emote_gallery_menu_seen: None,
            window_focused: true,
            pending_frontend_map_launch: false,
            frontend_background_request_id: None,
            frontend_cinematic: None,
            frontend_scene_cache_path: None,
            frontend_scene_texture: None,
            frontend_scene_cache_generation: 0,
            frontend_stage: Default::default(),
            deferred_frontend_map: None,
            startup_first_menu_frame_seen: false,
            proxy,
            loader_tx,
            window: None,
            render: None,
            startup_first_frame_seen: false,
            camera: Camera::new_with_fov([0.0, 0.0, 0.0], 0.0, presentation.fov),
            render_view_latch: ViewLatchMode::Disabled,
            local_server: None,
            map_collision: None,
            map_mark_surfaces: None,
            map_static_models: Arc::default(),
            map_visibility: None,
            steam_audio_acoustic_mesh: None,
            steam_audio_bake: None,
            steam_audio_bake_progress: None,
            steam_audio_bake_error: None,
            map_physics_collision: PhysicsMapMesh::default(),
            third_person: presentation.third_person,
            spectator_camera: presentation.spectator_camera,
            spectator_camera_state: SpectatorCameraState::default(),
            first_person_lightsaber: presentation.first_person_lightsaber,
            saber_trail: presentation.saber_trail,
            saber_team_colors: presentation.saber_team_colors,
            saber_staff_multi_color: presentation.saber_staff_multi_color,
            team_overlay: presentation.team_overlay,
            score_deaths: presentation.score_deaths,
            score_spectator_guess: presentation.score_spectator_guess,
            spectator_ping_guesses: [Default::default(); 32],
            draw_scores: presentation.draw_scores,
            mini_scores_shown: None,
            team_overlay_shown: None,
            cg_debug_events: match super::cvar_registry().find("cg_debugEvents").unwrap().default_value() {
                crate::console::cvars::CvarValue::Int(value) => value as u8, _ => unreachable!(),
            },
            cg_event_workers: match super::cvar_registry().find("cg_eventWorkers").unwrap().default_value() {
                crate::console::cvars::CvarValue::Bool(value) => value, _ => unreachable!(),
            },
            crosshair: presentation.crosshair,
            crosshair_target: ui::UiCrosshairTarget::default(),
            crosshair_hit_world: None,
            crosshair_seen: None,
            crosshair_scan_at: Instant::now(),
            force_select_until: None,
            weapon_select_until: None,
            player_names: presentation.player_names,
            player_name_visible: [false; 32],
            race_ghost_label_visible: [false; 128],
            player_name_scan_at: Instant::now(),
            follow_name_shown: None,
            game_timer_shown: None,
            race_timer_shown: None,
            vote_line_shown: None,
            movement_keys_hud: presentation.movement_keys,
            strafe_helper: presentation.strafe_helper,
            hud_layout: presentation.hud_layout,
            hud_edit_selected: None,
            hud_edit_extension_selected: None,
            hud_edit_drag_origin: None,
            hud_edit_drag_delta: [0.0, 0.0],
            hud_edit_chat_resize_origin: None,
            hud_edit_chat_resize_corner: None,
            third_person_camera: ThirdPersonCameraState::default(),
            local_player_presenter: None,
            solo_dynamic_models: Arc::new(Vec::new()),
            solo_client_info: {
                let mut info = ClientInfo::solo_model(&presentation.model);
                info.female = presentation.network.sex.as_bytes().first()
                    .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'f'));
                info
            },
            menu_selected: 0,
            setup_selected: 1,
            keys: HashSet::new(),
            movement_keys: HashSet::new(),
            bindings,
            controls_selected: 0,
            controls_waiting_for_key: false,
            controls_section: 0,
            mouse_buttons_down: HashSet::new(),
            modifiers: ModifiersState::empty(),
            captured: false,
            mouse_grab_mode: "none",
            mouse_input: presentation.mouse,
            last_mouse_motion_at: None,
            noclip_primary_down: false,
            noclip_alt_down: false,
            mouse_input_sequence: 0,
            last_simulated_mouse_input: None,
            cursor_position: (0.0, 0.0),
            demo_scrub_dragging: false,
            demo_scrub_fraction: None,
            demo_scrub_resume_rate: 1.0,
            demo_scrub_last_seek: None,
            demo_scrub_last_applied_fraction: None,
            last_demo_timeline_refresh: Instant::now(),
            demo_export_in_ms: None,
            demo_export_out_ms: None,
            demo_export_preset: DemoVideoPreset::DEFAULT,
            demo_export_fps: DEMO_EXPORT_DEFAULT_FPS,
            demo_export_depth: false,
            demo_video_export: None,
            previous_tick: Instant::now(),
            overlay: OverlayMode::Game,
            overlay_before_console: OverlayMode::Game,
            dev_radial_return_overlay: OverlayMode::Game,
            dev_radial_offset: [0.0; 2],
            model_placement: Default::default(),
            emote_radial_category: None,
            emote_radial_search: Default::default(),
            console_input: String::new(),
            console_cursor: 0,
            console_status: if front_end {
                "MAIN MENU".into()
            } else {
                "STARTING".into()
            },
            console_lines: VecDeque::with_capacity(1024),
            console_line_kinds: VecDeque::with_capacity(1024),
            console_path_links: VecDeque::with_capacity(1024),
            console_timestamps: presentation.console_timestamps,
            console_notify_time: crate::app::cvar_registry().find("con_notifytime").expect("notify-time definition").entry.default.parse().expect("notify-time default"),
            console_notify_lines: presentation.console_notify_lines,
            console_notify_words: presentation.console_notify_words.clone(),
            console_notify_terms: crate::app::console_state::parse_console_notify_terms(
                &presentation.console_notify_words,
            ),
            console_notify_connect: presentation.console_notify_connect,
            console_notify_vote: presentation.console_notify_vote,
            console_notify_beep: presentation.console_notify_beep,
            console_suggest: presentation.console_suggest,
            chatbox_completion: presentation.chatbox_completion,
            clean_chatbox: presentation.clean_chatbox,
            streamer: presentation.streamer,
            streamer_preview_local: false,
            last_clean_chat_message: None,
            chat_log_enabled: presentation.chat_log,
            chat_log: crate::chat_log::ChatLog::spawn(),
            console_suggest_index: 0,
            console_suggest_picked: false,
            console_suggest_hidden: false,
            console_map_suggest_cache: std::cell::RefCell::new(None),
            console_arg_suggest_cache: std::cell::RefCell::new(None),
            console_file_catalog_loaded: false,
            console_files: crate::app::frontend::ConsoleFileCatalog::default(),
            console_help_hover: false,
            ui_vgs: presentation.ui_vgs,
            jump_height_shade: presentation.jump_height_shade,
            screen_shake: presentation.screen_shake,
            head_look: presentation.head_look,
            swim_tilt: presentation.swim_tilt,
            swim_turn_bank: presentation.swim_turn_bank,
            move_lean: presentation.move_lean,
            force_model: presentation.force_model.clone(),
            forced_player_models,
            japro_zoom_fov: presentation.zoom_fov,
            view_bob: presentation.view_bob,
            japro_zoom_transition_at: None,
            japro_fk_duration: presentation.fk_duration,
            japro_fk_first_jump_duration: presentation.fk_first_jump_duration,
            japro_fk_second_jump_delay: presentation.fk_second_jump_delay,
            japro_flipkick_frames: 0,
            japro_flipkick_jumps: 0,
            japro_flipkick_moveup: false,
            vgs_menu: crate::vgs::Menu::Main,
            log_rx,
            console_history: Vec::with_capacity(128),
            console_history_index: None,
            console_command_buffer: VecDeque::new(),
            console_wait_until: None,
            console_map_barrier: false,
            perf_sample: None,
            latched_console_cvars: BTreeMap::new(),
            console_scroll: 0,
            console_size: ConsoleSize::Normal,
            console_selection_anchor: None,
            console_selection_focus: None,
            console_selecting: false,
            console_last_click: None,
            console_click_count: 0,
            console_search_open: false,
            console_search_input: String::new(),
            console_search_matches: Vec::new(),
            console_search_index: None,
            chat_mode: ChatMode::Global,
            chat_input: String::new(),
            chat_player_completion: None,
            chat_lines: VecDeque::with_capacity(16),
            last_chat_refresh: Instant::now(),
            console_notify: VecDeque::with_capacity(64),
            last_console_notify_refresh: Instant::now(),
            video_selected: 0,
            video_section: VideoSection::Display,
            environment_selected: 0,
            sun_ray_until: None,
            sun_ray_sent: false,
            sun_ray_preview_mode: 0,
            sun_ray_sent_mode: 0,
            baked_sun_debug_view: 0,
            ocean_settings_open: false,
            authored_oceans: Vec::new(),
            map_authored_oceans: Vec::new(),
            authored_ocean_selected: 0,
            authored_ocean_preview: false,
            ocean_selected: 0,
            ocean_settings_target: -1,
            promoted_oceans: Vec::new(),
            promoted_ocean_overrides: Vec::new(),
            ocean_sky_extend: 2,
            coastal_debug_mode: 0,
            coastal_enabled: false,
            coastal_pool: 0,
            coastal_live_sent: None,
            coastal_debug_sent: None,
            cloud_tuning_open: false,
            cloud_tuning_selected: 0,
            video,
            frame_time_graph: false,
            world_effects: {
                let mut effects = crate::world_effects::LegacyWorldEffects::default();
                let _ = effects.set_weather_type(video.weather_type);
                effects
            },
            gamma_runtime: crate::app::gamma::GammaRuntime::default(),
            windows_timer_resolution,
            quality_preset_selected: None,
            quality_preset_values: Vec::new(),
            applied_fullscreen: video.fullscreen,
            applied_renderer_backend: video.renderer_backend,
            applied_gpu_adapter: video.gpu_adapter,
            active_gpu_name: "Initializing...".to_owned(),
            active_gpu_type: "Unknown".to_owned(),
            applied_hdr_output: video.hdr_output,
            hdr_output_status: "Initializing display output...".to_owned(),
            hdr_output_active: false,
            hdr_available_modes: 1,
            hdr_modes_known: false,
            applied_resolution: video.resolution,
            applied_grass: video.grass,
            applied_ocean: video.ocean,
            video_confirmation: None,
            pending_video_confirmation: None,
            pending_renderer_restart: None,
            retiring_render: None,
            window_hidden_for_restart: false,
            supported_msaa: vec![1],
            wireframe_supported: false,
            spawns: Vec::new(),
            spawn_index: 0,
            solo_initial_spawn_pending: false,
            settings_batch_open: false,
            ui_publish_pending: std::cell::Cell::new(false),
            strafe_ground: std::cell::Cell::default(),
            speedometer: crate::speedometer::Speedometer::default(),
            speedometer_ui: None,
            speedometer_shown: None,
            speedometer_clock: Instant::now(),
            speedometer_last_frame: None,
            lagometer_ui: None,
            lagometer_shown: None,
            triangles: 0,
            map_distance_cull: crate::camera::DEFAULT_DISTANCE_CULL,
            map_authored_sun: None,
            sun_custom_initialized,
            latest_request_id: 0,
            latest_prepare_options: scene::MapPrepareOptions {
                visual_only: false,
                patch_subdivisions_bits: video.patch_subdivisions.to_bits(),
                patch_stitching: video.patch_stitching,
                picmip: video.picmip,
                grass: video.grass,
                voxel_probe_gi: video.voxel_probe_gi,
                ocean: video.ocean,
                client_physics: video.client_physics,
                gen_normal_maps: video.gen_normal_maps,
                float_lightmap: video.float_lightmap && (video.hdr || video.hdr_output != 0),
                planar_reflections: video.reflection_quality.planar_slot_budget() > 0,
                planar_environment: video.reflection_quality.promotes_environment_planars(),
                omit_environment_stages: video.reflection_quality.omits_environment_stages(),
                source_spatial_batches: video.gpu_driven,
                pbr_materials: video.pbr,
                allow_asset_overrides: video.allow_asset_overrides,
                steam_audio: audio.steam_audio,
                menu_look: false,
                menu_props: false,
            },
            prepared_map_cache: None,
            fx_light_promotion_catalog: Arc::new(
                crate::fx::light_promotion::FxLightPromotionCatalog::default(),
            ),
            shader_remaps: scene::ShaderRemapState::default(),
            shader_remap_upload: false,
            shader_remap_dirty: false,
            shader_remap_ui: ShaderRemapUi::default(),
            physics_mesh_label: None,
            display_transition_until: None,
            loading: initial_loading,
            static_ao_progress: None,
            preserve_game_state_on_next_map_upload: false,
            renderer_restart_started: None,
            renderer_restart_suspended: false,
            perf: PerfStats::default(),
            threads: Arc::from([]),
            surface_inspector: None,
            trace_menu_entries: Vec::new(),
            trace_menu_selected: None,
            puddle_debug_mode: 0,
            puddle_sky_blend: (
                crate::weather::RAIN_SKY_BLEND_WINDOW,
                crate::weather::RAIN_SKY_BLEND_CEILING,
            ),
            dof_focus_target: 4096.0,
            config_path,
            config_dirty,
            background_jobs,
            background_result_tx,
            background_result_rx,
            config_save_generation: 0,
            last_config_write: Instant::now(),
            quit_requested: false,
            fps_cap_input,
            fps_cap_editing: false,
            fps_cap_replace_on_type: false,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            egui_last_frame: Instant::now(),
            egui_repaint_requested: true,
            egui_renderer_active: false,
            egui_apply_video_requested: false,
            companion: None,
            companion_target_enabled: false,
            companion_scene_target: None,
        };
        // A first run has no opinion yet, so start somewhere that runs well on
        // modest hardware instead of at the engine's uncurated field defaults.
        if fresh_install {
            app.apply_quality_preset(quality::QualityPreset::Low);
        }
        if app.front_end {
            app.open_ui_page(&[app.game_launch_screen.page_name()]);
        }
        Ok(app)
    }
}
