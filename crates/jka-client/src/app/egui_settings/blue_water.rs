//! Blue Water backend selection and the controls currently consumed by the port.
use crate::app::egui_settings::{theme, App};

impl App {
    pub(in crate::app::egui_settings) fn egui_lakes(&mut self, ui: &mut egui::Ui) {
        theme::section(ui, "BLUE WATER LAKES & PONDS", "Half-Life 2 (Source engine) water: distorted refraction and reflection blended by Fresnel, with depth fog, over gentle moving relief. Each pool keeps its map level and shore outline; sky-connected water can continue to the horizon.");
        let mut settings = self.video.ocean_settings;
        let d = crate::ocean::LakeSettings::default();
        let mut changed = false;
        theme::row(ui, "Source shader hints", "When enabled, each automatically promoted pool inherits color, amplitude, roughness and movement cues from its original JKA shader. The color cue only applies while Water color is at its default; a Water color you pick is used as is. Off restores the shared Lake appearance for A/B tests.", theme::Reset::None, |ui| {
            if theme::chip(ui, "On", settings.lake.source_hints).clicked() { settings.lake.source_hints = true; changed = true; }
            if theme::chip(ui, "Off", !settings.lake.source_hints).clicked() { settings.lake.source_hints = false; changed = true; }
        });
        super::ocean_slider_d(ui, "Lake amplitude (inches)", "Maximum movement above and below the map level. Zero draws each pool as one flat quad instead of the relief mesh; the normal maps stay animated.", &mut settings.lake.amplitude, d.amplitude, 0.0..=12.0, &mut changed);
        super::ocean_slider_d(ui, "Lake movement speed", "How quickly the surface rolls and changes shape, and the normal-map animation rate (1 = HL2). Zero freezes both. No FFT simulation is needed.", &mut settings.lake.speed, d.speed, 0.0..=3.0, &mut changed);
        super::ocean_slider_d(ui, "Lake relief direction", "Direction the surface relief drifts, in degrees from +X toward world +Z.", &mut settings.lake.direction, d.direction, -180.0..=180.0, &mut changed);
        theme::row(ui, "Water type", "Canal: HL2's water_canals_water2 (animated normal, 128-unit tiles, strong refraction). Open water: HL2's water_coast01 (static normal, three scrolling layers, 768-unit tiles, gentle refraction). r_lakeSourceStyle.", theme::Reset::None, |ui| {
            for (mode, label) in [(0, "Canal"), (1, "Open water")] {
                if theme::chip(ui, label, settings.lake.source_style == mode).clicked() { settings.lake.source_style = mode; changed = true; }
            }
        });
        super::ocean_slider_d(ui, "Cubemap fade (m)", "HL2 cheap water: beyond this distance reflections fade to the sky cubemap, fully at 1.5x (HL2 canals: 2000/3000 units = 50/75 m). 0 keeps real reflections everywhere. r_lakeSourceCubemapFade.", &mut settings.lake.source_cubemap_fade, d.source_cubemap_fade, 0.0..=500.0, &mut changed);
        super::ocean_slider_d(ui, "Fog distance (feet)", "One distance for above and below the surface. From above: underwater distance along the view ray where the bed reaches the fog color (Source $fogend); also the depth below which submerged geometry is culled from above. Underwater (Source effects): where the fog reaches 95%, thickening smoothly like the other underwater modes.", &mut settings.lake.opaque_depth, d.opaque_depth, 0.25..=100.0, &mut changed);
        super::ocean_slider_d(ui, "Distortion", "Scales the HL2 material's $refractamount/$reflectamount (canal 2.1/0.3, open water 0.5/0.25). The default 0.026 is exactly HL2. Fades out in shallow water.", &mut settings.lake.cheap_refraction, d.cheap_refraction, 0.0..=0.05, &mut changed);
        super::color_row(ui, "Water color", "The lake's color, one setting for above and below the surface: what deep or distant water fades to seen from above (Source $fogcolor), and the underwater fog in every underwater mode. r_lakeDeepColor.", theme::Reset::None, &mut settings.lake.deep_color, &mut changed);
        let source_underwater = settings.lake.underwater_mode == crate::ocean::SOURCE_UNDERWATER_MODE;
        if !source_underwater {
            super::ocean_slider_d(ui, "Underwater visibility", "How far you can see while submerged, for both Godot fog and Fluid Ninja (also sets their cull distance). Higher is clearer.", &mut settings.lake.transparency, d.transparency, 0.1..=10.0, &mut changed);
        }
        theme::row(ui, "Underwater effects", "Godot fog is the shared Hybrid submerged fog, in the lake's Water color. Fluid Ninja uses the Ninja river's underwater look: distortion, distance tint, waterline meniscus and caustics. Source is HL2's underwater view from the chosen material: the surface seen from below, the water's distance fog, and on Open water the screen warp. Each costs only while the camera is underwater. r_lakeUnderwaterMode.", theme::Reset::None, |ui| {
            for (mode, label) in [(4, "Godot fog"), (crate::ocean::NINJA_UNDERWATER_MODE, "Fluid Ninja"), (crate::ocean::SOURCE_UNDERWATER_MODE, "Source")] {
                if mode == crate::ocean::NINJA_UNDERWATER_MODE && !jka_ninja::INCLUDED { continue; }
                if theme::chip(ui, label, settings.lake.underwater_mode == mode).clicked() { settings.lake.underwater_mode = mode; changed = true; }
            }
        });
        if jka_ninja::INCLUDED && source_underwater {
            theme::row(ui, "Underwater caustics", "With Source underwater effects: the Fluid Ninja caustics on the lakebed and everything else below the surface while you are underwater, dimmed by the water fog. r_lakeUnderwaterCaustics.", theme::Reset::None, |ui| {
                if let Some(on) = theme::switch(ui, settings.lake.underwater_caustics) { settings.lake.underwater_caustics = on; changed = true; }
            });
        }
        if jka_ninja::INCLUDED {
            theme::row(ui, "Caustics", "The Fluid Ninja ocean's animated caustics on the lakebed seen through the water, dimmed by depth and faded out past 30-50 m. Your Fluid Ninja wake distorts and brightens them. r_lakeCaustics.", theme::Reset::None, |ui| {
                if let Some(on) = theme::switch(ui, settings.lake.caustics) { settings.lake.caustics = on; changed = true; }
            });
            theme::row(ui, "Fluid Ninja wake", "Players moving through the lake push a Fluid Ninja water simulation (the Fluid Ninja Lake demo) that follows the camera: ripples and foam trails behind you. Off frees it. Needs Water wake (Effects) not Off. r_lakeNinjaWake.", theme::Reset::None, |ui| {
                if theme::chip(ui, "On", settings.lake_ninja_wake).clicked() { settings.lake_ninja_wake = true; changed = true; }
                if theme::chip(ui, "Off", !settings.lake_ninja_wake).clicked() { settings.lake_ninja_wake = false; changed = true; }
            });
        }
        theme::row(ui, "Lake reflection source", "Independent of global reflection settings. Sky uses the environment. SSR traces the visible scene and falls back to sky on misses. Planar renders a reduced-resolution world view and can include off-screen objects.", theme::Reset::None, |ui| {
            for (mode,label) in [(0,"Sky"),(1,"SSR"),(2,"Planar")] {
                if theme::chip(ui,label,settings.lake.reflection_mode == mode).clicked() { settings.lake.reflection_mode=mode;changed=true; }
            }
        });
        if theme::ghost_button(ui, "RESET LAKE").clicked() {
            settings.lake = d;
            changed = true;
        }
        if changed {self.video.ocean_settings=settings;self.commit_ocean_settings();}
    }

