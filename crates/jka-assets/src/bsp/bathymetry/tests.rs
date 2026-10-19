use super::*;

#[test]
#[ignore = "requires BLUE_WATER_TEST_BASE with costadelsol.bsp"]
fn upstream_coast_real_map_sky_corners_are_open_sea() {
    let root = std::path::PathBuf::from(std::env::var("BLUE_WATER_TEST_BASE").expect("asset directory"));
    let mut assets = crate::pk3::AssetSearchPath::open(&root).unwrap();
    let bytes = assets.read("maps/costadelsol.bsp", super::super::MAX_FILE_BYTES).unwrap().unwrap();
    let bsp = Bsp::parse(&bytes.bytes).unwrap();
    let geometry = Geometry::from_bsp(&bsp).unwrap();
    for origin in [[-14592.,-14592.],[-14592.,14080.],[14080.,-14592.],[14080.,14080.]] {
        let heights = geometry.upstream_shore_heights(origin,[5,5],128.,3.,&[]).unwrap();
        println!("Sky corner {origin:?}: {heights:?}");
        assert!(heights.iter().all(|h| h.is_nan() || *h < -100.),"sky shell is not land/shallow surf: {origin:?}");
    }
}

#[test]
fn upstream_coast_keeps_sky_shell_open_and_real_banks_solid() {
    let mut sky_wall = box_solid([0.,-100.,-100.],[10.,100.,100.]);
    sky_wall.sky[0] = true; // Interior face of a caulk-topped vertical sky shell.
    let geometry = Geometry {
        solids: vec![sky_wall, box_solid([-100.,-100.,-100.],[-10.,100.,15.])],
        triangles:vec![], liquids:vec![],
    };
    let samples = geometry.upstream_shore_heights([-20.,0.],[31,2],1.,10.,&[]).unwrap();
    assert_eq!(samples[0],5.,"actual bank stays land");
    assert!(samples[25].is_nan(),"sky collision shell must not become a surf bank or wind blocker");
    assert!(geometry.column([5.,0.],10.,&[0],&[]).bed.unwrap()>10.,"existing solver geometry is preserved");
    let mut floor = box_solid([-100.,-100.,-100.],[100.,100.,5.]);
    floor.sky[0]=true; // A distant sky side does not erase the actual floor below water.
    let geometry = Geometry {solids:vec![floor],triangles:vec![],liquids:vec![]};
    assert_eq!(geometry.upstream_shore_heights([0.,0.],[2,2],1.,10.,&[]).unwrap()[0],-5.);
}

#[test]
fn upstream_coast_raster_respects_water_level_roofs_and_unknown_columns() {
    let geometry=Geometry {
        solids:vec![box_solid([-100.,-100.,-100.],[100.,100.,5.]),
            box_solid([0.5,-100.,-100.],[100.,100.,15.]),
            box_solid([-100.,-100.,12.],[0.,100.,14.])],
        triangles:vec![],liquids:vec![],
    };
    let footprint=rectangle([-100.,-100.],[100.,100.]);
    let heights=geometry.upstream_shore_heights([-1.,0.],[3,2],1.,10.,&footprint).unwrap();
    assert_eq!(heights[0],-5.,"an overhead roof must not become the seabed");
    assert_eq!(heights[2],5.,"solid land crossing the water plane must shelter waves");
    let upper=geometry.upstream_shore_heights([1.,0.],[2,2],1.,20.,&footprint).unwrap();
    assert_eq!(upper[0],-5.,"each pool must use its own still-water level");
    let unknown=geometry.upstream_shore_heights([200.,200.],[2,2],1.,10.,&footprint).unwrap();
    assert!(unknown.iter().all(|h|h.is_nan()),"missing geometry is unknown, never a guessed bank");
    let terrain=Geometry {solids:vec![],liquids:vec![],triangles:vec![Triangle {
        points:[[0.,0.,15.],[10.,0.,15.],[0.,10.,15.]],normal:[0.,0.,1.],bounds:[[0.,0.],[10.,10.]],
    }]};
    let bank=terrain.upstream_shore_heights([1.,1.],[2,2],1.,10.,&[]).unwrap();
    assert_eq!(bank[0],5.,"brush-free triangle terrain above water must provide land shelter");
}

