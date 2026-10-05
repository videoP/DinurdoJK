use super::*;

fn wait_for_player_model(
    presenter: &mut PlayerPresenter,
    info: &crate::cgame::ClientInfo,
) -> ModelResolution {
    let deadline = Instant::now() + std::time::Duration::from_secs(60);
    loop {
        presenter.poll_asset_completions();
        match presenter.resolve_player_model_async(info) {
            ModelResolution::Loading | ModelResolution::Provisional(_) => {
                assert!(
                    Instant::now() < deadline,
                    "player model did not finish loading"
                );
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            done => return done,
        }
    }
}

#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s"]
fn async_player_model_registration_never_blocks_and_deduplicates() {
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    let queued_after_prime = presenter.async_models.stats().queued;

    // Uncached model: the request returns without waiting for IO/parsing.
    let reborn = crate::cgame::ClientInfo::solo_model("reborn/default");
    let started = Instant::now();
    let first = presenter.resolve_player_model_async(&reborn);
    assert!(
        started.elapsed().as_millis() < 50,
        "request blocked the caller"
    );
    assert!(!matches!(first, ModelResolution::Ready(_)));
    for _ in 0..25 {
        presenter.resolve_player_model_async(&reborn);
    }
    let stats = presenter.async_models.stats();
    assert!(stats.duplicates_avoided >= 25, "{stats:?}");

    // The requested model swaps in when ready and was queued exactly once.
    let ModelResolution::Ready(model) = wait_for_player_model(&mut presenter, &reborn) else {
        panic!("reborn should load");
    };
    assert!(model.key.starts_with("models/players/reborn/"));
    let queued_for_reborn = presenter.async_models.stats().queued - queued_after_prime;
    assert!(
        queued_for_reborn <= 1,
        "reborn queued {queued_for_reborn} times"
    );

    // A second consumer resolves from the cache with no new job.
    let queued_before = presenter.async_models.stats().queued;
    assert!(matches!(
        presenter.resolve_player_model_async(&reborn),
        ModelResolution::Ready(_)
    ));
    assert_eq!(presenter.async_models.stats().queued, queued_before);

    // Missing model: terminal failure -> OpenJK fallback, never re-queued.
    let missing = crate::cgame::ClientInfo::solo_model("definitely_not_a_model/default");
    let ModelResolution::Ready(fallback) = wait_for_player_model(&mut presenter, &missing) else {
        panic!("missing model should fall back to the default body");
    };
    assert!(fallback.key.starts_with("models/players/kyle/"));
    let queued_before = presenter.async_models.stats().queued;
    for _ in 0..50 {
        presenter.resolve_player_model_async(&missing);
    }
    assert_eq!(presenter.async_models.stats().queued, queued_before);

    // Shared skeleton: every humanoid model resolves the same GLA Arc, so a
    // model swap can keep the animation state.
    assert!(Arc::ptr_eq(&model.gla, &fallback.gla));
}

fn tinted_test_shaders() -> BTreeMap<String, Shader> {
    jka_assets::shader::parse(
        "models/players/test/torso_04_clothes {
{
map models/players/test/torso_04
blendFunc GL_ONE GL_ZERO
rgbGen lightingDiffuseEntity
}
{
map models/players/test/torso_04
blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
detail
rgbGen lightingDiffuse
}
}
models/players/test/plain {
{
map models/players/test/plain
blendFunc GL_ONE GL_ZERO
rgbGen lightingDiffuse
}
{
map models/players/test/plain
blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
}
}
",
    )
    .unwrap()
}

#[test]
fn skin_shader_names_with_an_extension_resolve_the_entity_tint_layers() {
    let shaders = tinted_test_shaders();
    // The skin entry carries `.tga`; R_FindShader strips it.
    let layers = material_layers(&shaders, "models/players/test/torso_04_clothes.tga");
    assert_eq!(layers.base.image, "models/players/test/torso_04");
    // GL_ONE GL_ZERO replaces the framebuffer, so the texture alpha mask is not blended.
    assert_eq!(layers.base.alpha_mode, DynamicModelAlphaMode::Opaque);
    assert!(layers.entity_tint);
    let overlay = layers
        .overlay
        .expect("untinted alpha-blended overlay stage");
    assert_eq!(overlay.image, "models/players/test/torso_04");
    assert_eq!(overlay.alpha_mode, DynamicModelAlphaMode::Blend);
}

