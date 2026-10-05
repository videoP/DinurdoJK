/// The original ordering implementation, kept to prove the matrix version
/// returns exactly the same sequence.
fn order_pvs_pieces_reference<T>(
    pieces: std::collections::BTreeMap<(Vec<u64>, [u64; 4]), T>,
) -> Vec<((Vec<u64>, [u64; 4]), T)> {
    const MAX_ORDERED_PIECES: usize = 4096;
    let mut pieces = pieces.into_iter().collect::<Vec<_>>();
    let count = pieces.len();
    if count <= 2 || count > MAX_ORDERED_PIECES {
        return pieces;
    }
    let distance = |a: &[u64], b: &[u64]| -> u32 {
        let shared = a.len().min(b.len());
        let tail = a[shared..]
            .iter()
            .chain(&b[shared..])
            .map(|word| word.count_ones())
            .sum::<u32>();
        a[..shared]
            .iter()
            .zip(&b[..shared])
            .map(|(x, y)| (x ^ y).count_ones())
            .sum::<u32>()
            + tail
    };
    let mut current = (0..count)
        .min_by_key(|&index| {
            pieces[index]
                .0
                 .0
                .iter()
                .map(|word| word.count_ones())
                .sum::<u32>()
        })
        .unwrap_or(0);
    let mut visited = vec![false; count];
    let mut order = Vec::with_capacity(count);
    visited[current] = true;
    order.push(current);
    for _ in 1..count {
        let next = (0..count)
            .filter(|&index| !visited[index])
            .min_by_key(|&index| distance(&pieces[current].0 .0, &pieces[index].0 .0))
            .unwrap();
        visited[next] = true;
        order.push(next);
        current = next;
    }
    let signature = |piece: usize| pieces[piece].0 .0.as_slice();
    for _ in 0..8 {
        let mut improved = false;
        for i in 0..count.saturating_sub(2) {
            for j in i + 2..count {
                let (a, b, c) = (order[i], order[i + 1], order[j]);
                let d = order.get(j + 1).copied();
                let before = distance(signature(a), signature(b))
                    + d.map_or(0, |d| distance(signature(c), signature(d)));
                let after = distance(signature(a), signature(c))
                    + d.map_or(0, |d| distance(signature(b), signature(d)));
                if after < before {
                    order[i + 1..=j].reverse();
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    let mut slots = pieces.drain(..).map(Some).collect::<Vec<_>>();
    order
        .into_iter()
        .map(|index| slots[index].take().unwrap())
        .collect()
}

#[test]
fn piece_ordering_matches_the_reference() {
    let mut state = 0x1234_5678_u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    for (count, words) in [(3, 1), (17, 2), (60, 3), (150, 5), (40, 0)] {
        let mut pieces = std::collections::BTreeMap::new();
        for index in 0..count {
            let signature = (0..words)
                .map(|_| u64::from(next()) << 32 | u64::from(next() & next()))
                .collect::<Vec<_>>();
            pieces.insert((signature, [index as u64, 0, 0, 0]), index);
        }
        let fast = super::order_pvs_pieces(pieces.clone());
        let reference = order_pvs_pieces_reference(pieces);
        assert_eq!(fast, reference, "{count} pieces x {words} words");
    }
}

/// A second preparation that only changes options the heavy stages do not
/// read must reuse them and still return an identical world.
#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s"]
fn reprepare_reuses_unchanged_stages_and_matches_a_fresh_build() {
    let base =
        std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
    let name = std::env::var("JKA_TEST_MAP").unwrap_or_else(|_| "mp/duel3".into());

    let first_options = MapPrepareOptions {
        ocean: true,
        steam_audio: false,
        ..Default::default()
    };
    let started = Instant::now();
    let first = Arc::new(prepare_with_options(&base, None, &name, first_options).unwrap());
    let first_ms = started.elapsed().as_secs_f64() * 1000.0;

    // Ocean/physics do not feed the reusable stages, so everything is lent by `first`.
    let second_options = MapPrepareOptions {
        ocean: false,
        client_physics: true,
        ..first_options
    };
    let started = Instant::now();
    let second = prepare_with_seed(&base, None, &name, second_options, &first).unwrap();
    let second_ms = started.elapsed().as_secs_f64() * 1000.0;

    // Reflection topology changes the batches, so the PVS plan must be rebuilt.
    let third_options = MapPrepareOptions {
        planar_reflections: false,
        planar_environment: false,
        ..first_options
    };
    let third = prepare_with_seed(&base, None, &name, third_options, &first).unwrap();

    // Ground truth: the same options as `second`, built with no seed at all.
    let fresh = prepare_with_options(&base, None, &name, second_options).unwrap();

    println!(
            "first {first_ms:.0} ms, second {second_ms:.0} ms (plans {:.0}, gi {:.0}, bsp {:.0}), third plans {:.0} ms",
            second.load_timings.portal_plans_ms,
            second.load_timings.gi_ms,
            second.load_timings.bsp_parse_ms,
            third.load_timings.portal_plans_ms,
        );
    println!(
        "first timings {:?}
second timings {:?}",
        first.load_timings, second.load_timings
    );
    assert!(
        second_ms < first_ms,
        "re-prepare should be faster than the first build"
    );
    assert_eq!(second.vertices.len(), fresh.vertices.len(), "vertices");
    assert_eq!(second.batches.len(), fresh.batches.len(), "batches");
    assert_eq!(
        second.pvs_batches.len(),
        fresh.pvs_batches.len(),
        "pvs batches"
    );
    assert_eq!(
        format!("{:?}", second.portal_draw_plan),
        format!("{:?}", fresh.portal_draw_plan),
        "plan"
    );
    assert_eq!(
        format!("{:?}", second.voxel_probe_gi),
        format!("{:?}", fresh.voxel_probe_gi),
        "gi"
    );
    assert_eq!(
        second.grass_patches.len(),
        fresh.grass_patches.len(),
        "grass"
    );
    assert_eq!(second.textures.len(), fresh.textures.len(), "textures");
    assert!(
        second
            .textures
            .iter()
            .zip(&fresh.textures)
            .all(|(a, b)| a.rgba == b.rgba && a.label == b.label),
        "texture pixels"
    );
    assert_ne!(
        format!("{:?}", third.portal_draw_plan),
        format!("{:?}", first.portal_draw_plan),
        "a batch-topology change must not reuse the old plan"
    );
}

/// Full headless map preparation of a demo's map: inline BSP models must
/// come out as self-consistent mover geometry sharing the world's
/// material and lightmap resources.
#[test]
#[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26"]
fn demo_map_prepares_inline_models_with_world_materials() {
    use jka_protocol::{demo::DemoReader, server::Decoder};
    let base =
        std::path::PathBuf::from(std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE"));
    let mut assets = AssetSearchPath::open(&base).unwrap();
    let bytes = assets
        .read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024)
        .unwrap()
        .unwrap()
        .bytes;
    let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
    let mut decoder = Decoder::new();
    let map = loop {
        let record = reader.next_record().unwrap().expect("demo gamestate");
        decoder
            .parse_packet(record.sequence, &record.payload)
            .unwrap();
        if let Some(map) = decoder.map_name() {
            break map;
        }
    };
    let prepared = prepare(&base, None, &map).unwrap();
    println!(
        "INLINE PREPARE {map}: models={:?} vertices={} batches={} worldVertices={}",
        prepared
            .inline_models
            .iter()
            .map(|model| model.model)
            .collect::<Vec<_>>(),
        prepared.inline_vertices.len(),
        prepared.inline_batches.len(),
        prepared.vertices.len(),
    );
    assert!(!prepared.inline_models.is_empty());
    let mut next_batch = 0;
    for model in &prepared.inline_models {
        assert!(model.model > 0, "model 0 is the static world");
        assert_eq!(
            model.batches.start, next_batch,
            "models own contiguous batches"
        );
        next_batch = model.batches.end;
        assert!(model.vertices.end as usize <= prepared.inline_vertices.len());
        assert_eq!(model.vertices.len() % 3, 0);
        for batch in &prepared.inline_batches[model.batches.clone()] {
            assert!(
                batch.vertices.start >= model.vertices.start
                    && batch.vertices.end <= model.vertices.end
            );
            assert!(
                batch.pvs_signature.is_empty(),
                "movers are not PVS-bound to compiled clusters"
            );
            assert!(!batch.water_primary && !batch.planar_reflection);
            assert!(batch
                .lightmap
                .is_none_or(|page| page < prepared.lightmaps.len()));
            assert!(batch
                .texture
                .is_none_or(|texture| texture < prepared.textures.len()));
        }
    }
    assert_eq!(next_batch, prepared.inline_batches.len());
    assert!(
        prepared
            .inline_batches
            .iter()
            .any(|batch| batch.lightmap.is_some()),
        "inline models lost their lightmaps"
    );
}

use super::*;

#[test]
fn one_zero_world_base_is_opaque_and_seeds_depth() {
    let stage = MaterialStage {
        texture: StageTexture::Image(7),
        enhancements: Default::default(),
        blend: Some(materials::BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::Zero,
        }),
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    let material = SurfaceMaterial {
        stages: vec![stage.clone()],
        explicit: true,
        ..Default::default()
    };

    assert_eq!(stage_class(&stage), DrawClass::Opaque);
    assert_eq!(blend_for(&stage), BlendMode::Opaque);

    let first = stage_pipeline(&material, &stage, true);
    assert_eq!(first.class, DrawClass::Opaque);
    assert_eq!(first.blend, BlendMode::Opaque);
    assert!(first.depth_write);

    let later = stage_pipeline(&material, &stage, false);
    assert!(
        !later.depth_write,
        "only a replacement base gets implicit depthWrite"
    );
}

fn sun_test_grid(cells: Vec<ClassicLightGridCell>) -> ClassicEntityLightGrid {
    ClassicEntityLightGrid {
        origin: [0.0; 3],
        size: [64.0, 64.0, 128.0],
        bounds: [cells.len() as u32, 1, 1],
        external_hdr: false,
        cells,
        map_toward_sun: None,
    }
}

/// Coherence is the length of the blended probe direction: 1 when neighbouring
/// probes agree, 0 when they point opposite ways and cancel.
#[test]
fn direction_coherence_reports_disagreeing_probes() {
    let cell = |direction: [f32; 3]| ClassicLightGridCell {
        ambient: [40.0; 3],
        directed: [100.0; 3],
        direction,
        sun_weight: 0.0,
        valid: true,
    };
    let agree = sun_test_grid(vec![cell([0.0, 1.0, 0.0]), cell([0.0, 1.0, 0.0])]);
    let oppose = sun_test_grid(vec![cell([0.0, 1.0, 0.0]), cell([0.0, -1.0, 0.0])]);
    // Halfway between the two probes (64-unit spacing in x).
    let midpoint = [32.0, 0.0, 0.0];
    let agreeing = agree.sample_classic_entity_light(midpoint).unwrap();
    let opposing = oppose.sample_classic_entity_light(midpoint).unwrap();
    assert!(
        agreeing.direction_coherence > 0.99,
        "{}",
        agreeing.direction_coherence
    );
    assert!(
        opposing.direction_coherence < 0.05,
        "{}",
        opposing.direction_coherence
    );
}

/// The estimate must separate a sunlit probe (blended direction on the sun,
/// directed color equal to the sun color) from torch-lit and skylit probes,
/// and the relight must move exactly the sun share to the new sun.
#[test]
fn entity_sun_weights_split_sunlit_from_torch_and_sky_probes() {
    let toward_sun = [0.0, 1.0, 0.0];
    let warm = [255.0, 200.0, 120.0];
    let cell = |directed: [f32; 3], direction: [f32; 3]| ClassicLightGridCell {
        ambient: [40.0; 3],
        directed,
        direction,
        sun_weight: 0.0,
        valid: true,
    };
    let mut grid = sun_test_grid(vec![
        cell(warm, toward_sun),                // sunlit
        cell(warm, [1.0, 0.0, 0.0]),           // warm torch from the side
        cell([60.0, 90.0, 255.0], toward_sun), // blue sky fill from above
        cell([0.0; 3], toward_sun),            // unlit
    ]);
    // Travel direction is the negated toward-sun direction.
    grid.estimate_sun_weights(
        [0.0, -1.0, 0.0],
        [warm[0] / 255.0, warm[1] / 255.0, warm[2] / 255.0],
    );
    let weights = grid.cells.iter().map(|c| c.sun_weight).collect::<Vec<_>>();
    assert!(weights[0] > 0.99, "sunlit probe: {weights:?}");
    assert!(weights[1] < 0.01, "side torch: {weights:?}");
    assert!(weights[2] < 0.01, "blue skylight: {weights:?}");
    assert!(weights[3] == 0.0, "unlit probe: {weights:?}");
    assert_eq!(grid.map_toward_sun(), Some(toward_sun));

    let mut light = ClassicEntityLight {
        ambient: [40.0; 3],
        directed: [100.0, 80.0, 48.0],
        direction: toward_sun,
        baked_sun: [100.0, 80.0, 48.0],
        ..Default::default()
    };
    let new_toward = [1.0, 0.0, 0.0];
    light.relight_sun(toward_sun, new_toward, [0.5, 1.0, 2.0]);
    assert_eq!(light.directed, [0.0; 3], "all directed light was sun");
    assert_eq!(light.sun_directed, [50.0, 80.0, 96.0]);
    assert_eq!(light.sun_direction, new_toward);

    // A probe with no sun share is left exactly as sampled.
    let mut torch = ClassicEntityLight {
        directed: [30.0; 3],
        direction: [1.0, 0.0, 0.0],
        ..Default::default()
    };
    torch.relight_sun(toward_sun, new_toward, [9.0; 3]);
    assert_eq!(torch.directed, [30.0; 3]);
    assert_eq!(torch.sun_directed, [0.0; 3]);
}

fn legacy_lightmap_modulate_pair(enhanced: bool) -> [MaterialStage; 2] {
    let lightmap = MaterialStage {
        texture: StageTexture::Lightmap,
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Lightmap,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    let mut diffuse = MaterialStage {
        texture: StageTexture::Image(7),
        enhancements: Default::default(),
        blend: Some(materials::BlendFunc {
            src: BlendFactor::DstColor,
            dst: BlendFactor::Zero,
        }),
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    if enhanced {
        diffuse.enhancements.normal_texture = Some(8);
    }
    [lightmap, diffuse]
}

#[test]
fn plain_jka_lightmap_then_dst_color_diffuse_collapses_to_modulation() {
    let [lightmap, diffuse] = legacy_lightmap_modulate_pair(false);
    assert!(can_fold_jka_lightmap_pair(&lightmap, &diffuse));
}

#[test]
fn enhanced_lightmap_then_dst_color_diffuse_still_collapses_to_modulation() {
    let [lightmap, diffuse] = legacy_lightmap_modulate_pair(true);
    assert!(can_fold_jka_lightmap_pair(&lightmap, &diffuse));
}

#[test]
fn nonidentity_diffuse_stage_does_not_use_plain_lightmap_collapse() {
    let [lightmap, mut diffuse] = legacy_lightmap_modulate_pair(false);
    diffuse.color = [0.5, 1.0, 1.0];
    assert!(!can_fold_jka_lightmap_pair(&lightmap, &diffuse));
}

#[test]
fn vertex_lit_lightmap_stage_keeps_explicit_white_texture_source() {
    let diffuse = MaterialStage {
        texture: StageTexture::Image(7),
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: true,
        depth_equal: false,
    };
    let lightmap = MaterialStage {
        texture: StageTexture::Lightmap,
        enhancements: Default::default(),
        blend: Some(materials::BlendFunc {
            src: BlendFactor::DstColor,
            dst: BlendFactor::Zero,
        }),
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    let material = SurfaceMaterial {
        stages: vec![diffuse, lightmap],
        explicit: true,
        ..Default::default()
    };

    let stages = prepared_stages(&material, true);
    assert!(matches!(stages[1].texture, StageTexture::White));
    assert_eq!(stages[1].rgb_gen, RgbGen::ExactVertex);

    let batch = stage_batch(
        &material,
        &stages[1],
        false,
        true,
        0..3,
        Some(226),
        None,
        None,
        &[],
        [0; 4],
        [0.0; 4],
        false,
    );
    assert!(batch.texture.is_none());
    assert!(!batch.texture_is_lightmap);
    assert!(batch.texture_is_white);
}

#[test]
fn vertex_lit_pbr_override_consumes_bsp_vertex_lighting() {
    let mut base = MaterialStage {
        texture: StageTexture::Image(7),
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    base.enhancements.normal_texture = Some(8);
    base.enhancements.roughness_texture = Some(9);

    let glow = MaterialStage {
        texture: StageTexture::Image(10),
        enhancements: Default::default(),
        blend: Some(materials::BlendFunc {
            src: BlendFactor::One,
            dst: BlendFactor::One,
        }),
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    let material = SurfaceMaterial {
        stages: vec![base, glow],
        explicit: true,
        ..Default::default()
    };

    let stages = prepared_stages(&material, true);
    assert_eq!(stages[0].rgb_gen, RgbGen::ExactVertex);
    assert_eq!(stages[1].rgb_gen, RgbGen::Identity);
}

#[test]
fn classic_explicit_vertex_lit_identity_stage_is_not_forced() {
    let base = MaterialStage {
        texture: StageTexture::Image(7),
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.0,
        opacity: 1.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Identity,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    let material = SurfaceMaterial {
        stages: vec![base],
        explicit: true,
        ..Default::default()
    };

    let stages = prepared_stages(&material, true);
    assert_eq!(stages[0].rgb_gen, RgbGen::Identity);
}

#[test]
fn guaranteed_alpha_discard_render_cull_keeps_surface_light_semantics() {
    let material = SurfaceMaterial {
        stages: vec![MaterialStage {
            texture: StageTexture::White,
            enhancements: Default::default(),
            blend: None,
            alpha_cutoff: 0.5,
            opacity: 0.0,
            color: [1.0; 3],
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Const,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        }],
        surface_light: Some(materials::SurfaceLight {
            value: 750.0,
            color: [1.0; 3],
            subdivide: 120.0,
            inferred_from_emissive: false,
        }),
        explicit: true,
        ..Default::default()
    };

    assert!(material.surface_light.is_some());
    assert!(material_render_is_guaranteed_discarded(&material, true));
}

#[test]
fn black_additive_surface_light_render_cull_keeps_surface_light_semantics() {
    let material = SurfaceMaterial {
        stages: vec![MaterialStage {
            texture: StageTexture::White,
            enhancements: Default::default(),
            blend: Some(materials::BlendFunc {
                src: BlendFactor::One,
                dst: BlendFactor::One,
            }),
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [0.0; 3],
            rgb_gen: RgbGen::Const,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        }],
        surface_light: Some(materials::SurfaceLight {
            value: 500.0,
            color: [1.0; 3],
            subdivide: 120.0,
            inferred_from_emissive: false,
        }),
        explicit: true,
        ..Default::default()
    };

    assert!(material.surface_light.is_some());
    assert!(material_render_is_guaranteed_discarded(&material, true));
}

#[test]
fn black_additive_non_light_shader_is_not_mapload_culled() {
    let material = SurfaceMaterial {
        stages: vec![MaterialStage {
            texture: StageTexture::White,
            enhancements: Default::default(),
            blend: Some(materials::BlendFunc {
                src: BlendFactor::One,
                dst: BlendFactor::One,
            }),
            alpha_cutoff: 0.0,
            opacity: 1.0,
            color: [0.0; 3],
            rgb_gen: RgbGen::Const,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: false,
            depth_equal: false,
        }],
        explicit: true,
        ..Default::default()
    };

    assert!(!material_render_is_guaranteed_discarded(&material, false));
}

#[test]
fn guaranteed_alpha_discard_render_cull_requires_every_stage_to_discard() {
    let invisible = MaterialStage {
        texture: StageTexture::White,
        enhancements: Default::default(),
        blend: None,
        alpha_cutoff: 0.5,
        opacity: 0.0,
        color: [1.0; 3],
        rgb_gen: RgbGen::Identity,
        alpha_gen: AlphaGen::Const,
        tc_gen: TcGen::Base,
        tc_mods: Vec::new(),
        depth_write: false,
        depth_equal: false,
    };
    let visible = MaterialStage {
        opacity: 1.0,
        alpha_gen: AlphaGen::Identity,
        ..invisible.clone()
    };
    let material = SurfaceMaterial {
        stages: vec![invisible, visible],
        explicit: true,
        ..Default::default()
    };

    assert!(!material_render_is_guaranteed_discarded(&material, false));
}

#[test]
fn coordinate_conversion_round_trips() {
    let jka = [1.0, 2.0, 3.0];
    assert_eq!(render_position(jka), [1.0, 3.0, -2.0]);
    assert_eq!(jka_position(render_position(jka)), jka);
}

#[test]
fn distance_cull_matches_openjk_leading_float_parse() {
    assert_eq!(parse_distance_cull("24000"), Some(24000.0));
    assert_eq!(parse_distance_cull("24000r"), Some(24000.0));
    assert_eq!(parse_distance_cull(" 8500.5 trailing"), Some(8500.5));
    assert_eq!(parse_distance_cull("bogus"), None);
}

#[test]
fn surface_sprite_twosided_material_uses_retail_one_sided_world_cull() {
    let material = SurfaceMaterial {
        cull: CullMode::None,
        surface_sprite_cull_quirk: true,
        ..Default::default()
    };
    assert_eq!(effective_world_cull(&material), CullMode::Back);

    let ordinary_twosided = SurfaceMaterial {
        cull: CullMode::None,
        ..Default::default()
    };
    assert_eq!(effective_world_cull(&ordinary_twosided), CullMode::None);
}

fn planar_test_vertex(position: [f32; 3]) -> GpuVertex {
    GpuVertex {
        position,
        uv: [0.0; 2],
        lightmap_uv: [0.0; 2],
        normal: [0.0, 0.0, 1.0],
        color: [1.0; 4],
        alpha_cutoff: 1.0,
    }
}

#[test]
fn environment_planar_promotion_rejects_nonplanar_geometry() {
    let planar = [
        planar_test_vertex([0.0, 0.0, 0.0]),
        planar_test_vertex([64.0, 0.0, 0.0]),
        planar_test_vertex([0.0, 64.0, 0.0]),
        planar_test_vertex([64.0, 0.0, 0.0]),
        planar_test_vertex([64.0, 64.0, 0.0]),
        planar_test_vertex([0.0, 64.0, 0.0]),
    ];
    assert!(planar_batch_plane(&planar, true).is_some());

    let mut bent = planar;
    bent[4].position[2] = 8.0;
    assert!(planar_batch_plane(&bent, true).is_none());
    // Authored mirrors remain authoritative and can still derive a plane
    // from their first valid triangle, matching the original mirror path.
    assert!(planar_batch_plane(&bent, false).is_some());
}

#[test]
fn q3map_surface_light_subdivision_preserves_area_scaled_energy() {
    let authored = materials::SurfaceLight {
        value: 300.0,
        color: [1.0, 1.0, 1.0],
        subdivide: 100.0,
        inferred_from_emissive: false,
    };
    let mut candidates = Vec::new();
    append_surface_light_triangle(
        &mut candidates,
        [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(200.0, 0.0, 0.0),
            Vec3::new(0.0, 200.0, 0.0),
        ],
        Vec3::Z,
        authored,
        false,
    );
    assert!(candidates.len() > 1);
    let total_intensity: f32 = candidates
        .iter()
        .map(|candidate| candidate.light.intensity)
        .sum();
    assert!((total_intensity - 2.0).abs() < 1e-4);
    assert!(candidates.iter().all(|candidate| {
        candidate.light.emitter_normal == Vec3::Z.to_array() && !candidate.light.emitter_two_sided
    }));
}

#[test]
fn voxel_gi_propagation_does_not_cross_a_solid_wall() {
    let bounds = [7, 3, 3];
    let cell_count = bounds[0] as usize * bounds[1] as usize * bounds[2] as usize;
    let mut occupied = vec![false; cell_count];
    for z in 0..bounds[2] {
        for y in 0..bounds[1] {
            occupied[voxel_gi_index(3, y, z, bounds)] = true;
        }
    }
    let mut source = vec![Vec3::ZERO; cell_count];
    source[voxel_gi_index(1, 1, 1, bounds)] = Vec3::ONE;
    let field = voxel_gi_propagate(&source, &occupied, bounds);

    assert!(field[voxel_gi_index(2, 1, 1, bounds)].max_element() > 0.0);
    assert_eq!(field[voxel_gi_index(4, 1, 1, bounds)], Vec3::ZERO);
    assert_eq!(field[voxel_gi_index(5, 1, 1, bounds)], Vec3::ZERO);
}

#[test]
fn q3map_sun_angles_round_trip_through_renderer_space() {
    for (yaw, pitch) in [
        (0.0, 0.0),
        (90.0, 45.0),
        (220.0, 30.0),
        (314.25595, 57.04725),
    ] {
        let sun = DirectionalSun::from_q3_angles([1.0, 0.5, 0.25], 250.0, yaw, pitch);
        let [actual_yaw, actual_pitch] = sun.q3_angles();
        let yaw_error = (actual_yaw - yaw)
            .abs()
            .min(360.0 - (actual_yaw - yaw).abs());
        assert!(yaw_error < 1e-3, "yaw {yaw} became {actual_yaw}");
        assert!(
            (actual_pitch - pitch).abs() < 1e-3,
            "pitch {pitch} became {actual_pitch}"
        );
    }
}

#[test]
fn q3map_sun_direction_converts_to_renderer_space() {
    let noon = render_sun(ShaderSun {
        color: [1.0, 0.0, 0.0],
        intensity: 100.0,
        azimuth: 0.0,
        elevation: 90.0,
        deviance: 0.0,
        samples: 0,
    });
    assert!(noon.direction[0].abs() < 1e-5);
    assert!((noon.direction[1] + 1.0).abs() < 1e-5);
    assert!(noon.direction[2].abs() < 1e-5);

    let east_horizon = render_sun(ShaderSun {
        color: [1.0, 0.0, 0.0],
        intensity: 100.0,
        azimuth: 0.0,
        elevation: 0.0,
        deviance: 0.0,
        samples: 0,
    });
    assert!((east_horizon.direction[0] + 1.0).abs() < 1e-5);
    assert!(east_horizon.direction[1].abs() < 1e-5);
    assert!(east_horizon.direction[2].abs() < 1e-5);
}

fn legacy_box_source(top_shader: &str) -> String {
    format!(
        r#"{{
"classname" "worldspawn"
{{
( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall 0 0 0 1 1
( 64 0 0 ) ( 64 0 64 ) ( 64 64 0 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 0 0 64 ) ( 64 0 0 ) test/wall 0 0 0 1 1
( 0 64 0 ) ( 64 64 0 ) ( 0 64 64 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 64 0 ) test/floor 0 0 0 1 1
( 0 0 64 ) ( 0 64 64 ) ( 64 0 64 ) {top_shader} 0 0 0 1 1
}}
}}
"#
    )
}

fn parsed_box() -> MapBrush {
    let document = jka_assets::map::parse(&legacy_box_source("test/ceiling")).unwrap();
    document.entities[0].brushes[0].clone()
}

#[test]
fn six_sided_box_reconstructs_all_faces_and_corners() {
    let reconstructed = reconstruct_brush(&parsed_box()).unwrap();
    assert_eq!(reconstructed.vertices.len(), 8);
    assert_eq!(reconstructed.faces.len(), 6);
    assert!(reconstructed
        .faces
        .iter()
        .all(|face| face.vertices.len() == 4));
}

#[test]
fn sloped_brush_face_reconstructs() {
    let mut brush = parsed_box();
    let length = 1.25_f64.sqrt();
    brush.faces[5].plane = MapPlane {
        normal: [-0.5 / length, 0.0, 1.0 / length],
        distance: 64.0 / length,
    };
    let reconstructed = reconstruct_brush(&brush).unwrap();
    assert_eq!(reconstructed.vertices.len(), 8);
    let top = reconstructed
        .faces
        .iter()
        .find(|face| face.face_index == 5)
        .unwrap();
    assert_eq!(top.vertices.len(), 4);
    assert!(top
        .vertices
        .iter()
        .any(|point| (point.z - 96.0).abs() < 1e-5));
}

#[test]
fn reversed_plane_does_not_produce_a_valid_closed_brush() {
    let mut brush = parsed_box();
    brush.faces[0].plane.normal = [1.0, 0.0, 0.0];
    brush.faces[0].plane.distance = 0.0;
    assert!(reconstruct_brush(&brush).is_err());
}

#[test]
fn reconstructed_face_winding_points_outward() {
    let brush = parsed_box();
    let reconstructed = reconstruct_brush(&brush).unwrap();
    for polygon in reconstructed.faces {
        let normal = map_vec(brush.faces[polygon.face_index].plane.normal);
        let a = polygon.vertices[0];
        let b = polygon.vertices[1];
        let c = polygon.vertices[2];
        assert!((b - a).cross(c - a).dot(normal) > 0.0);
    }
}

#[test]
fn utility_face_is_hidden_but_still_clips_the_brush() {
    let document = jka_assets::map::parse(&legacy_box_source("system/caulk")).unwrap();
    let reconstructed = reconstruct_brush(&document.entities[0].brushes[0]).unwrap();
    assert_eq!(reconstructed.vertices.len(), 8);
    assert_eq!(reconstructed.faces.len(), 6);

    let root = std::env::temp_dir().join(format!("jka-map-scene-test-{}", std::process::id()));
    let prepared = prepare_map_document(&root, None, Path::new("fixture.map"), document).unwrap();
    let stats = prepared.map_file_stats.unwrap();
    assert_eq!(stats.world_brushes, 1);
    assert_eq!(stats.grouped_world_brushes, 0);
    assert_eq!(stats.rendered_faces, 5);
    assert_eq!(stats.collision_brushes, 1);
    assert_eq!(prepared.triangles, 10);
    assert!(
        prepared.collision.is_some(),
        "caulk must stay in gameplay collision"
    );
}

#[test]
fn shader_metadata_hides_custom_utility_surfaces() {
    let mut shader = Shader::default();
    shader.player_clip = true;
    assert!(shader_is_render_utility(&shader));
    shader.player_clip = false;
    shader.nonsolid = true;
    shader.translucent = true;
    assert!(
        !shader_is_render_utility(&shader),
        "visible glass must not be dropped just because it is nonsolid"
    );
}

#[test]
fn implicit_detail_face_keeps_default_solid_contents() {
    let face = MapFace {
        plane: MapPlane {
            normal: [0.0, 0.0, 1.0],
            distance: 64.0,
        },
        shader: "textures/rift/floor1b".into(),
        projection: TextureProjection::Legacy {
            shift: [0.0, 0.0],
            rotate: 0.0,
            scale: [0.5, 0.5],
        },
        // Radiant commonly stores CONTENTS_DETAIL here without repeating
        // the implicit SOLID default.
        trailing: vec![0x0800_0000_u32 as f64, 0.0, 0.0],
        plane_span: [0, 0],
        line: 1,
    };
    let (contents, _) = source_face_collision_flags(&face, None);
    let contents = contents as u32;
    assert_ne!(contents & SOURCE_CONTENTS_SOLID, 0);
    assert_ne!(contents & SOURCE_CONTENTS_OPAQUE, 0);
    assert_ne!(contents & 0x0800_0000, 0);

    // The same modifier must not erase solidity merely because the texture
    // has an explicit shader definition.
    let scripted = Shader::default();
    let (contents, _) = source_face_collision_flags(&face, Some(&scripted));
    let contents = contents as u32;
    assert_ne!(contents & SOURCE_CONTENTS_SOLID, 0);
    assert_ne!(contents & SOURCE_CONTENTS_OPAQUE, 0);
    assert_ne!(contents & 0x0800_0000, 0);

    // A real gameplay content class is different: playerclip must remain a
    // clip volume, not inherit the ordinary SOLID bit from the implicit base.
    let clip_face = MapFace {
        shader: "textures/common/playerclip".into(),
        trailing: vec![SOURCE_CONTENTS_PLAYERCLIP as f64, 0.0, 0.0],
        ..face
    };
    let (contents, _) = source_face_collision_flags(&clip_face, None);
    let contents = contents as u32;
    assert_eq!(contents & SOURCE_CONTENTS_SOLID, 0);
    assert_ne!(contents & SOURCE_CONTENTS_PLAYERCLIP, 0);
}

#[test]
fn source_map_world_ambient_and_minlight_match_q3map2_keys() {
    let mut world = jka_assets::map::MapEntity::default();
    world
        .properties
        .insert("classname".into(), "worldspawn".into());
    world
        .properties
        .insert("_color".into(), "0.5 1 0.25".into());
    world.properties.insert("_ambient".into(), "51".into());
    world.properties.insert("_minlight".into(), "25.5".into());
    let lighting = map_world_lighting(&world);
    assert_eq!(lighting.ambient, [0.1, 0.2, 0.05]);
    assert_eq!(lighting.minlight, [0.05, 0.1, 0.025]);

    let mut alias = jka_assets::map::MapEntity::default();
    alias.properties.insert("ambient".into(), "25.5".into());
    let lighting = map_world_lighting(&alias);
    assert_eq!(lighting.ambient, [0.1, 0.1, 0.1]);
}

#[test]
fn source_map_light_uses_authored_scale_color_and_linear_flag() {
    let mut entity = jka_assets::map::MapEntity::default();
    entity.properties.insert("classname".into(), "light".into());
    entity.properties.insert("origin".into(), "64 32 16".into());
    entity.properties.insert("_color".into(), "1 0 0".into());
    entity.properties.insert("_light".into(), "450".into());
    entity.properties.insert("scale".into(), "0.5".into());
    entity.properties.insert("spawnflags".into(), "1".into());
    entity.properties.insert("fade".into(), "2".into());

    let light = map_dynamic_light(&entity).expect("light entity");
    assert_eq!(light.color, [1.0, 0.0, 0.0]);
    assert_eq!(light.falloff, DynamicLightFalloff::Linear);
    assert!(light.surface_lighting);
    let photons = 450.0 * 0.5 * Q3MAP_POINT_SCALE;
    assert!((light.intensity - photons / Q3MAP_LIGHTMAP_BYTE_SCALE).abs() < 1e-3);
    assert!((light.radius - photons * Q3MAP_LINEAR_SCALE / 2.0).abs() < 1e-3);
    assert!(!light.angle_attenuation);
    assert_eq!(light.color, [1.0, 0.0, 0.0]);
}

#[test]
fn source_map_point_light_uses_q3map2_photon_scale_and_fast_envelope() {
    let mut entity = jka_assets::map::MapEntity::default();
    entity.properties.insert("classname".into(), "light".into());
    entity
        .properties
        .insert("origin".into(), "-24 128 400".into());
    entity.properties.insert("light".into(), "300".into());

    let light = map_dynamic_light(&entity).expect("light entity");
    let photons = 300.0 * Q3MAP_POINT_SCALE;
    assert_eq!(light.color, [1.0, 1.0, 1.0]);
    assert_eq!(light.falloff, DynamicLightFalloff::InverseSquare);
    assert!(light.angle_attenuation);
    assert_eq!(light.angle_scale, 0.0);
    assert_eq!(light.extra_distance, 0.0);
    assert!((light.intensity - photons / Q3MAP_LIGHTMAP_BYTE_SCALE).abs() < 1e-3);
    assert!((light.radius - photons.sqrt()).abs() < 1e-3);
}

#[test]
fn spawn_selection_follows_jka_rules() {
    let at = |x: f32, initial: bool, no_humans: bool| SpawnPoint {
        position: [x, 0.0, 0.0],
        initial,
        no_humans,
        ..SpawnPoint::default()
    };
    let spawns = [
        at(0.0, false, false),
        at(100.0, true, false),
        at(200.0, false, false),
        at(300.0, false, false),
    ];
    // The initial spot wins for the first spawn, whatever the roll.
    assert_eq!(select_spawn_index(&spawns, [0.0; 3], true, 0.9), Some(1));
    // Bot-only initial spots are skipped, falling back to the furthest half.
    let bot_only = [
        at(0.0, false, false),
        at(100.0, true, true),
        at(200.0, false, false),
        at(300.0, false, false),
    ];
    assert_eq!(select_spawn_index(&bot_only, [0.0; 3], true, 0.0), Some(3));
    // Later spawns pick from the furthest half (300 and 200 from the origin).
    assert_eq!(select_spawn_index(&spawns, [0.0; 3], false, 0.0), Some(3));
    assert_eq!(select_spawn_index(&spawns, [0.0; 3], false, 0.99), Some(2));
    // Avoiding the far end flips the ranking.
    assert_eq!(
        select_spawn_index(&spawns, [300.0, 0.0, 0.0], false, 0.0),
        Some(0)
    );
    assert_eq!(select_spawn_index(&[], [0.0; 3], true, 0.0), None);
    assert_eq!(
        select_spawn_index(&spawns[..1], [0.0; 3], false, 0.5),
        Some(0)
    );
}

#[test]
fn source_map_spawn_yaw_uses_radians() {
    let source = format!(
            "{}\n{{\n\"classname\" \"info_player_deathmatch\"\n\"origin\" \"16 32 48\"\n\"angle\" \"90\"\n}}\n",
            legacy_box_source("test/ceiling")
        );
    let document = jka_assets::map::parse(&source).unwrap();
    let spawns = map_spawn_points(&document);
    assert_eq!(spawns.len(), 1);
    assert!((spawns[0].yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
}

#[test]
fn source_map_can_load_from_loose_base_maps_directory() {
    let root = std::env::temp_dir().join(format!("jka-map-vfs-test-{}", std::process::id()));
    let base = root.join("base");
    std::fs::create_dir_all(base.join("maps")).unwrap();
    std::fs::write(
        base.join("maps/loose_source.map"),
        legacy_box_source("test/ceiling"),
    )
    .unwrap();

    let prepared = prepare_source(&base, None, &MapSource::Map("loose_source".into())).unwrap();
    assert_eq!(prepared.triangles, 12);
    assert!(prepared.source.ends_with("base/maps/loose_source.map"));

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn nonworld_entity_brushes_are_skipped_and_reported() {
    let mut source = legacy_box_source("test/ceiling");
    source.push_str(
        r#"
{
"classname" "func_static"
{
( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) test/wall 0 0 0 1 1
( 192 0 0 ) ( 192 0 64 ) ( 192 64 0 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 128 0 64 ) ( 192 0 0 ) test/wall 0 0 0 1 1
( 128 64 0 ) ( 192 64 0 ) ( 128 64 64 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 192 0 0 ) ( 128 64 0 ) test/floor 0 0 0 1 1
( 128 0 64 ) ( 128 64 64 ) ( 192 0 64 ) test/ceiling 0 0 0 1 1
}
}
"#,
    );
    let document = jka_assets::map::parse(&source).unwrap();
    let root = std::env::temp_dir().join(format!("jka-map-scene-test-{}", std::process::id()));
    let prepared = prepare_map_document(&root, None, Path::new("fixture.map"), document).unwrap();
    let stats = prepared.map_file_stats.unwrap();
    assert_eq!(stats.world_brushes, 1);
    assert_eq!(stats.grouped_world_brushes, 0);
    assert_eq!(stats.skipped_entity_brushes, 1);
    assert_eq!(prepared.triangles, 12);
}

#[test]
fn func_group_brushes_are_folded_into_static_world() {
    let mut source = legacy_box_source("test/ceiling");
    source.push_str(
        r#"
{
"classname" "func_group"
{
( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) test/wall 0 0 0 1 1
( 192 0 0 ) ( 192 0 64 ) ( 192 64 0 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 128 0 64 ) ( 192 0 0 ) test/wall 0 0 0 1 1
( 128 64 0 ) ( 192 64 0 ) ( 128 64 64 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 192 0 0 ) ( 128 64 0 ) test/floor 0 0 0 1 1
( 128 0 64 ) ( 128 64 64 ) ( 192 0 64 ) test/ceiling 0 0 0 1 1
}
}
"#,
    );
    let document = jka_assets::map::parse(&source).unwrap();
    let root = std::env::temp_dir().join(format!("jka-map-scene-test-{}", std::process::id()));
    let prepared = prepare_map_document(&root, None, Path::new("fixture.map"), document).unwrap();
    let stats = prepared.map_file_stats.unwrap();
    assert_eq!(stats.world_brushes, 1);
    assert_eq!(stats.grouped_world_brushes, 1);
    assert_eq!(stats.skipped_entity_brushes, 0);
    assert_eq!(prepared.triangles, 24);
}

#[test]
fn multiple_world_brushes_are_counted() {
    let first = legacy_box_source("test/ceiling");
    let second_brush = r#"{
( 128 0 0 ) ( 128 64 0 ) ( 128 0 64 ) test/wall 0 0 0 1 1
( 192 0 0 ) ( 192 0 64 ) ( 192 64 0 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 128 0 64 ) ( 192 0 0 ) test/wall 0 0 0 1 1
( 128 64 0 ) ( 192 64 0 ) ( 128 64 64 ) test/wall 0 0 0 1 1
( 128 0 0 ) ( 192 0 0 ) ( 128 64 0 ) test/floor 0 0 0 1 1
( 128 0 64 ) ( 128 64 64 ) ( 192 0 64 ) test/ceiling 0 0 0 1 1
}"#;
    let source = first.replacen("\n}\n}", &format!("\n}}\n{second_brush}\n}}"), 1);
    let document = jka_assets::map::parse(&source).unwrap();
    assert_eq!(document.entities[0].brushes.len(), 2);
}

#[test]
fn map_arguments_choose_source_by_extension() {
    assert!(matches!(
        MapSource::from_map_argument("mp/ffa3").unwrap(),
        MapSource::Bsp(name) if name == "mp/ffa3"
    ));
    assert!(matches!(
        MapSource::from_map_argument("mp/ffa3.bsp").unwrap(),
        MapSource::Bsp(name) if name == "mp/ffa3"
    ));
    assert!(matches!(
        MapSource::from_map_argument("maps/mp/ffa3.map").unwrap(),
        MapSource::Map(name) if name == "mp/ffa3"
    ));
    assert!(matches!(
        MapSource::from_map_argument("mp\\ffa3.map").unwrap(),
        MapSource::Map(name) if name == "mp/ffa3"
    ));
    assert!(MapSource::from_map_argument("mp/ffa3.pk3").is_err());
    assert!(MapSource::from_map_argument("../ffa3.map").is_err());
}

#[test]
fn map_source_preflight_checks_asset_existence() {
    let root = std::env::temp_dir().join(format!("jka-map-preflight-test-{}", std::process::id()));
    let base = root.join("base");
    std::fs::create_dir_all(base.join("maps")).unwrap();
    std::fs::write(
        base.join("maps/present.bsp"),
        b"not parsed during preflight",
    )
    .unwrap();

    assert!(verify_map_source_exists(&base, None, &MapSource::Bsp("present".into())).is_ok());
    assert!(verify_map_source_exists(&base, None, &MapSource::Bsp("missing".into())).is_err());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dynamic_light_entities_are_bounded_and_finite() {
    let light =
        dynamic_light_from_values("light", Some("1 2 3"), Some("1.2 -0.5 0.4"), Some("600"))
            .unwrap();
    assert_eq!(light.position, render_position([1.0, 2.0, 3.0]));
    assert_eq!(light.color, [1.2, 0.0, 0.4]);
    assert_eq!(light.radius, 600.0);
    assert_eq!(light.intensity, 2.0);
    assert!(dynamic_light_from_values("light", Some("NaN 0 0"), None, None).is_none());
    assert!(
        dynamic_light_from_values("info_player_deathmatch", Some("0 0 0"), None, None).is_none()
    );
}