fn large_pool_tiles() -> Vec<Region> {
    let footprint = rectangle([0.0, 0.0], [2048.0, 2048.0]);
    (0..8)
        .flat_map(|y| {
            let footprint = &footprint;
            (0..8).map(move |x| Region {
                slot: 2,
                level: 0.0,
                triangles: footprint.clone(),
                bounds: Some([
                    [x as f32 * 256.0, y as f32 * 256.0],
                    [(x + 1) as f32 * 256.0, (y + 1) as f32 * 256.0],
                ]),
            })
        })
        .collect()
}

#[test]
fn shoreline_bands_keep_internal_bank_and_match_complete_bake() {
    let mut geometry = Geometry {
        solids: vec![box_solid([-256., -256., -256.], [2304., 2304., -128.])],
        triangles: vec![],
        liquids: vec![],
    };
    let mut bank = box_solid([720., 720., -256.], [820., 820., 64.]);
    bank.planes[4] = Plane {
        normal: [-1., 0., 1.],
        distance: -800.,
    };
    geometry.solids.push(bank);
    let tiles = large_pool_tiles();
    let options = Options {
        max_cells_per_region: 4096,
        ..Default::default()
    };
    let complete = geometry.bake(&tiles, options).unwrap();
    let screened = geometry
        .bake_shoreline_bands(&tiles, options, 38.4)
        .unwrap();
    assert_eq!(
        screened.grids.len(),
        16,
        "interior bank plus diagonal halo, not the outer pool envelope"
    );
    assert_eq!(screened.shoreline_bands[0].full_tiles, 64);
    for reference in &complete.grids {
        let matching = screened.grids.iter().find(|g| g.origin == reference.origin);
        if reference
            .cells
            .iter()
            .any(|c| c.class == Class::Wet && c.depth <= 38.4)
        {
            assert!(
                matching.is_some(),
                "every shallow tile must survive screening"
            );
        }
        if let Some(g) = matching {
            for (a, b) in g.cells.iter().zip(&reference.cells) {
                assert_eq!(a.class, b.class);
                assert_eq!(a.bed.to_bits(), b.bed.to_bits());
                assert_eq!(a.depth.to_bits(), b.depth.to_bits());
            }
            // The bank spans four core tiles. Its stitched faces must exactly
            // match the complete atlas; only the distant halo cuts may differ.
            if (0..2).all(|a| g.origin[a] >= 512. && g.origin[a] <= 768.) {
                for (a, b) in g
                    .faces_x
                    .iter()
                    .chain(&g.faces_y)
                    .zip(reference.faces_x.iter().chain(&reference.faces_y))
                {
                    assert_eq!(a.boundary, b.boundary);
                    assert_eq!(a.crest.to_bits(), b.crest.to_bits());
                }
            }
        }
    }
    let roundtrip = Atlas::decode(&screened.encode([9, 8]), [9, 8]).unwrap();
    assert_eq!(roundtrip.shoreline_bands[0].retained_tiles, 16);
    assert_eq!(roundtrip.shoreline_bands[0].max_depth, 38.4);

    let mut broken = tiles.clone();
    broken
        .iter_mut()
        .find(|r| r.bounds.unwrap()[0] == [512., 512.])
        .unwrap()
        .triangles
        .clear();
    let rejected = geometry
        .bake_shoreline_bands(&broken, options, 38.4)
        .unwrap();
    assert!(
        rejected.grids.is_empty(),
        "a failed required tile rejects the whole pool"
    );
    assert!(rejected.shoreline_bands.is_empty());
    assert!(rejected
        .warnings
        .iter()
        .any(|w| w.contains("incomplete shoreline-band bake")));
    assert!(Atlas::decode(&rejected.encode([9, 8]), [9, 8]).is_ok());
}

#[test]
fn shoreline_screening_keeps_subcell_terrain_and_rejects_over_budget_band() {
    let mut geometry = Geometry {
        solids: vec![],
        liquids: vec![],
        triangles: vec![Triangle {
            points: [[767.9, 750., -1.], [768.1, 750., -1.], [768., 750.1, -1.]],
            normal: [0., 0., 1.],
            bounds: [[767.9, 750.], [768.1, 750.1]],
        }],
    };
    let tiles = large_pool_tiles();
    let options = Options {
        max_cells_per_region: 4096,
        ..Default::default()
    };
    let narrow = geometry
        .bake_shoreline_bands(&tiles, options, 38.4)
        .unwrap();
    assert_eq!(
        narrow.grids.len(),
        12,
        "tiny bank across a tile seam cannot vanish between coarse probes"
    );
    geometry
        .solids
        .push(box_solid([-256., -256., -256.], [2304., 2304., -16.]));
    let broad = geometry
        .bake_shoreline_bands(&tiles, options, 38.4)
        .unwrap();
    assert!(
        broad.grids.is_empty(),
        "do not silently truncate an oversized shallow domain"
    );
    assert!(broad.warnings[0].contains("no geometric bank crossings"));
}