/// GlowTink (TinkerBell 1.0): a `GL_SRC_ALPHA GL_ONE` first stage driven by
/// `alphaGen lightingSpecular` must not be drawn at full alpha (it washed the
/// model white); the glow stages after it carry the look.
#[test]
fn blended_glow_shaders_draw_every_supported_stage() {
    let shaders = jka_assets::shader::parse(
        "models/players/GlowTink/tink3
{
    cull twosided
    {
        map models/players/GlowTink/tink3
        blendFunc GL_SRC_ALPHA GL_ONE
        alphaGen lightingSpecular
        tcMod scale 2.2 2.2
        tcMod scroll 0.0 -1.01
    }
    {
        map models/players/GlowTink/tink3_e
        blendFunc GL_ONE GL_ONE
        rgbGen const ( 1.4 1.2 0.5 )
        tcMod scale 2.2 2.2
        tcMod scroll 0.0 -1.01
    }
    {
        map models/players/GlowTink/tink3_cel
        blendFunc GL_DST_COLOR GL_ONE
        rgbGen const ( 1.0 0.9 0.2 )
        tcGen environment
    }
    {
        map models/players/GlowTink/tink3_e
        blendFunc GL_ONE GL_ONE
        rgbGen const ( 2.2 1.8 0.7 )
        tcMod scale 4.2 4.2
    }
}
models/players/GlowTink/black
{
    {
        map models/players/GlowTink/black
        blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
        rgbGen const ( 1.18 1.18 1.18 )
    }
    {
        map models/players/GlowTink/tink3_e
        blendFunc GL_ONE GL_ONE
        rgbGen const ( 1.2 1.0 0.45 )
    }
}
",
    )
    .expect("test shaders parse");
    let layers = material_layers(&shaders, "models/players/GlowTink/tink3.tga");
    let stages = layers
        .stages
        .iter()
        .map(|(stage, mode)| (stage.image.as_str(), *mode))
        .collect::<Vec<_>>();
    assert_eq!(
        stages,
        [
            (
                "models/players/glowtink/tink3",
                DynamicModelAlphaMode::Additive
            ),
            (
                "models/players/glowtink/tink3_e",
                DynamicModelAlphaMode::AdditiveOne
            ),
            (
                "models/players/glowtink/tink3_e",
                DynamicModelAlphaMode::AdditiveOne
            ),
        ],
        "the GL_DST_COLOR environment filter is not drawn"
    );
    assert!(resolved_stage(layers.stages[0].0, layers.stages[0].1, None).specular_alpha);
    let glow = resolved_stage(layers.stages[1].0, layers.stages[1].1, None);
    // The shader parser clamps rgbGen const to 0..1.
    assert_eq!(glow.rgb, [1.0, 1.0, 0.5]);
    assert!(glow.unlit);

    let layers = material_layers(&shaders, "models/players/GlowTink/black");
    let modes = layers
        .stages
        .iter()
        .map(|(_, mode)| *mode)
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        [
            DynamicModelAlphaMode::BlendUnlit,
            DynamicModelAlphaMode::AdditiveOne
        ]
    );
}

#[test]
fn specular_alpha_peaks_on_the_mirror_direction_and_clamps_backfacing_to_zero() {
    // JKA (0,0,1) is render (0,1,0). Light straight above, viewer straight above:
    // the reflection of the light is the viewer direction.
    let (position, up) = ([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
    let alpha = specular_alpha(position, up, [0.0, 0.0, 100.0], [0.0, 0.0, 100.0]);
    assert!((alpha - 1.0).abs() < 1.0e-5, "{alpha}");
    // A viewer below the surface sees no glint.
    assert_eq!(
        specular_alpha(position, up, [0.0, 0.0, 100.0], [0.0, 0.0, -100.0]),
        0.0
    );
}

#[test]
fn stage_tc_mods_compose_in_order_and_scroll_wraps() {
    let mods = [TcMod::Scale(2.0, 2.0), TcMod::Scroll(0.0, -0.5)];
    assert_eq!(stage_uv_xform(&mods, 0.0), [2.0, 2.0, 0.0, 0.0]);
    // A quarter second scrolls -0.125, wrapped into [0, 1).
    assert_eq!(stage_uv_xform(&mods, 0.25), [2.0, 2.0, 0.0, 0.875]);
    // Scaling after a scroll scales the offset with it.
    let mods = [TcMod::Scroll(0.25, 0.0), TcMod::Scale(4.0, 1.0)];
    assert_eq!(stage_uv_xform(&mods, 1.0 / 8.0), [4.0, 1.0, 0.125, 0.0]);
}

#[test]
fn opaque_base_player_shaders_keep_the_single_stage_path() {
    let shaders = jka_assets::shader::parse(
        "models/players/TinkerBell/newer
{
    cull disable
    {
        map models/players/TinkerBell/newer
        alphaFunc GE128
        rgbGen lightingDiffuse
    }
    {
        map models/players/TinkerBell/newer_s
        blendFunc GL_ONE GL_ONE
        rgbGen identity
        alphaGen lightingSpecular
    }
}
",
    )
    .expect("test shaders parse");
    let layers = material_layers(&shaders, "models/players/TinkerBell/newer");
    assert!(layers.stages.is_empty());
    assert_eq!(layers.base.alpha_mode, DynamicModelAlphaMode::Mask);
}

#[test]
fn shaders_without_an_entity_colour_stage_stay_single_pass() {
    let shaders = tinted_test_shaders();
    let layers = material_layers(&shaders, "models/players/test/plain");
    assert!(!layers.entity_tint);
    assert!(layers.overlay.is_none());
    // A name with no shader is a plain texture path.
    let layers = material_layers(&shaders, "models/players/test/kyle.tga");
    assert_eq!(layers.base.image, "models/players/test/kyle.tga");
    assert!(!layers.entity_tint);
}

#[test]
fn custom_rgba_tints_players_and_missing_state_is_untinted() {
    assert_eq!(custom_rgba_tint([0; 4]), [1.0; 3]);
    assert_eq!(custom_rgba_tint([255, 255, 255, 255]), [1.0; 3]);
    assert_eq!(custom_rgba_tint([255, 0, 51, 255]), [1.0, 0.0, 0.2]);
    // A genuine black tint carries alpha 255 and must not read as "unset".
    assert_eq!(custom_rgba_tint([0, 0, 0, 255]), [0.0; 3]);
}

#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s"]
fn jedi_zf_torso_registers_with_entity_tint_and_overlay() {
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    let info = crate::cgame::ClientInfo::solo_model("jedi_zf/default");
    let model = presenter.load_model(&info).unwrap();
    let tinted: Vec<_> = model
        .surfaces
        .iter()
        .filter(|surface| surface.entity_tint)
        .collect();
    assert!(
        !tinted.is_empty(),
        "jedi_zf default skin has an lightingDiffuseEntity surface"
    );
    for surface in tinted {
        assert!(
            surface.texture.is_some(),
            "tinted surface lost its texture (white torso)"
        );
        assert_eq!(surface.alpha_mode, DynamicModelAlphaMode::Opaque);
        assert!(surface
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.texture.is_some()));
    }
    assert!(model
        .surfaces
        .iter()
        .all(|surface| surface.texture.is_some()));
}

/// ShadowTink (TinkerBell 1.0): an opaque `rgbGen const` base under scrolling
/// marble/smoke layers, a lit opaque eye under a `glow` map, and twosided
/// additive wings. Each was drawn as its primary stage only.
#[test]
fn opaque_base_shaders_with_overlay_stages_draw_the_whole_stack() {
    let shaders = jka_assets::shader::parse(
        "models/players/ShadowTink/legs
{
    cull twosided
    {
        map models/players/ShadowTink/legs
        blendFunc GL_ONE GL_ZERO
        rgbGen const ( 0.2 0.0 0.4 )
    }
    {
        map models/players/ShadowTink/tink43
        blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
        rgbGen const ( 0.6 0.6 0.6 )
        alphaGen const 0.45
        tcMod scale 2.2 2.2
    }
}
models/players/ShadowTink/ShadowEyes
{
    {
        map models/players/ShadowTink/ShadowEyes
        blendFunc GL_ONE GL_ZERO
        rgbGen lightingDiffuse
    }
    {
        map models/players/ShadowTink/ShadowEyes_g
        blendFunc GL_ONE GL_ONE
        detail
        glow
    }
}
models/players/ShadowTink/ShadowWings
{
    cull twosided
    {
        map models/players/ShadowTink/ShadowWings
        blendFunc GL_ONE GL_ONE
        rgbGen wave sin 2.5 2.5 0 0
    }
}
models/players/test/plain_opaque
{
    {
        map models/players/test/plain_opaque
        rgbGen lightingDiffuse
    }
}
",
    )
    .expect("test shaders parse");

    let legs = material_layers(&shaders, "models/players/ShadowTink/legs.tga");
    let modes = legs
        .stages
        .iter()
        .map(|(_, mode)| *mode)
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        [
            DynamicModelAlphaMode::Opaque,
            DynamicModelAlphaMode::BlendUnlit
        ]
    );
    let base = resolved_stage(legs.stages[0].0, legs.stages[0].1, None);
    assert_eq!(base.rgb, [0.2, 0.0, 0.4]);
    assert!(base.unlit, "rgbGen const is not lit by the light grid");
    assert!(!legs.two_sided, "an opaque body hides its own back faces");

    let eyes = material_layers(&shaders, "models/players/ShadowTink/ShadowEyes");
    let modes = eyes
        .stages
        .iter()
        .map(|(_, mode)| *mode)
        .collect::<Vec<_>>();
    assert_eq!(
        modes,
        [
            DynamicModelAlphaMode::Opaque,
            DynamicModelAlphaMode::AdditiveOne
        ]
    );
    assert!(!resolved_stage(eyes.stages[0].0, eyes.stages[0].1, None).unlit);

    let wings = material_layers(&shaders, "models/players/ShadowTink/ShadowWings");
    assert!(
        wings.two_sided,
        "twosided additive wings need reversed triangles"
    );

    let plain = material_layers(&shaders, "models/players/test/plain_opaque");
    assert!(plain.stages.is_empty() && !plain.two_sided);
}