    pub(in crate::app::egui_settings) fn egui_blue_water(&mut self, ui: &mut egui::Ui) {
        let hybrid = self.video.ocean_settings.backend == 2;
        theme::section(
            ui,
            if hybrid { "HYBRID OCEAN" } else { "BLUE WATER OCEAN" },
            if hybrid { "Native Blue Water surface above; JKA/Godot optics and surface while submerged." }
            else { "Original Blue Water ocean surface, waves and optical material." },
        );
        let mut ocean = self.video.ocean_settings;
        let d = crate::ocean::OceanSettings::default();
        let mut changed = false;
        if hybrid {
            theme::row(ui, "Ocean quality", "Adaptive (experimental) keeps native-resolution pixels and FFT geometry, restores near fine ripples and pixel-size sun sparkles, and skips fully filtered FFT reads farther away. Optimized removes those details. Sky only is optimized with SSR forced off (r_hybridMaterial 2). Original is the full Bluewater material. The r_oceanPixelDetail setting is not used in optimized/adaptive modes. Changes apply live.", theme::Reset::None, |ui| {
                for (mode, label) in [(0, "Original"), (1, "Optimized"), (2, "Sky only"), (3, "Adaptive + glints"), (4, "Ninja + FFT")] {
                    if mode >= 4 && !jka_ninja::INCLUDED { continue; }
                    if theme::chip(ui, label, ocean.hybrid_material == mode).clicked() {
                        ocean.hybrid_material = mode;
                        changed = true;
                    }
                }
            });
            if ocean.hybrid_material == 4 && ocean.ninja_preset == 1 && jka_ninja::INCLUDED {
                super::ocean_slider_d(ui, "Coast FFT shading detail", "Lower values fade the finest wave bands first. 1 uses all three bands for shading. Wave height, geometry and simulation rate stay the same.",
                    &mut ocean.ninja_coast_fft_detail, d.ninja_coast_fft_detail, 0.0..=1.0, &mut changed);
                theme::row(ui, "Coast sim quality", "Resolution of the Coast fluid sim (swash, foam, sim ripples) over its 100 m window. Full is the original port. UE uses the UE water actor's smaller velocity/density output. Half and Quarter run the solver at 1024 and 512 cells: the same waves and swirls with coarser detail, at a fraction of the cost. A change restarts the sim. r_ninjaCoastSimQuality.", theme::Reset::None, |ui| {
                    for (quality, label) in [(3u8, "Full"), (2, "UE"), (1, "Half"), (0, "Quarter")] {
                        if theme::chip(ui, label, ocean.ninja_coast_sim_quality == quality).clicked() { ocean.ninja_coast_sim_quality = quality; changed = true; }
                    }
                });
                theme::row(ui, "Coast tile map", "The Coast material's passive tile map: the long dark streaks across the water, their foam boost and mesh bumps. Off also skips its texture reads. r_ninjaCoastTileMap.", theme::Reset::None, |ui| {
                    if theme::chip(ui, "On", ocean.ninja_coast_tile_map).clicked() { ocean.ninja_coast_tile_map = true; changed = true; }
                    if theme::chip(ui, "Off", !ocean.ninja_coast_tile_map).clicked() { ocean.ninja_coast_tile_map = false; changed = true; }
                });
                theme::row(ui, "Coast sim detail", "Where the Coast fluid sim's small ripples, foam, swash and whitewater particles appear. Shore only keeps them over seabed shallower than 8 m; open ocean shows the plain sea. r_ninjaCoastShoreDepth.", theme::Reset::None, |ui| {
                    if theme::chip(ui, "Shore only", ocean.ninja_coast_shore_depth > 0.0).clicked() { ocean.ninja_coast_shore_depth = 8.0; changed = true; }
                    if theme::chip(ui, "Everywhere", ocean.ninja_coast_shore_depth <= 0.0).clicked() { ocean.ninja_coast_shore_depth = 0.0; changed = true; }
                });
                theme::row(ui, "Coast waves follow sliders", "The Coast fluid sim's waves follow the wave sliders: shores facing the swell direction get the strongest surf, wavelength sets how fast it arrives and wave height how strong. Off uses the fixed Fluid Ninja waves. r_ninjaCoastFollowSwell.", theme::Reset::None, |ui| {
                    if theme::chip(ui, "On", ocean.ninja_coast_follow_swell).clicked() { ocean.ninja_coast_follow_swell = true; changed = true; }
                    if theme::chip(ui, "Off", !ocean.ninja_coast_follow_swell).clicked() { ocean.ninja_coast_follow_swell = false; changed = true; }
                });
                theme::row(ui, "Open-sea foam", "Over open sea (outside the shore depth) the Coast foam shows on the FFT wave crests and rides with them, also up close. Needs wave height above 0. r_ninjaCoastOpenFoam.", theme::Reset::None, |ui| {
                    for (value, label) in [(0.0, "Off"), (2.0, "Light"), (4.0, "Medium"), (8.0, "Heavy")] {
                        if theme::chip(ui, label, (ocean.ninja_coast_open_foam - value).abs() < 0.01).clicked() { ocean.ninja_coast_open_foam = value; changed = true; }
                    }
                });
                theme::row(ui, "Coast sim when far", "Hold the Coast fluid and whitewater simulation while you are more than 55 m (straight-line distance) from the nearest water in the sim window. From there the sim no longer shades any visible water; only the swash height in the window freezes until you come closer. r_ninjaCoastSimDistance.", theme::Reset::None, |ui| {
                    if theme::chip(ui, "Always run", ocean.ninja_coast_sim_distance <= 0.0).clicked() { ocean.ninja_coast_sim_distance = 0.0; changed = true; }
                    if theme::chip(ui, "Pause past 55 m", ocean.ninja_coast_sim_distance > 0.0).clicked() { ocean.ninja_coast_sim_distance = 55.0; changed = true; }
                });
                theme::row(ui, "Coast tile normals", "The Coast material's tile height normals: the fine honeycomb ripple pattern. Off also skips their texture reads (~0.7 ms at 1440p over open sea). r_ninjaCoastTileNormals.", theme::Reset::None, |ui| {
                    if theme::chip(ui, "On", ocean.ninja_coast_tile_normals).clicked() { ocean.ninja_coast_tile_normals = true; changed = true; }
                    if theme::chip(ui, "Off", !ocean.ninja_coast_tile_normals).clicked() { ocean.ninja_coast_tile_normals = false; changed = true; }
                });
                theme::row(ui, "Coast wake foam", "How strongly moving water turns to foam. Lake uses the Fluid Ninja Lake material's velocity foam, so your wake leaves foam trails (fast shore swash also foams more). Coastline keeps the Coastline material, where wakes barely foam. r_ninjaCoastWakeFoam.", theme::Reset::None, |ui| {
                    if theme::chip(ui, "Lake", ocean.ninja_coast_wake_foam).clicked() { ocean.ninja_coast_wake_foam = true; changed = true; }
                    if theme::chip(ui, "Coastline", !ocean.ninja_coast_wake_foam).clicked() { ocean.ninja_coast_wake_foam = false; changed = true; }
                });
            }
        }
        theme::section(ui, "WAVES", "Native Blue Water parameters, shared across promoted pools. Changes apply live.");
        super::ocean_slider_d(ui, "Wave height", if hybrid {
            "Master wave height: controls Blue Water FFT and proportionally scales the Hybrid Gerstner swell. Zero removes both wave contributions. Gerstner height still sets relative strength."
        } else {
            "Significant wave height in metres (4 times the surface height standard deviation). Zero is flat."
        }, &mut ocean.blue_water_waves.height, d.blue_water_waves.height, 0.0..=60.0, &mut changed);
        super::ocean_slider_d(ui, "Swell direction", "Direction waves travel, in degrees from +X toward world +Z.", &mut ocean.blue_water_waves.direction, d.blue_water_waves.direction, -180.0..=180.0, &mut changed);
        super::ocean_slider_d(ui, "Peak wavelength (m)", "Dominant crest spacing in metres. Controls wave speed through the native dispersion relation.", &mut ocean.blue_water_waves.wavelength, d.blue_water_waves.wavelength, 2.0..=512.0, &mut changed);
        super::ocean_slider_d(ui, "Choppiness", "Horizontal displacement strength. Zero keeps only vertical waves.", &mut ocean.blue_water_waves.choppiness, d.blue_water_waves.choppiness, 0.0..=4.0, &mut changed);
        super::ocean_slider_d(ui, "Spectrum wind speed", "Reference wind speed in metres per second. Changes the native spectral shape and fine surface roughness; independent of weather wind.", &mut ocean.blue_water_waves.wind_speed, d.blue_water_waves.wind_speed, 0.1..=40.0, &mut changed);
        {
            // r_oceanLongWaves: the FFT's long waves from N stronger waves.
            let mut count = ocean.long_waves as f32;
            super::ocean_slider_d(ui, "Long-wave count (0 = off)",
                "Builds the big waves from this many wave pieces instead of thousands, keeping the same height, wavelength and direction spread. Short waves, chop and foam are unchanged. Find the lowest count that still looks like the original: that is how many waves the server would add up at your position.",
                &mut count, d.long_waves as f32, 0.0..=1024.0, &mut changed);
            ocean.long_waves = count.round().max(0.0) as u32;
            // r_oceanMediumWaves: the same for the medium waves.
            let mut medium = ocean.medium_waves as f32;
            super::ocean_slider_d(ui, "Medium-wave count (0 = off)",
                "Builds the medium waves (shorter than the big ones, longer than the fine chop) from this many wave pieces instead of thousands, keeping the same height and spread. Find the lowest count that still looks like the original.",
                &mut medium, d.medium_waves as f32, 0.0..=1024.0, &mut changed);
            ocean.medium_waves = medium.round().max(0.0) as u32;
        }
        super::ocean_slider_d(ui, "Medium-wave detail",
            "Strength of the medium waves (a few metres up to about 20 m). Raise it for more mid-size surface variation. 1 = Blue Water's own spectrum.",
            &mut ocean.medium_wave_detail, d.medium_wave_detail, 0.0..=16.0, &mut changed);
        super::ocean_slider_d(ui, "Small-wave detail",
            "Strength of the fine chop (the smallest waves). Raise it for more small surface variation. Visual only: the server never adds these up, so it costs nothing there. 1 = Blue Water's own spectrum.",
            &mut ocean.small_wave_detail, d.small_wave_detail, 0.0..=16.0, &mut changed);
        if hybrid {
            theme::row(ui, "Crest breaking (A/B)", "Original preserves the current waves. Limit is the original conservative comparison. Folds keeps the previous experimental spilling behavior. Spill uses the conservative compression budget and whitewater from the unbounded wave (also in compact materials). Vertical wave height is preserved; no intentional overhang, not a physical breaking-wave simulation.", theme::Reset::None, |ui| {
                for (mode, label) in [(0, "Original"), (1, "Limit"), (2, "Folds"), (3, "Spill")] {
                    if theme::chip(ui, label, ocean.hybrid_breaker == mode).clicked() {
                        ocean.hybrid_breaker = mode;
                        changed = true;
                    }
                }
            });
            theme::section(ui, "GERSTNER / FFT MIX", "Mix Blue Water's spectral waves with the authored directional swell. The FFT simulation stays resident and unchanged.");
            super::ocean_slider_d(ui, "Cross-sea / FFT blend",
                "1 = pure Blue Water FFT (original). 0 = authored Gerstner swell. Between them, both displacements contribute. Hybrid only; changing this does not rebuild the FFT.",
                &mut ocean.hybrid_fft_blend, d.hybrid_fft_blend, 0.0..=1.0, &mut changed);
            if ocean.hybrid_fft_blend < 1.0 {
                // Hybrid has independent authored wave controls; no Godot map
                // wave parameters or linked controls are modified.
                super::ocean_slider_d(ui, "Gerstner height (units)",
                    "Base swell amplitude at the default Wave height (40 JKA units = 1 metre). Actual Gerstner displacement also follows Wave height and the FFT blend.",
                    &mut ocean.hybrid_swell.amplitude, d.hybrid_swell.amplitude, 0.0..=1024.0, &mut changed);
                super::ocean_slider_metres_d(ui, "Gerstner wavelength (m)", "Distance between crests, in metres (40 JKA units = 1 metre).", &mut ocean.hybrid_swell.wavelength, d.hybrid_swell.wavelength, 256.0, 1.6, &mut changed);
                super::ocean_slider_d(ui, "Gerstner speed (units/s)", "Travel speed in JKA units per second; negative reverses travel.", &mut ocean.hybrid_swell.speed, d.hybrid_swell.speed, -4096.0..=4096.0, &mut changed);
                super::ocean_slider_d(ui, "Gerstner steepness", "Controls the authored swell's horizontal choppiness.", &mut ocean.hybrid_swell.steepness, d.hybrid_swell.steepness, 0.0..=1.0, &mut changed);
            }
            if theme::ghost_button(ui, "RESET GERSTNER & BLEND").clicked() {
                ocean.hybrid_fft_blend = 1.0;
                ocean.hybrid_swell = crate::ocean::OceanAuthoring::default();
                changed = true;
            }
        }
        if theme::ghost_button(ui, "RESET WAVES").clicked() {
            ocean.blue_water_waves = crate::ocean::BlueWaterWaves::default();
            changed = true;
        }
        theme::row(
            ui,
            "FFT update rate",
            "How often the Blue Water wave field advances. Zero removes the rate cap.",
            theme::Reset::None,
            |ui| {
                let mut value = ocean.updates_per_second;
                let readout = if value == 0.0 {
                    "Uncapped".to_owned()
                } else {
                    format!("{value:.0} Hz")
                };
                if theme::slider_default(ui, &mut value, 0.0..=60.0, &readout, d.updates_per_second) {
                    ocean.updates_per_second = value;
                    changed = true;
                }
            },
        );
        theme::row(
            ui,
            "Spread FFT updates",
            "On: each capped update's work is spread over the frames of its period (computed for the time it appears), so the FFT costs about the same every frame. Off: the whole update on its tick. Uncapped (0) is unaffected. This session. Console: r_oceanUpdateSpread.",
            theme::Reset::None,
            |ui| {
                let mut spread = crate::ocean::update_spread();
                if ui.checkbox(&mut spread, "Spread over frames").changed() {
                    crate::ocean::set_update_spread(spread);
                }
            },
        );
        theme::section(ui,"COASTAL WAVES & SURF (A/B)","Blue Water's original coast model. Your separate JKA shallow-bank solver remains available through r_coastal.");
        theme::row(ui,"Upstream coastal waves","Builds per-pool shore depth and wind-fetch maps in the background. Waves shrink in shallow water and behind land; moving surf bands follow the bank. Off restores open-ocean waves. Native / Hybrid only.",theme::Reset::None,|ui| {
            if ui.checkbox(&mut ocean.blue_water_coast,"Blue Water coast & surf").changed() {changed=true;}
        });
        if ocean.blue_water_coast {
            theme::row(ui,"Physical land shelter (advanced)","Optional upstream wind-fetch and swell shelter. Low wind with long authored waves can become almost flat behind land. Leave off for authored incoming ocean waves with depth breaking and surf at real banks.",theme::Reset::None,|ui| {
                if ui.checkbox(&mut ocean.blue_water_coast_shelter,"Wind / swell shelter").changed() {changed=true;}
            });
            let status=self.prepared_map_cache.as_ref().and_then(|c|c.map.coastal_bathymetry.as_ref())
                .map_or("Load a BSP map with water promotion enabled to prepare its shores.",|j|j.upstream_shore.status());
            theme::banner(ui,status,theme::TEXT_DIM());
        }
        theme::section(ui, "REFLECTIONS & OPTICS", "Screen-space reflections, caustics, refraction and underwater effects. Each change applies live.");
        theme::row(ui, "Screen-space reflections", "Trace nearby scene reflections. Environment and sky remain when disabled.", theme::Reset::None, |ui| {
            let value = if hybrid { &mut ocean.hybrid_ssr } else { &mut ocean.blue_water_ssr };
            if ui.checkbox(value, "Trace scene reflections").changed() { changed = true; }
        });
        theme::row(ui, "Caustics", "Godot carries the inexpensive optics caustics into BlueWater transmission; native modes use its separate receiver pass.", theme::Reset::None, |ui| {
            for &(mode, name) in if hybrid { &[(3, "Godot"), (4, "Animated"), (0, "Native"), (1, "Native FFT only"), (2, "Off")][..] } else { &[(0, "Native"), (1, "Native FFT only"), (2, "Off")][..] } {
                if theme::chip(ui, name, (if hybrid { ocean.hybrid_caustics_mode } else { ocean.blue_water_caustics_mode }) == mode).clicked() {
                    if hybrid { ocean.hybrid_caustics_mode = mode; } else { ocean.blue_water_caustics_mode = mode; } changed = true;
                }
            }
        });
        theme::row(ui, "Refraction method", "Screen offset restores seabed bending with the inexpensive Godot approach. Ray trace is the more expensive physical scene trace.", theme::Reset::None, |ui| {
            for &(mode, name) in if hybrid { &[(1, "Screen offset"), (2, "Ray trace"), (0, "Off")][..] } else { &[(2, "Ray trace"), (1, "Screen offset"), (0, "Off")][..] } {
                if theme::chip(ui, name, (if hybrid { ocean.hybrid_refraction_mode } else { ocean.blue_water_refraction_mode }) == mode).clicked() {
                    if hybrid { ocean.hybrid_refraction_mode = mode; } else { ocean.blue_water_refraction_mode = mode; } changed = true;
                }
            }
        });
        theme::row(ui, "Underwater reflections", "Seen from below, the surface mirrors the submerged scene (screen traced) outside Snell's window. Every underwater mode, including Godot submerged. Shared with rivers; this session. Console: r_underwaterReflections.", theme::Reset::None, |ui| {
            let mut mirror = crate::ocean::underwater_reflections();
            if ui.checkbox(&mut mirror, "Mirror the submerged scene").changed() {
                crate::ocean::set_underwater_reflections(mirror);
            }
        });
        theme::row(ui, "Underwater effects", "Hybrid uses the Godot surface and underwater optics while submerged; native modes keep Blue Water scattering and waterline. Fluid Ninja uses the Ninja river's underwater look (distortion, distance tint, meniscus, caustics). Above water, Hybrid keeps its own surface (Blue Water or Ninja).", theme::Reset::None, |ui| {
            for &(mode, name) in if hybrid { &[(4, "Godot submerged"), (0, "Native"), (3, "Native submerged"), (1, "No motes"), (2, "Off/test"), (5, "Fluid Ninja")][..] } else { &[(0, "Native"), (3, "Native submerged"), (1, "No motes"), (2, "Off/test"), (5, "Fluid Ninja")][..] } {
                if mode == crate::ocean::NINJA_UNDERWATER_MODE && !jka_ninja::INCLUDED { continue; }
                if theme::chip(ui, name, (if hybrid { ocean.hybrid_underwater_mode } else { ocean.blue_water_underwater_mode }) == mode).clicked() {
                    if hybrid { ocean.hybrid_underwater_mode = mode; } else { ocean.blue_water_underwater_mode = mode; } changed = true;
                }
            }
        });
        theme::section(ui, "SHIP WAKE & SPRAY (A/B)", "Blue Water's own wake, impact rings and spindrift, ported beside JKA's water interaction for comparison. Console: oceanwake ship | strike | clear | reset.");
        theme::row(ui, "Ship wake", "Off keeps JKA's own water interaction only. On binds Blue Water's dispersive wake field, trail foam and slick, bow waves, hull contact foam and impact rings to this surface. It needs hulls: sail a demo with 'oceanwake ship destroyer', or drop an impact with 'oceanwake strike'.", theme::Reset::None, |ui| {
            let mut on = ocean.blue_wake != 0;
            if ui.checkbox(&mut on, "Blue Water wake").changed() { ocean.blue_wake = u8::from(on); changed = true; }
        });
        theme::row(ui, "Spindrift", "Wisps of spray torn off breaking crests, from a gale's wind (13 m/s; raise Spectrum wind speed above). The surface's spray haze needs 17 m/s.", theme::Reset::None, |ui| {
            let mut on = ocean.spindrift != 0;
            if ui.checkbox(&mut on, "Blue Water spindrift").changed() { ocean.spindrift = u8::from(on); changed = true; }
        });
        theme::section(ui, "WATER APPEARANCE", "Changes apply live above and below the surface.");
        theme::row(ui, "Transparency", if hybrid {
            "Hybrid clarity multiplier: controls both native surface transmission and Godot underwater fog distance. 1x uses stock Godot fog distance, independent of the Godot preset."
        } else {
            "Higher values let light travel farther through water. 1x preserves native calibration."
        }, theme::Reset::None, |ui| {
            let mut value = if hybrid { ocean.hybrid_transparency } else { ocean.blue_water_optics.transparency };
            let readout = format!("{value:.2}x");
            if theme::slider_default(ui, &mut value, 0.1..=10.0, &readout, if hybrid { d.hybrid_transparency } else { d.blue_water_optics.transparency }) {
                if hybrid { ocean.hybrid_transparency = value; } else { ocean.blue_water_optics.transparency = value; }
                changed = true;
            }
        });
        if hybrid {
            super::color_row(ui, "Water color", "The water's color, one setting for above and below the surface: the Hybrid / Fluid Ninja surface body tint and the underwater fog (Godot submerged optics and Blue Water scattering). Does not modify native Blue Water's saved colors. r_hybridWaterColor.", theme::Reset::None,
                &mut ocean.hybrid_water_color, &mut changed);
        } else {
            super::color_row(ui, "Water color", "Tint of the native water body and crest transmission.", theme::Reset::None,
                &mut ocean.blue_water_optics.water_color, &mut changed);
            super::color_row(ui, "Underwater fog color", "Color of native Blue Water scattered light below the surface.", theme::Reset::None,
                &mut ocean.blue_water_optics.fog_color, &mut changed);
        }
        if theme::ghost_button(ui, "RESET WATER APPEARANCE").clicked() {
            if hybrid {
                ocean.hybrid_transparency = 1.0;
                ocean.hybrid_water_color = crate::ocean::BlueWaterOptics::default().water_color;
            } else {
                ocean.blue_water_optics = crate::ocean::BlueWaterOptics::default();
            }
            changed = true;
        }
        if hybrid && ocean.hybrid_caustics_mode == 4 {
            theme::section(ui, "ANIMATED CAUSTICS", "Independent procedural light networks that deform, split and merge over time. GPU pattern is generated once, not re-simulated per frame. No ocean FFT reads per receiver pixel.");
            super::ocean_slider_d(ui, "Pattern scale", "Larger values make finer light webs, without increasing texture resolution or FFT cost.", &mut ocean.hybrid_procedural_scale, d.hybrid_procedural_scale, 0.25..=4.0, &mut changed);
            super::ocean_slider_d(ui, "Animation speed", "Pattern evolution rate; zero freezes the field for performance/visual A/B. Wave roughness also influences the animation.", &mut ocean.hybrid_procedural_speed, d.hybrid_procedural_speed, 0.0..=3.0, &mut changed);
        }
        if hybrid && ocean.hybrid_caustics_mode == 3 {
            theme::section(ui, "HYBRID CAUSTIC SHAPE", "The 181m Blue Water mid-cascade makes larger patterns than Godot's 57m cascade. These controls only affect Hybrid's Godot receiver caustics (mode: Godot). No extra passes or textures.");
            super::ocean_slider_d(ui, "Pattern density", "Higher values create smaller caustic markings by tiling the existing Blue Water slope texture. 3x roughly compensates for the 181m vs 57m wave tile lengths.", &mut ocean.hybrid_caustics_scale, d.hybrid_caustics_scale, 1.0..=6.0, &mut changed);
            super::ocean_slider_d(ui, "Focus gain", "Additional Hybrid-only brightness/contrast after wave curvature calculation. Godot caustic strength is still the master control.", &mut ocean.hybrid_caustics_gain, d.hybrid_caustics_gain, 0.0..=4.0, &mut changed);
        }
        super::color_row(
            ui,
            "Foam color",
            "Tint for the original Blue Water crest and surface foam.",
            theme::Reset::None,
            &mut ocean.foam_color,
            &mut changed,
        );
        super::ocean_slider_d(ui, "Surface refraction", "Wave distortion used by the original surface's scene transmission.", &mut ocean.blue_water_refraction, d.blue_water_refraction, 0.0..=0.15, &mut changed);
        super::ocean_slider_d(ui, "Caustic strength", "Strength of the selected Godot or native Blue Water caustics.", &mut ocean.optics.caustics, d.optics.caustics, 0.0..=4.0, &mut changed);
        if hybrid {
            super::ocean_slider_d(ui, "Caustic view distance", "Godot-style Hybrid caustics: skip distant terrain receivers and fade the last 25% of range, in metres. 0 disables culling. Native caustics use their own height gate.", &mut ocean.optics.caustic_max_distance_m, d.optics.caustic_max_distance_m, 0.0..=500.0, &mut changed);
        }
        if changed {
            self.video.ocean_settings = ocean;
            self.commit_ocean_settings();
        }

        theme::banner(ui, "Use render_perf_capture for ocean GPU pass timings. r_showtris 32 draws this backend's actual mesh. Sky/cloud backends live in Clouds.", theme::TEXT_DIM());
    }
}