#[test]
fn oversized_shallows_keep_all_internal_bank_seeds_with_bounded_patches() {
    let mut bank = box_solid([720., 720., -256.], [820., 820., 64.]);
    bank.planes[4] = Plane {
        normal: [-1., 0., 1.],
        distance: -800.,
    };
    let geometry = Geometry {
        solids: vec![box_solid([-256., -256., -256.], [2304., 2304., -16.]), bank],
        triangles: vec![],
        liquids: vec![],
    };
    let tiles = large_pool_tiles();
    let options = Options {
        max_cells_per_region: 4096,
        ..Default::default()
    };
    let complete = geometry.bake(&tiles, options).unwrap();
    let patch = geometry
        .bake_shoreline_bands(&tiles, options, 38.4)
        .unwrap();
    let band = &patch.shoreline_bands[0];
    assert_eq!(band.patch_radius_cells, 8);
    assert_eq!(band.full_tiles, 64);
    assert!(patch.grids.len() * 256 <= options.max_cells_per_region);
    for grid in &complete.grids {
        if grid
            .faces_x
            .iter()
            .chain(&grid.faces_y)
            .any(|f| f.boundary == Boundary::Bank)
        {
            let retained = patch
                .grids
                .iter()
                .find(|g| g.origin == grid.origin)
                .expect("every bank seed must survive, including internal banks");
            for (a, b) in grid.cells.iter().zip(&retained.cells) {
                assert_eq!(a.class, b.class);
                assert_eq!(a.bed.to_bits(), b.bed.to_bits());
            }
        }
    }
    let decoded = Atlas::decode(&patch.encode([4, 5]), [4, 5]).unwrap();
    assert_eq!(decoded.shoreline_bands[0].patch_radius_cells, 8);
    let mut invalid = patch.clone();
    invalid.shoreline_bands[0].patch_radius_cells = 33;
    assert!(Atlas::decode(&invalid.encode([4, 5]), [4, 5]).is_err());
    assert!(
        !patch.grids.iter().any(|g| g.origin == [0., 0.]),
        "offshore shallows keep FFT"
    );
    let mut broken = tiles.clone();
    broken
        .iter_mut()
        .find(|r| r.bounds.unwrap()[0] == patch.grids[0].origin)
        .unwrap()
        .triangles
        .clear();
    let rejected = geometry
        .bake_shoreline_bands(&broken, options, 38.4)
        .unwrap();
    assert!(
        rejected.grids.is_empty() && rejected.shoreline_bands.is_empty(),
        "a failed required patch tile rejects every patch atomically"
    );
}

#[test]
fn oversized_bank_patch_set_rejects_every_bank_atomically() {
    let mut bank = box_solid([-256., -256., -256.], [2304., 2304., 256.]);
    bank.planes[4] = Plane {
        normal: [-0.1, -0.1, 1.0],
        distance: -204.8,
    };
    let geometry = Geometry {
        solids: vec![bank],
        triangles: vec![],
        liquids: vec![],
    };
    let atlas = geometry
        .bake_shoreline_bands(
            &large_pool_tiles(),
            Options {
                max_cells_per_region: 4096,
                ..Default::default()
            },
            38.4,
        )
        .unwrap();
    assert!(atlas.grids.is_empty() && atlas.shoreline_bands.is_empty());
    assert!(atlas.warnings[0].contains("all bank-seeded patches also exceed"));
}