#[test]
fn body_queue_weapon_rule_copies_actual_model_then_applies_known_weapon() {
    // CG_BodyQueueCopy never manufactures model index 1 when the source
    // Ghoul2 did not actually have one.
    assert_eq!(body_queue_model1_weapon(None, WP_SABER), None);
    // A normal saber death keeps model index 1 occupied, but OpenJK's
    // ET_BODY replacement resolves WP_SABER through the default weapon
    // instance rather than the live client's custom primary saber.
    assert_eq!(
        body_queue_model1_weapon(Some(WP_SABER), WP_SABER),
        Some(WP_SABER)
    );
    // Weapons newer than Bryar are stripped from model index 1. Their
    // separately dropped ET_ITEM, when the server creates one, owns them.
    assert_eq!(
        body_queue_model1_weapon(Some(WP_BRYAR_PISTOL + 1), WP_BRYAR_PISTOL + 1),
        None
    );
    // Low-tier known weapons replace an already-existing duplicated slot.
    assert_eq!(
        body_queue_model1_weapon(Some(WP_SABER), WP_BRYAR_PISTOL),
        Some(WP_BRYAR_PISTOL),
    );
}

#[test]
fn openjk_dead_saber_length_retracts_toward_zero() {
    let mut blade = SaberBladeLengthState::new(40.0, -1.0, 1000);
    assert_eq!(blade.length, 40.0);
    let first = blade.set_desired_and_update(0.0, 40.0, 1016);
    assert!(first < 40.0 && first > 0.0);
    let later = blade.set_desired_and_update(0.0, 40.0, 1100);
    assert!(later < first);
}

#[test]
fn zero_length_saber_request_reaches_weapon_fx_cleanup() {
    let request = saber_blade_fx_request(
        [0.0; 3],
        [1.0, 0.0, 0.0],
        0.0,
        40.0,
        3.0,
        4,
        1.0,
        7,
        0,
        0,
        0,
        0,
        false,
        0,
        1,
        false,
        false,
    )
    .expect("zero-length terminal sample must reach WeaponFx cleanup");
    let PlayerFxRequest::SaberBlade { length, .. } = request else {
        panic!("expected saber blade request");
    };
    assert_eq!(length, 0.0);
}

#[test]
fn force_grip_trace_hits_default_player_box() {
    let start = Vec3::new(0.0, 0.0, DEFAULT_VIEWHEIGHT);
    let forward = Vec3::X;
    let hit = ray_default_player_box_entry(start, forward, [128.0, 0.0, 0.0], MAX_GRIP_DISTANCE);
    assert!(hit.is_some());
    assert!((hit.unwrap() - 113.0).abs() < 1.0e-4);
}

#[test]
fn force_grip_trace_misses_off_axis_player_box() {
    let start = Vec3::new(0.0, 0.0, DEFAULT_VIEWHEIGHT);
    let forward = Vec3::X;
    assert!(
        ray_default_player_box_entry(start, forward, [128.0, 80.0, 0.0], MAX_GRIP_DISTANCE,)
            .is_none()
    );
}

#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26"]
fn demo_saber_throw_rotates_returns_and_catches() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = assets
        .read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .expect("regression demo")
        .bytes;
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    presenter.set_async_loading(false);
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let mut directions: HashMap<u16, [f32; 3]> = HashMap::new();
    let mut spinning = false;
    let mut returning = false;
    let mut caught = false;
    let mut frames = 0;
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    game.set_initial_snapshot(snapshot).unwrap();
                    let mut entities = game.present_entities(snapshot.server_time).unwrap();
                    if let Some(followed) = game.present_followed_player(snapshot.server_time) {
                        entities.push(followed);
                    }
                    for owner in entities
                        .iter()
                        .filter(|entity| entity.entity_type == ET_PLAYER)
                    {
                        let client = owner.state.field_i32("clientNum").unwrap() as usize;
                        let Some(info) = game.client_info(client, &[]) else {
                            continue;
                        };
                        let was_flying = presenter.thrown_sabers.contains_key(&owner.number);
                        let draws = presenter
                            .present_thrown_saber_for_player(
                                owner,
                                &info,
                                &entities,
                                &game,
                                snapshot.server_time,
                                1.0,
                            )
                            .unwrap();
                        if owner.state.field_i32("saberInFlight") == Some(0) {
                            if was_flying {
                                assert!(draws.is_empty());
                                assert!(!presenter.thrown_sabers.contains_key(&owner.number));
                                caught = true;
                            }
                            continue;
                        }
                        if draws.is_empty() {
                            continue;
                        }
                        frames += 1;
                        let saber_num = owner.state.field_i32("saberEntityNum").unwrap() as u16;
                        let saber = entities
                            .iter()
                            .find(|entity| entity.number == saber_num)
                            .unwrap();
                        assert_eq!(
                            saber.state.field_i32("apos.trType"),
                            Some(super::super::TR_LINEAR)
                        );
                        let def = presenter
                            .saber_definitions
                            .definition_or_default(&info.saber_name);
                        let angles = presenter
                            .thrown_sabers
                            .get_mut(&owner.number)
                            .unwrap()
                            .angles(
                                owner,
                                saber,
                                snapshot.server_time,
                                snapshot.message_num,
                                def.return_damage,
                            )
                            .unwrap();
                        if saber.state.field_i32("saberInFlight") == Some(1) && angles[0] == 90.0 {
                            let hilt = presenter.load_saber_model(&def).unwrap();
                            let pose = Ghoul2Animator::new(&hilt.gla)
                                .evaluate_pose_openjk_root(&hilt.gla, snapshot.server_time)
                                .unwrap();
                            let bolt = model_bolt_matrix(&hilt.glm, &hilt.gla, &pose, "*blade1")
                                .unwrap()
                                .expect("staff blade bolt");
                            let dir = normalize_vec3(transform_jka_model_vector(
                                [-bolt[0][0], -bolt[1][0], -bolt[2][0]],
                                angles_to_axis(blade_angles(angles)),
                            ));
                            assert!(
                                dir[2].abs() < 0.05,
                                "settled blade must be horizontal: {dir:?}"
                            );
                            if let Some(previous) = directions.insert(owner.number, dir) {
                                let dot: f32 = previous.iter().zip(dir).map(|(a, b)| a * b).sum();
                                spinning |= dot < 0.99;
                            }
                        } else if saber.state.field_i32("saberInFlight") == Some(0) {
                            let mut expected = openjk_vectoangles([
                                saber.origin[0] - owner.origin[0],
                                saber.origin[1] - owner.origin[1],
                                saber.origin[2] - owner.origin[2],
                            ]);
                            expected[0] += 90.0;
                            if was_flying
                                && !def.return_damage
                                && saber.state.field_i32("bolt2") != Some(123)
                            {
                                for axis in 0..3 {
                                    assert!((angles[axis] - expected[axis]).abs() < 0.001);
                                }
                                returning = true;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if spinning && returning && caught {
            break;
        }
    }
    println!("SABER REGRESSION frames={frames} spinning={spinning} returning={returning} caught={caught}");
    assert!(spinning && returning && caught);
}

/// sabers.dm_26: a staff (`dual_3`) thrown in single-blade mode
/// (saberHolstered == 1) must fly with only its first blade, and a second
/// saber selected at runtime (`saber kyle kyle`) must be attached in hand as
/// soon as the userinfo changes, not only after the primary is thrown and
/// caught.
#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/sabers.dm_26"]
fn demo_runtime_saber_changes_and_half_staff_throw() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = assets
        .read("demos/sabers.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .expect("regression demo")
        .bytes;
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    presenter.set_async_loading(false);
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let mut snapshots = 0;
    let (mut half_staff_throw_frames, mut dual_frames) = (0, 0);
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    if snapshots == 0 {
                        game.set_initial_snapshot(snapshot).unwrap();
                    } else {
                        game.set_next_snapshot(Some(snapshot)).unwrap();
                        game.transition_snapshot(snapshot.server_time).unwrap();
                    }
                    snapshots += 1;
                    let mut entities = game.present_entities(snapshot.server_time).unwrap();
                    let Some(followed) = game.present_followed_player(snapshot.server_time) else {
                        continue;
                    };
                    entities.push(followed.clone());
                    let client = followed.state.field_i32("clientNum").unwrap_or(0) as usize;
                    let Some(info) = game.client_info(client, &[]) else {
                        continue;
                    };
                    presenter.present_snapshot_players(
                        &entities,
                        &game,
                        &[],
                        snapshot.server_time,
                        None,
                        None,
                        None,
                        None,
                    );
                    presenter.drain_fx_requests();
                    let in_flight = followed.state.field_i32("saberInFlight").unwrap_or(0) != 0;
                    let holstered = followed.state.field_i32("saberHolstered").unwrap_or(0);
                    let weapon = followed.state.field_i32("weapon").unwrap_or(0);
                    let [_, has_secondary] = presenter
                        .saber_definitions
                        .equipped_slots([&info.saber_name, &info.saber2_name], true);
                    if weapon == WP_SABER && has_secondary && !in_flight {
                        dual_frames += 1;
                        assert!(
                            presenter.entities[&followed.number].secondary_saber_attached,
                            "second saber '{}' not attached at {}",
                            info.saber2_name, snapshot.server_time,
                        );
                    }
                    if in_flight && holstered == 1 && info.saber_name.eq_ignore_ascii_case("dual_3")
                    {
                        presenter
                            .present_thrown_saber_for_player(
                                &followed,
                                &info,
                                &entities,
                                &game,
                                snapshot.server_time,
                                1.0,
                            )
                            .unwrap();
                        let blades: Vec<u8> = presenter
                            .drain_fx_requests()
                            .into_iter()
                            .filter_map(|request| match request {
                                PlayerFxRequest::SaberBlade {
                                    blade_num,
                                    saber_in_flight: true,
                                    ..
                                } => Some(blade_num),
                                _ => None,
                            })
                            .collect();
                        if !blades.is_empty() {
                            half_staff_throw_frames += 1;
                            assert!(
                                blades.iter().all(|&blade| blade == 0),
                                "extra blades drawn on a half-staff throw: {blades:?}"
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    println!("SABER SETUP REGRESSION snapshots={snapshots} halfStaffThrowFrames={half_staff_throw_frames} dualFrames={dual_frames}");
    assert!(
        half_staff_throw_frames > 0,
        "demo no longer throws a half staff"
    );
    assert!(dual_frames > 0, "demo no longer wields dual sabers");
}

/// Reborn NPCs held by a spawner are parked EF_NODRAW with anim 0
/// (FACE_TALK0, a reference-pose frame); CG_Player must not draw them.
#[test]
#[ignore = "requires JKA_TEST_BASE and demos/TEST.dm_26"]
fn demo_nodraw_npcs_are_not_submitted_in_reference_pose() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = assets
        .read("demos/TEST.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .unwrap()
        .bytes;
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    presenter.set_async_loading(false);
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let (mut snapshots, mut nodraw_frames) = (0, 0);
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    if snapshots == 0 {
                        game.set_initial_snapshot(snapshot).unwrap();
                    } else {
                        game.set_next_snapshot(Some(snapshot)).unwrap();
                        game.transition_snapshot(snapshot.server_time).unwrap();
                    }
                    snapshots += 1;
                    let entities = game.present_entities(snapshot.server_time).unwrap();
                    let draws = presenter.present_snapshot_players(
                        &entities,
                        &game,
                        &[],
                        snapshot.server_time,
                        None,
                        None,
                        None,
                        None,
                    );
                    for npc in entities
                        .iter()
                        .filter(|entity| entity.entity_type == ET_NPC)
                    {
                        let drawn = draws.iter().any(|surface| surface.entity_num == npc.number);
                        let nodraw = npc.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0;
                        if nodraw {
                            nodraw_frames += 1;
                            assert!(
                                !drawn,
                                "EF_NODRAW NPC {} submitted at {}",
                                npc.number, snapshot.server_time
                            );
                        } else {
                            assert_ne!(
                                npc.state.field_i32("legsAnim").unwrap_or(0) & !0x800,
                                0,
                                "visible NPC {} in anim 0",
                                npc.number
                            );
                        }
                    }
                }
                _ => {}
            }
        }
    }
    println!("NODRAW REGRESSION snapshots={snapshots} nodrawNpcFrames={nodraw_frames}");
    assert!(
        nodraw_frames > 0,
        "demo no longer exercises spawner-held NPCs"
    );
}