#[test]
fn large_deep_wall_pool_stays_fft_without_allocating_the_interior() {
    let geometry = Geometry {
        solids: vec![
            box_solid([-256., -256., -256.], [2304., 2304., -128.]),
            box_solid([1023.99, -256., -256.], [1024.01, 2304., 128.]),
        ],
        triangles: vec![],
        liquids: vec![],
    };
    let atlas = geometry
        .bake_shoreline_bands(
            &large_pool_tiles(),
            Options {
                max_cells_per_region: 4096,
                ..Default::default()
            },
            38.4,
        )
        .unwrap();
    assert!(atlas.grids.is_empty());
    assert!(atlas.warnings[0].contains("FFT-only"));
    assert!(atlas.shoreline_bands.is_empty());
}

fn box_solid(min: [f32; 3], max: [f32; 3]) -> Solid {
    Solid {
        planes: vec![
            Plane {
                normal: [1., 0., 0.],
                distance: max[0],
            },
            Plane {
                normal: [-1., 0., 0.],
                distance: -min[0],
            },
            Plane {
                normal: [0., 1., 0.],
                distance: max[1],
            },
            Plane {
                normal: [0., -1., 0.],
                distance: -min[1],
            },
            Plane {
                normal: [0., 0., 1.],
                distance: max[2],
            },
            Plane {
                normal: [0., 0., -1.],
                distance: -min[2],
            },
        ],
        sky: vec![false; 6],
        bounds: [[min[0], min[1]], [max[0], max[1]]],
    }
}
fn rectangle(min: [f32; 2], max: [f32; 2]) -> Vec<FootprintTriangle> {
    vec![[min, [max[0], min[1]], max], [min, max, [min[0], max[1]]]]
}
fn region() -> Region {
    Region {
        slot: 2,
        level: 0.0,
        triangles: rectangle([0., 0.], [64., 64.]),
        bounds: None,
    }
}
fn basin() -> Geometry {
    Geometry {
        solids: vec![box_solid([-64., -64., -128.], [128., 128., -16.])],
        triangles: vec![],
        liquids: vec![],
    }
}
fn bake(g: Geometry, r: Region) -> Grid {
    g.bake(&[r], Options::default()).unwrap().grids.remove(0)
}
fn cell(g: &Grid, p: [f32; 2]) -> Cell {
    let x = ((p[0] - g.origin[0]) / g.cell_size) as usize;
    let y = ((p[1] - g.origin[1]) / g.cell_size) as usize;
    g.cells[y * g.size[0] as usize + x]
}

#[test]
fn analytic_basin_has_real_depth_and_one_component() {
    let grid = bake(basin(), region());
    let c = cell(&grid, [24., 24.]);
    assert_eq!(c.class, Class::Wet);
    assert_eq!(c.depth, 16.);
    assert_eq!(c.bed, -16.);
    assert_eq!(grid.components, 1);
    assert!(grid
        .cells
        .iter()
        .filter(|c| c.class == Class::Wet)
        .all(|c| c.depth == 16.));
}

#[test]
fn water_and_clip_brushes_are_never_seabed() {
    use crate::bsp::{Brush, BrushSide, Model, Shader};
    for contents in [CONTENTS_WATER, CONTENTS_WATER | CONTENTS_SOLID, 0x10] {
        let solid = box_solid([-100., -100., -128.], [100., 100., -16.]);
        let bsp = Bsp {
            collision: None,
            visibility: None,
            shaders: vec![Shader {
                name: b"water".to_vec(),
                surface_flags: 0,
                contents,
            }],
            fogs: vec![],
            vertices: vec![],
            indices: vec![],
            surfaces: vec![],
            models: vec![Model {
                mins: [-100., -100., -128.],
                maxs: [100., 100., 0.],
                surfaces: 0..0,
                brushes: 0..1,
            }],
            planes: solid.planes,
            brushes: vec![Brush {
                sides: 0..6,
                shader: 0,
            }],
            brush_sides: (0..6).map(|plane| BrushSide { plane, shader: 0 }).collect(),
            entities: vec![],
            lightmaps: vec![],
            light_grid: None,
        };
        let geometry = Geometry::from_bsp(&bsp).unwrap();
        let grid = bake(geometry, region());
        let c = cell(&grid, [24., 24.]);
        assert_eq!(c.class, Class::NoFloor);
        assert!(c.bed.is_nan());
        assert!(c.depth.is_nan());
    }
}