/// The TEST.dm_26 recorder switches between the saber and blaster,
/// repeater, rocket launcher, thermal and concussion rifle: each non-saber
/// weapon's world GLM must be bolted to *r_hand and submitted.
#[test]
#[ignore = "requires JKA_TEST_BASE and demos/TEST.dm_26"]
fn demo_followed_player_holds_non_saber_weapons() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = assets
        .read("demos/TEST.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .unwrap()
        .bytes;
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    presenter.set_async_loading(false);
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let mut snapshots = 0;
    let mut held: std::collections::BTreeMap<i32, usize> = Default::default();
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    if snapshots == 0 {
                        game.set_initial_snapshot(snapshot).unwrap();
                    } else {
                        game.set_next_snapshot(Some(snapshot)).unwrap();
                        game.transition_snapshot(snapshot.server_time).unwrap();
                    }
                    snapshots += 1;
                    let Some(followed) = game.present_followed_player(snapshot.server_time) else {
                        continue;
                    };
                    let client = followed.state.field_i32("clientNum").unwrap_or(0) as usize;
                    let Some(info) = game.client_info(client, &[]) else {
                        continue;
                    };
                    let draws = presenter
                        .present_player_entity(
                            &followed,
                            &info,
                            snapshot.server_time,
                            1.0,
                            None,
                            false,
                            true,
                            false,
                            None,
                        )
                        .unwrap();
                    let weapon = followed.state.field_i32("weapon").unwrap_or(0);
                    let attached = presenter.entities[&followed.number].attached_weapon;
                    if weapon != 0 && weapon != WP_SABER && weapon != WP_MELEE {
                        assert_eq!(
                            attached,
                            Some(weapon),
                            "weapon {weapon} not bolted at {}",
                            snapshot.server_time
                        );
                        let body = presenter.models
                            [&presenter.entities[&followed.number].model_key]
                            .surfaces
                            .len();
                        assert!(draws.len() > body, "weapon {weapon} surfaces not submitted");
                        *held.entry(weapon).or_insert(0) += 1;
                    }
                }
                _ => {}
            }
        }
    }
    println!("HELD WEAPON REGRESSION snapshots={snapshots} held={held:?}");
    assert!(
        held.len() >= 3,
        "demo exercised too few held weapons: {held:?}"
    );
}