#[test]
fn sloping_convex_ground_uses_plane_intersection() {
    let mut g = basin();
    g.solids[0].planes[4] = Plane {
        normal: [-0.25, 0., 1.],
        distance: -16.,
    };
    g.solids[0].bounds = [[-64., -64.], [128., 128.]];
    let grid = bake(g, region());
    let a = cell(&grid, [8., 24.]);
    let b = cell(&grid, [40., 24.]);
    assert_eq!(a.bed, -14.);
    assert_eq!(b.bed, -6.);
    assert_eq!(a.depth, 14.);
    assert_eq!(b.depth, 6.);
}

#[test]
fn subcell_wall_splits_wet_connectivity_in_deep_water() {
    let mut g = basin();
    g.solids
        .push(box_solid([31.99, -64., -128.], [32.01, 128., 64.]));
    let grid = bake(g, region());
    // Wall sits between all centers and inset corner probes.
    assert_eq!(cell(&grid, [24., 24.]).class, Class::Wet);
    assert_eq!(cell(&grid, [40., 24.]).class, Class::Wet);
    assert_ne!(
        cell(&grid, [24., 24.]).component,
        cell(&grid, [40., 24.]).component
    );
    assert_eq!(grid.components, 2);
    let x = ((32. - grid.origin[0]) / grid.cell_size) as usize;
    let y = ((24. - grid.origin[1]) / grid.cell_size) as usize;
    assert_eq!(
        grid.faces_x[y * (grid.size[0] as usize + 1) + x].boundary,
        Boundary::Wall
    );
}

#[test]
fn sloped_waterline_is_an_overtoppable_bank_not_a_wall() {
    let mut g = basin();
    g.solids[0].planes[4] = Plane {
        normal: [-1.0, 0.0, 1.0],
        distance: -16.0,
    };
    let grid = bake(g, region());
    assert_eq!(cell(&grid, [8.0, 24.0]).class, Class::Wet);
    assert_eq!(cell(&grid, [24.0, 24.0]).class, Class::Dry);
    let x = ((16.0 - grid.origin[0]) / grid.cell_size) as usize;
    let y = ((24.0 - grid.origin[1]) / grid.cell_size) as usize;
    assert_eq!(
        grid.faces_x[y * (grid.size[0] as usize + 1) + x].boundary,
        Boundary::Bank
    );
}

#[test]
fn exact_triangle_union_does_not_bridge_a_narrow_gap() {
    let mut triangles = rectangle([0., 0.], [29., 64.]);
    triangles.extend(rectangle([30., 0.], [64., 64.]));
    assert!(!segment_covered(
        [24., 24.],
        [40., 24.],
        triangles.iter().copied()
    ));
    let grid = bake(
        basin(),
        Region {
            triangles,
            ..region()
        },
    );
    assert_ne!(
        cell(&grid, [24., 24.]).component,
        cell(&grid, [40., 24.]).component
    );
}

#[test]
fn footprint_hole_and_separate_heights_are_preserved() {
    let mut triangles = rectangle([0., 0.], [16., 64.]);
    triangles.extend(rectangle([48., 0.], [64., 64.]));
    let grid = bake(
        basin(),
        Region {
            triangles,
            ..region()
        },
    );
    assert_eq!(cell(&grid, [24., 24.]).class, Class::Outside);
    let g = basin();
    let mut high = region();
    high.slot = 3;
    high.level = 8.;
    let atlas = g.bake(&[region(), high], Options::default()).unwrap();
    assert_eq!(cell(&atlas.grids[0], [24., 24.]).depth, 16.);
    assert_eq!(cell(&atlas.grids[1], [24., 24.]).depth, 24.);
}

#[test]
fn ceilings_are_retained_and_submerged_stacked_floors_get_fallback() {
    for (bottom, top) in [(4., 8.), (-8., -4.)] {
        let mut g = basin();
        g.solids
            .push(box_solid([-64., -64., bottom], [128., 128., top]));
        let grid = bake(g, region());
        let c = cell(&grid, [24., 24.]);
        if bottom > 0. {
            assert_eq!(c.class, Class::Wet);
            assert_eq!(c.bed, -16.);
            assert_eq!(c.ceiling, 4.);
        } else {
            assert_eq!(c.class, Class::Overhang);
            assert_eq!(c.component, 0);
        }
    }
}