/// Force powers in TEST.dm_26: CG_Player's hand-bolt effects, puffs and
/// shells must reach the FX system and produce draws.
#[test]
#[ignore = "requires JKA_TEST_BASE and demos/TEST.dm_26"]
fn demo_force_powers_produce_fx() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let open = || AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = open()
        .read("demos/TEST.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .unwrap()
        .bytes;
    let mut presenter = PlayerPresenter::new(open(), false).unwrap();
    presenter.set_async_loading(false);
    let mut weapon_fx = crate::cgame::weapon_fx::WeaponFx::new(open());
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let mut snapshots = 0;
    let mut requests: std::collections::BTreeMap<String, usize> = Default::default();
    let mut state_counts: std::collections::BTreeMap<&str, usize> = Default::default();
    let (mut fx_draw_frames, mut shell_surfaces) = (0, 0);
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    if snapshots == 0 {
                        game.set_initial_snapshot(snapshot).unwrap();
                    } else {
                        game.set_next_snapshot(Some(snapshot)).unwrap();
                        game.transition_snapshot(snapshot.server_time).unwrap();
                    }
                    snapshots += 1;
                    weapon_fx.begin_frame(snapshot.server_time);
                    let mut entities = game.present_entities(snapshot.server_time).unwrap();
                    if let Some(followed) = game.present_followed_player(snapshot.server_time) {
                        entities.push(followed);
                    }
                    for entity in entities
                        .iter()
                        .filter(|e| e.entity_type == ET_PLAYER || e.entity_type == ET_NPC)
                    {
                        let state = &entity.state;
                        if state.field_i32("activeForcePass").unwrap_or(0) != 0 {
                            *state_counts.entry("activeForcePass").or_insert(0) += 1;
                        }
                        if state.field_i32("powerups").unwrap_or(0) & (1 << PW_DISINT_4) != 0 {
                            *state_counts.entry("PW_DISINT_4").or_insert(0) += 1;
                        }
                        if state.field_i32("eFlags").unwrap_or(0) & EF_BODYPUSH != 0 {
                            *state_counts.entry("EF_BODYPUSH").or_insert(0) += 1;
                        }
                        let active = state.field_i32("forcePowersActive").unwrap_or(0);
                        if active & (1 << FP_PROTECT) != 0 {
                            *state_counts.entry("FP_PROTECT").or_insert(0) += 1;
                        }
                        if active & (1 << FP_RAGE) != 0 {
                            *state_counts.entry("FP_RAGE").or_insert(0) += 1;
                        }
                        if active & (1 << FP_GRIP) != 0 {
                            *state_counts.entry("FP_GRIP").or_insert(0) += 1;
                        }
                    }
                    let draws = presenter.present_snapshot_players(
                        &entities,
                        &game,
                        &[],
                        snapshot.server_time,
                        None,
                        None,
                        None,
                        None,
                    );
                    shell_surfaces += draws
                        .iter()
                        .filter(|surface| {
                            surface.alpha_mode == DynamicModelAlphaMode::Additive
                                && surface.vertices.len() > 50
                        })
                        .count();
                    for request in presenter.drain_fx_requests() {
                        let key = match &request {
                            PlayerFxRequest::Effect { name, .. } => name.to_string(),
                            PlayerFxRequest::EffectDir { name, .. } => name.clone(),
                            PlayerFxRequest::SaberBlade { .. } => "saber blade".into(),
                            PlayerFxRequest::PushPuffs { .. } => "push puffs".into(),
                            PlayerFxRequest::GripPuffs { .. } => "grip puffs".into(),
                            PlayerFxRequest::HeadSprite { shader, .. } => {
                                format!("head sprite {shader}")
                            }
                        };
                        *requests.entry(key).or_insert(0) += 1;
                        weapon_fx.player_fx(&request);
                    }
                    if !weapon_fx.end_frame().draws.is_empty() {
                        fx_draw_frames += 1;
                    }
                }
                _ => {}
            }
        }
    }
    println!("FORCE REGRESSION snapshots={snapshots} states={state_counts:?} requests={requests:?} fxDrawFrames={fx_draw_frames} shellSurfaces={shell_surfaces} fx={:?}", weapon_fx.stats());
    assert!(
        !requests.is_empty(),
        "no force power visuals were requested"
    );
    assert!(fx_draw_frames > 0);
}

/// TEST.dm_26 (mp/ffa3) is full of saber-wielding reborn NPCs: they must
/// resolve an npcClient, pose through CG_Player and submit body + blades.
#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/TEST.dm_26"]
fn demo_reborn_npcs_submit_body_and_saber_geometry() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = assets
        .read("demos/TEST.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .expect("TEST demo")
        .bytes;
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    presenter.set_async_loading(false);
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let mut npc_frames = 0;
    let mut npcs_with_body = HashSet::new();
    let mut npcs_with_blade = HashSet::new();
    let mut infos = Vec::new();
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    game.set_initial_snapshot(snapshot).unwrap();
                    let entities = game.present_entities(snapshot.server_time).unwrap();
                    let npcs = entities
                        .iter()
                        .filter(|entity| entity.entity_type == ET_NPC)
                        .collect::<Vec<_>>();
                    if npcs.is_empty() {
                        continue;
                    }
                    npc_frames += 1;
                    let draws = presenter.present_snapshot_players(
                        &entities,
                        &game,
                        &[],
                        snapshot.server_time,
                        None,
                        None,
                        None,
                        None,
                    );
                    for npc in npcs {
                        if infos.len() < 3 {
                            let info = game.npc_client_info(&npc.state).unwrap();
                            infos.push(format!(
                                "{}/{} sabers={}+{} colors={}/{} def={} bolt={:?}",
                                info.model_name,
                                info.skin_name,
                                info.saber_name,
                                info.saber2_name,
                                info.saber_color,
                                info.saber2_color,
                                info.definition_saber_colors,
                                npc.state.field_i32("boltToPlayer")
                            ));
                        }
                        let own = draws
                            .iter()
                            .filter(|surface| surface.entity_num == npc.number)
                            .count();
                        if own >= 10 {
                            npcs_with_body.insert(npc.number);
                        }
                        if presenter
                            .reported_saber_diagnostics
                            .contains(&format!("held:{}:0:0:submitted", npc.number))
                        {
                            npcs_with_blade.insert(npc.number);
                        }
                    }
                }
                _ => {}
            }
        }
        if npc_frames >= 400 {
            break;
        }
    }
    println!(
        "NPC REGRESSION frames={npc_frames} body={} blade={} infos={infos:?}",
        npcs_with_body.len(),
        npcs_with_blade.len()
    );
    assert!(npc_frames > 0);
    assert!(
        !npcs_with_body.is_empty(),
        "no reborn NPC submitted body geometry"
    );
    assert!(
        !npcs_with_blade.is_empty(),
        "no reborn NPC submitted a saber blade"
    );
}

/// Runs the actual demo -> clientinfo -> model/pose -> CPU geometry path.
/// Requires user-owned assets; no window or GPU is needed.
#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26"]
fn demo_missing_opponent_submits_fallback_geometry() {
    use jka_protocol::{
        demo::DemoReader,
        server::{Decoder, Event},
    };
    let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
    let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
    let bytes = assets
        .read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .expect("regression demo")
        .bytes;
    let mut presenter = PlayerPresenter::new(assets, false).unwrap();
    presenter.set_async_loading(false);
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let mut game = ClientGameState::new();
    let mut verified = false;
    while let Some(record) = reader.next_record().unwrap() {
        let packet = decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        for event in packet.events {
            match event {
                Event::Gamestate {
                    server_command_sequence,
                    ..
                } => {
                    game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                }
                Event::ServerCommand(command) => game.queue_server_command(command),
                Event::Snapshot { .. } => {
                    let snapshot = decoder.latest_snapshot().unwrap();
                    game.set_initial_snapshot(snapshot).unwrap();
                    let entities = game.present_entities(snapshot.server_time).unwrap();
                    let Some(opponent) = entities
                        .iter()
                        .find(|entity| entity.number == 2 && entity.entity_type == ET_PLAYER)
                    else {
                        continue;
                    };
                    let client_num = opponent.state.field_i32("clientNum").unwrap() as usize;
                    let info = game.client_info(client_num, &[]).unwrap();
                    if presenter.try_load_model(&info).is_ok() {
                        continue;
                    }
                    println!("REGRESSION opponent candidate: entity=2 model={} duelIndex={:?} duelInProgress={:?}",
                            info.model_cvar(), snapshot.player_state.field_i32("duelIndex"),
                            snapshot.player_state.field_i32("duelInProgress"));
                    assert_eq!(snapshot.player_state.field_i32("duelIndex"), Some(2));
                    assert_eq!(snapshot.player_state.field_i32("duelInProgress"), Some(1));
                    let fallback = presenter.load_model(&info).unwrap();
                    assert!(fallback.key.starts_with("models/players/kyle/model.glm|"));
                    let again = presenter.load_model(&info).unwrap();
                    assert!(
                        Arc::ptr_eq(&fallback, &again),
                        "fallback assets must be reused"
                    );
                    let draws = presenter.present_snapshot_players(
                        &entities,
                        &game,
                        &[],
                        snapshot.server_time,
                        None,
                        None,
                        None,
                        None,
                    );
                    assert!(draws.iter().any(|surface| surface.entity_num == 2
                        && !surface.vertices.is_empty()
                        && !surface.indices.is_empty()));
                    assert!(
                        presenter.player_diagnostics[&2].contains("bodySurfaces=19 submitted=1")
                    );
                    // Configstring changes must select new assets, not a stale fallback alias.
                    let jan = crate::cgame::ClientInfo::solo_model("jan");
                    assert!(presenter
                        .load_model(&jan)
                        .unwrap()
                        .key
                        .starts_with("models/players/jan/"));
                    verified = true;
                    break;
                }
                _ => {}
            }
        }
        if verified {
            break;
        }
    }
    assert!(
        verified,
        "demo did not exercise missing entity-2 model fallback"
    );
}

#[test]
fn weapon_attachment_follows_cg_player_copy_rules() {
    let mut instance = None;
    let mut cent_weapon = 0;
    let mut attached = None;
    let mut saber1 = false;
    let mut saber2 = false;

    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        5,
        Some(5),
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(
        (instance, cent_weapon, attached, saber1, saber2),
        (Some(5), 5, Some(5), false, false)
    );

    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        0,
        None,
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(
        attached,
        Some(5),
        "a NULL G2 weapon instance does not remove the old model"
    );
    assert_eq!(cent_weapon, 0);

    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_MELEE,
        Some(WP_MELEE),
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!((attached, saber1, saber2), (None, false, false));

    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        11,
        Some(11),
        false,
        false,
        false,
        true,
        false,
    );
    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        Some(WP_SABER),
        true,
        true,
        false,
        true,
        true,
    );
    assert_eq!(instance, None, "dead CG_Player clears only ghoul2weapon");
    assert_eq!(cent_weapon, 11, "dead CG_Player preserves cent->weapon");
    assert_eq!((attached, saber1, saber2), (Some(11), false, false));

    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        Some(WP_SABER),
        false,
        false,
        false,
        true,
        true,
    );
    assert_eq!(
        (cent_weapon, attached, saber1, saber2),
        (WP_SABER, None, true, true)
    );
    assert!(weapon_world_model(8)
        .unwrap()
        .ends_with("heavy_repeater_w.glm"));
    assert!(weapon_world_model(0).is_none());
}