#[test]
fn cache_checks_identity_integrity_and_roundtrips_unknown_depth() {
    let atlas = Geometry {
        solids: vec![],
        triangles: vec![],
        liquids: vec![],
    }
    .bake(&[region()], Options::default())
    .unwrap();
    let bytes = atlas.encode([123, 456]);
    let decoded = Atlas::decode(&bytes, [123, 456]).unwrap();
    assert!(cell(&decoded.grids[0], [24., 24.]).depth.is_nan());
    assert_eq!(decoded.grids[0].footprint, region().triangles);
    assert!(Atlas::decode(&bytes, [123, 457]).is_err());
    let mut corrupt = bytes.clone();
    corrupt[60] ^= 1;
    assert!(Atlas::decode(&corrupt, [123, 456]).is_err());
    assert!(Atlas::decode(&bytes[..bytes.len() / 2], [123, 456]).is_err());
}

#[test]
fn water_volume_selects_layer_without_becoming_a_depth_limit() {
    let mut g = basin();
    g.solids
        .push(box_solid([-64., -64., -256.], [128., 128., -192.]));
    // The nominal water brush bottom sits ABOVE the real seabed. Depth must
    // still come from solid geometry, and the lower room is another layer.
    g.liquids
        .push(box_solid([-64., -64., -8.], [128., 128., 0.]));
    let grid = bake(g, region());
    let c = cell(&grid, [24., 24.]);
    assert_eq!(c.class, Class::Wet);
    assert_eq!(c.depth, 16.);
}

#[test]
fn nodraw_terrain_patch_is_bed_and_triangle_wall_seals_the_face() {
    use crate::bsp::{Model, Shader, Surface, Vertex, SURF_NODRAW};
    let vertices = (0..3)
        .flat_map(|y| {
            (0..3).map(move |x| Vertex {
                position: [x as f32 * 32., y as f32 * 32., -10.],
                normal: [0., 0., 1.],
                texcoord: [0.; 2],
                lightmap_uv: [[0.; 2]; 4],
                color: [[255; 4]; 4],
            })
        })
        .collect();
    let bsp = Bsp {
        collision: None,
        visibility: None,
        shaders: vec![Shader {
            name: b"terrain".to_vec(),
            surface_flags: SURF_NODRAW,
            contents: CONTENTS_TERRAIN,
        }],
        fogs: vec![],
        vertices,
        indices: vec![],
        surfaces: vec![Surface {
            shader: 0,
            fog_num: -1,
            kind: SurfaceKind::Patch,
            flare: None,
            vertices: 0..9,
            indices: 0..0,
            lightmaps: [-1; 4],
            lightmap_styles: [0; 4],
            vertex_styles: [0; 4],
            patch_size: [3, 3],
            patch_lod_bounds: [[0.0;3];2],
        }],
        models: vec![Model {
            mins: [0., 0., -10.],
            maxs: [64., 64., 0.],
            surfaces: 0..1,
            brushes: 0..0,
        }],
        planes: vec![],
        brushes: vec![],
        brush_sides: vec![],
        entities: vec![],
        lightmaps: vec![],
        light_grid: None,
    };
    let mut geometry = Geometry::from_bsp(&bsp).unwrap();
    let grid = geometry
        .bake(&[region()], Options::default())
        .unwrap()
        .grids
        .remove(0);
    assert_eq!(cell(&grid, [24., 24.]).bed, -10.);
    let a = [32., -64., -64.];
    let b = [32., 128., -64.];
    let c = [32., 128., 64.];
    let d = [32., -64., 64.];
    for points in [[a, b, c], [a, c, d]] {
        geometry.triangles.push(Triangle {
            points,
            normal: [1., 0., 0.],
            bounds: bounds(points.iter().map(|p| [p[0], p[1]])),
        });
    }
    let grid = bake(geometry, region());
    assert_ne!(
        cell(&grid, [24., 24.]).component,
        cell(&grid, [40., 24.]).component
    );
}

#[test]
fn oversized_grid_is_rejected_without_coarsening_or_guessing_depth() {
    let r = Region {
        triangles: rectangle([0., 0.], [100000., 100000.]),
        ..region()
    };
    let atlas = basin().bake(&[r], Options::default()).unwrap();
    assert!(atlas.grids.is_empty());
    assert_eq!(atlas.warnings.len(), 1);
    assert!(atlas.warnings[0].contains("retain legacy water"));
}