#[test]
fn reset_player_entity_copies_both_dual_sabers_before_primary_flight_removal() {
    let mut instance = None;
    let mut cent_weapon = 0;
    let mut attached = None;
    let mut saber1 = false;
    let mut saber2 = false;

    // CG_SetInitialSnapshot -> CG_ResetEntity -> CG_ResetPlayerEntity.
    // CG_CopyG2WeaponInstance(WP_SABER) copies *both* configured saber
    // models before CG_Player sees that saber 0 is already in flight.
    reset_remote_player_saber_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        true,
        true,
        true,
    );
    assert_eq!(instance, Some(WP_SABER));
    assert_eq!(cent_weapon, WP_SABER);
    assert_eq!((attached, saber1, saber2), (None, true, true));

    // The following CG_Player weapon update is pointer-equal and therefore
    // does not recopy anything merely because the primary is in flight.
    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        Some(WP_SABER),
        true,
        false,
        false,
        true,
        true,
    );
    assert_eq!((saber1, saber2), (true, true));

    // cg_players.c's saberInFlight model path removes Ghoul2 model index 1
    // only. Model index 2 is deliberately untouched.
    saber1 = false;
    assert_eq!((saber1, saber2), (false, true));

    // On subsequent in-flight frames the index-1 g2HasWeapon test resets
    // tracking, but saberInFlight pins ghoul2weapon and still must not
    // disturb model index 2.
    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        Some(WP_SABER),
        true,
        false,
        false,
        true,
        true,
    );
    assert_eq!((saber1, saber2), (false, true));
}

#[test]
fn saber_return_rearms_missing_model_slot_like_openjk_g2hasweapon() {
    let mut instance = Some(WP_SABER);
    let mut cent_weapon = WP_SABER;
    let mut attached = None;
    let mut saber1 = false;
    let mut saber2 = false;

    // Model index 1 has been removed by the in-flight saber path. While the
    // saber is still away, OpenJK's g2HasWeapon check clears cent->weapon,
    // then saberInFlight pins ghoul2weapon to the saber instance so it is
    // not immediately recopied into the hand.
    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        Some(WP_SABER),
        true,
        false,
        false,
        true,
        false,
    );
    assert_eq!(instance, Some(WP_SABER));
    assert_eq!(cent_weapon, 0);
    assert_eq!((attached, saber1, saber2), (None, false, false));

    // On the first frame where saberInFlight clears, model index 1 is still
    // absent. The same g2HasWeapon check therefore clears ghoul2weapon, the
    // normal mismatch test fires, and CG_CopyG2WeaponInstance semantics
    // restore the hilt. This is the catch/recovery transition from OpenJK.
    update_weapon_attachment(
        &mut instance,
        &mut cent_weapon,
        &mut attached,
        &mut saber1,
        &mut saber2,
        WP_SABER,
        Some(WP_SABER),
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(instance, Some(WP_SABER));
    assert_eq!(cent_weapon, WP_SABER);
    assert_eq!((attached, saber1, saber2), (None, true, false));
}

#[test]
fn angles_to_axis_matches_jka_cardinal_yaw() {
    let axis = angles_to_axis([0.0, 90.0, 0.0]);
    assert!(axis[0][0].abs() < 1.0e-5);
    assert!((axis[0][1] - 1.0).abs() < 1.0e-5);
    assert!((axis[2][2] - 1.0).abs() < 1.0e-5);
}

#[test]
fn glm_winding_is_flipped_for_wgpu_backface_culling() {
    assert_eq!(
        glm_indices_for_wgpu(vec![0, 1, 2, 3, 4, 5]),
        vec![0, 2, 1, 3, 5, 4]
    );
}

#[test]
fn player_entity_reset_matches_openjk_teleport_and_client_discontinuity() {
    assert!(!openjk_player_entity_needs_reset(0, 2, 0, 2, false, false));
    assert!(openjk_player_entity_needs_reset(
        0,
        2,
        EF_TELEPORT_BIT,
        2,
        false,
        false
    ));
    assert!(openjk_player_entity_needs_reset(
        EF_TELEPORT_BIT,
        2,
        0,
        2,
        false,
        false
    ));
    assert!(openjk_player_entity_needs_reset(0, 2, 0, 3, false, false));
    assert!(openjk_player_entity_needs_reset(0, 2, 0, 2, true, false));
    assert!(openjk_player_entity_needs_reset(0, 2, 0, 2, false, true));
}

#[test]
fn player_angle_reset_preserves_openjk_clientinfo_state() {
    let mut state = PlayerAngleState {
        torso_yawing: 1,
        torso_pitching: 1,
        legs_yawing: 1,
        torso_yaw_angle: 1.0,
        torso_pitch_angle: 2.0,
        legs_yaw_angle: 3.0,
        corr_time: 41,
        look_time: 42,
        super_smooth_time: 43,
        last_head_angles: [4.0, 5.0, 6.0],
    };
    state.reset_entity_swing_from_angles([10.0, 20.0, 30.0]);
    assert_eq!(state.torso_yawing, 0);
    assert_eq!(state.torso_pitching, 0);
    assert_eq!(state.legs_yawing, 0);
    assert_eq!(state.torso_yaw_angle, 20.0);
    assert_eq!(state.torso_pitch_angle, 10.0);
    assert_eq!(state.legs_yaw_angle, 20.0);
    assert_eq!(state.super_smooth_time, 0);
    assert_eq!(state.corr_time, 41);
    assert_eq!(state.look_time, 42);
    assert_eq!(state.last_head_angles, [4.0, 5.0, 6.0]);
}