#[test]
fn sparse_l_tiles_skip_the_empty_envelope_corner_and_stitch_open_faces() {
    let mut exact = rectangle([0.0, 0.0], [512.0, 256.0]);
    exact.extend(rectangle([0.0, 256.0], [256.0, 512.0]));
    let tile = |bounds: [[f32; 2]; 2]| Region {
        slot: 4,
        level: 0.0,
        triangles: exact.clone(),
        bounds: Some(bounds),
    };
    let geometry = Geometry {
        solids: vec![box_solid([-64.0, -64.0, -128.0], [576.0, 576.0, -16.0])],
        triangles: vec![],
        liquids: vec![],
    };
    let atlas = geometry
        .bake(
            &[
                tile([[0.0, 0.0], [256.0, 256.0]]),
                tile([[0.0, 256.0], [256.0, 512.0]]),
                tile([[256.0, 0.0], [512.0, 256.0]]),
            ],
            Options::default(),
        )
        .unwrap();
    assert_eq!(atlas.grids.len(), 3);
    assert_eq!(
        atlas.grids.iter().map(|g| g.cells.len()).sum::<usize>(),
        768
    );
    let lower_left = atlas.grids.iter().find(|g| g.origin == [0.0, 0.0]).unwrap();
    let nx = lower_left.size[0] as usize;
    let ny = lower_left.size[1] as usize;
    assert_eq!(
        lower_left.faces_x[8 * (nx + 1) + nx].boundary,
        Boundary::Open
    );
    assert_eq!(lower_left.faces_y[ny * nx + 8].boundary, Boundary::Open);
}

#[test]
fn water_volume_shaders_keep_brush_and_top_plane_sheets_only() {
    use crate::bsp::{Brush, BrushSide, Model, Shader};
    let shader = |name: &[u8], contents| Shader { name: name.to_vec(), surface_flags: 0, contents };
    let solid = box_solid([-100., -100., -128.], [100., 100., -16.]);
    let bsp = Bsp {
        collision: None,
        visibility: None,
        shaders: vec![
            shader(b"caulk_water", CONTENTS_WATER),
            shader(b"oceanwater", CONTENTS_WATER),
            // seph_surf: water contents in the table, never on a brush.
            shader(b"seph_surf", CONTENTS_WATER),
            shader(b"rock", CONTENTS_SOLID),
            // Water-contents sheet lying exactly on the brush top (z = -16).
            shader(b"lakewater", CONTENTS_WATER),
        ],
        fogs: vec![],
        vertices: [-16., -16., -16., -14., -14., -14.]
            .into_iter()
            .enumerate()
            .map(|(i, z)| crate::bsp::Vertex {
                position: [(i % 3) as f32 * 40., (i % 3) as f32 * 10., z],
                texcoord: [0.; 2],
                lightmap_uv: [[0.; 2]; 4],
                normal: [0., 0., 1.],
                color: [[255; 4]; 4],
            })
            .collect(),
        indices: vec![],
        // lakewater on the top plane; seph_surf two units above it.
        surfaces: [(4usize, 0usize..3usize), (2, 3..6)]
            .into_iter()
            .map(|(shader, vertices)| crate::bsp::Surface {
                shader,
                fog_num: -1,
                kind: crate::bsp::SurfaceKind::Patch,
                flare: None,
                vertices,
                indices: 0..0,
                lightmaps: [-1; 4],
                lightmap_styles: [255; 4],
                vertex_styles: [255; 4],
                patch_size: [0; 2],
                patch_lod_bounds: [[0.; 3]; 2],
            })
            .collect(),
        models: vec![Model {
            mins: [-100., -100., -128.],
            maxs: [100., 100., 0.],
            surfaces: 0..0,
            brushes: 0..2,
        }],
        planes: solid.planes,
        brushes: vec![
            Brush { sides: 0..6, shader: 0 },
            // A solid brush's sides never make a shader a water volume.
            Brush { sides: 0..6, shader: 3 },
        ],
        brush_sides: (0..6)
            .map(|plane| BrushSide { plane, shader: if plane == 4 { 1 } else { 0 } })
            .collect(),
        entities: vec![],
        lightmaps: vec![],
        light_grid: None,
    };
    let volume = water_volume_shaders(&bsp);
    // 0/1 are on a water brush, 4 lies on its top plane; seph_surf (2) floats above it.
    assert_eq!(volume.into_iter().collect::<Vec<_>>(), vec![0, 1, 4]);
}
