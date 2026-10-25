use super::*;
use crate::eal::EalMap;

fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut b = tag.to_vec();
    b.extend((data.len() as u32).to_le_bytes());
    b.extend(data);
    if data.len() % 2 != 0 {
        b.push(0);
    }
    b
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn names(values: &[&str], width: usize) -> Vec<u8> {
    let mut result = vec![0; values.len() * width];
    for (slot, value) in result.chunks_exact_mut(width).zip(values) {
        slot[..value.len()].copy_from_slice(value.as_bytes());
    }
    result
}
fn list(kind: &[u8; 4], fields: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut b = kind.to_vec();
    for (tag, data) in fields {
        b.extend(chunk(tag, data));
    }
    chunk(b"LIST", &b)
}
fn preset(decay: f32, wet: bool) -> Vec<u8> {
    let mut p = [0u32; 28];
    for (i, v) in [
        (1, 7.5f32),
        (2, 1.),
        (6, decay),
        (7, 1.),
        (8, 1.),
        (10, 0.01),
        (15, 0.02),
        (19, 0.25),
        (21, 0.25),
        (23, -5.),
        (24, 5000.),
        (25, 250.),
    ] {
        p[i] = v.to_bits();
    }
    p[3] = (if wet { -1000i32 } else { -10000 }) as u32;
    p[27] = 0x20;
    words(&p)
}
fn source(dry: bool) -> Vec<u8> {
    let mut p = [0u32; 28];
    p[0] = (if dry { 0i32 } else { -10000 }) as u32;
    for (i, v) in [
        (7, 0.25f32),
        (8, 1.5),
        (9, 1.),
        (11, 1.),
        (13, 1.),
        (16, 0.),
        (21, 0.),
        (22, 0.),
        (23, -1.),
        (24, 1.),
        (25, 1000000.),
    ] {
        p[i] = v.to_bits();
    }
    p[18] = 360;
    p[19] = 360;
    p[26] = 1;
    words(&p)
}
// A complete, generated EAL 3 dataset: two rooms split at Eagle X=0,
// authored material between them, optional aperture and diffraction box.
pub(crate) fn fixture(
    wet: bool,
    material: i32,
    lf: f32,
    diff: bool,
    aperture: bool,
    dry: bool,
) -> EalMap {
    let mut b = b"eal ".to_vec();
    for (tag, data) in [
        (b"majv", words(&[3])),
        (b"minv", words(&[0])),
        (b"gdfm", words(&[(-2500i32) as u32, 0.2f32.to_bits(), 90])),
        (
            b"lisa",
            words(&[1f32.to_bits(), 1f32.to_bits(), 1f32.to_bits()]),
        ),
        (b"denv", preset(0.15, wet)),
        (b"dsrc", source(dry)),
        (b"dmat", words(&[0, 0, 1f32.to_bits(), 3])),
    ] {
        b.extend(chunk(tag, &data));
    }
    b.extend(list(
        b"envp",
        &[
            (b"num ", words(&[2])),
            (b"nams", names(&["Small", "Large"], 32)),
            (b"lisp", [preset(0.15, wet), preset(3., wet)].concat()),
        ],
    ));
    b.extend(list(
        b"matp",
        &[
            (b"num ", words(&[1])),
            (b"nams", names(&["Wall"], 32)),
            (
                b"mata",
                words(&[material as u32, lf.to_bits(), 1f32.to_bits(), 3]),
            ),
        ],
    ));
    b.extend(list(
        b"srcp",
        &[
            (b"num ", words(&[3])),
            (b"nams", names(&["Aperture0a", "Aperture0b", "Test"], 32)),
            (b"fils", names(&["", "", "sound/test.wav"], 260)),
            (b"srca", [source(dry), source(dry), source(dry)].concat()),
        ],
    ));
    let mut g = words(&[if aperture { 4 } else { 0 }, 2, 0, 1, 2]);
    if aperture {
        g.extend(words(&[0, 0, 1, 1]));
        for p in [
            [5f32, 0., -10.],
            [-5., 0., -10.],
            [5., 0., 10.],
            [-5., 0., 10.],
        ] {
            g.extend(words(&p.map(f32::to_bits)));
        }
    }
    g.extend(words(&[0, 1, u32::MAX, u32::MAX]));
    // default, small, large; only cross-room pairs hit the wall.
    g.extend(words(&[
        u32::MAX - 1,
        u32::MAX - 1,
        u32::MAX - 1,
        u32::MAX - 1,
        u32::MAX - 1,
        0,
        u32::MAX - 1,
        0,
        u32::MAX - 1,
    ]));
    g.extend(words(&[
        0,
        0,
        0,
        1f32.to_bits(),
        0,
        0,
        0x80000000,
        0x80000001,
        0,
        0,
        0,
    ]));
    g.extend(words(&[0, 0, 0, 1]));
    g.extend(words(&[u32::from(diff)]));
    if diff {
        g.extend(words(&[
            0,
            10f32.to_bits(),
            (-5f32).to_bits(),
            (-5f32).to_bits(),
            20f32.to_bits(),
            5f32.to_bits(),
            5f32.to_bits(),
        ]));
    }
    b.extend(list(
        b"gemp",
        &[
            (b"num ", words(&[1])),
            (b"nams", names(&["Main"], 32)),
            (b"fils", vec![0; 260]),
            (b"gema", g),
        ],
    ));
    EalMap::decode(&chunk(b"RIFF", &b)).unwrap()
}
fn frame(source: [f32; 3]) -> SceneFrame {
    let mut f = SceneFrame::silent();
    f.listener = [5., 0., 0.];
    f.voices[0].active = 1;
    f.voices[0].id = 1;
    f.voices[0].position = source;
    let path = b"sound/test.wav";
    f.voices[0].name[..path.len()].copy_from_slice(path);
    f
}
fn energy(values: &[f32]) -> f64 {
    values.iter().map(|&v| (v as f64).powi(2)).sum()
}
fn tone(material: i32, lf: f32, hz: f32) -> f64 {
    let mut scene =
        NativeScene::new(48000, &fixture(false, material, lf, false, false, true)).unwrap();
    let mut f = frame([-5., 0., 0.]);
    let mut out = [0.; FRAMES * 2];
    let mut e = 0.;
    for block in 0..100 {
        for i in 0..FRAMES {
            f.voices[0].pcm[i] =
                0.1 * (std::f32::consts::TAU * hz * (block * FRAMES + i) as f32 / 48000.).sin();
        }
        let stats = scene.render(&f, &mut out).unwrap();
        assert_eq!(stats.sources, 1);
        if block > 40 {
            e += energy(&out);
        }
    }
    e
}
#[test]
fn ffi_layout_matches_native_scene() {
    assert_eq!(std::mem::size_of::<VoiceBlock>(), 1144);
    assert_eq!(std::mem::size_of::<SceneFrame>(), 48 + 96 * 1144);
    assert_eq!(std::mem::size_of::<SceneStats>(), 72);
}
#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn native_material_preserves_lf_hf_filtering() {
    let low_clear = tone(0, 0., 200.);
    let low_wall = tone(-4000, 0., 200.);
    let high_clear = tone(0, 0., 10000.);
    let high_wall = tone(-4000, 0., 10000.);
    let low_solid = tone(-4000, 1., 200.);
    println!(
        "EAL material energy ratios: LF {} HF {} solid LF {}",
        low_wall / low_clear,
        high_wall / high_clear,
        low_solid / low_clear
    );
    assert!(low_wall / low_clear > 0.3);
    assert!(high_wall / high_clear < 0.03);
    assert!(low_solid / low_clear < 0.03);
}
#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn native_diffraction_changes_audio_within_one_room() {
    let mut energies = vec![];
    for diff in [false, true] {
        let mut scene = NativeScene::new(48000, &fixture(false, 0, 0., diff, false, true)).unwrap();
        let mut f = frame([30., 0., 0.]);
        let mut out = [0.; FRAMES * 2];
        let mut e = 0.;
        let mut detected = 0;
        for block in 0..80 {
            for i in 0..FRAMES {
                f.voices[0].pcm[i] = 0.1
                    * (std::f32::consts::TAU * 10000. * (block * FRAMES + i) as f32 / 48000.).sin();
            }
            let stats = scene.render(&f, &mut out).unwrap();
            detected = detected.max(stats.diffracted_sources);
            if block > 40 {
                e += energy(&out);
            }
        }
        assert_eq!(detected, u32::from(diff));
        energies.push(e);
    }
    println!("Diffraction HF energy ratio {}", energies[1] / energies[0]);
    assert!(energies[1] < energies[0] * 0.2);
}
fn tail(position: [f32; 3], rotate: bool) -> ([f64; 2], SceneStats) {
    let mut scene = NativeScene::new(48000, &fixture(true, 0, 0., false, true, false)).unwrap();
    let mut f = frame(position);
    if rotate {
        f.ahead = [-1., 0., 0.];
    }
    let mut out = [0.; FRAMES * 2];
    let mut e = [0.; 2];
    let mut stats = SceneStats::default();
    for _ in 0..40 {
        scene.render(&f, &mut out).unwrap();
    }
    f.voices[0].pcm[0] = 0.5;
    scene.render(&f, &mut out).unwrap();
    f.voices[0].pcm.fill(0.);
    for block in 0..300 {
        stats = scene.render(&f, &mut out).unwrap();
        if block > 100 {
            for pair in out.chunks_exact(2) {
                e[0] += (pair[0] as f64).powi(2);
                e[1] += (pair[1] as f64).powi(2);
            }
        }
    }
    (e, stats)
}
#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn native_sources_keep_separate_room_decay_and_aperture_pan() {
    let (small, _) = tail([10., 0., 0.], false);
    let (large, stats) = tail([-5., 0., 0.], false);
    println!("Late tail small {small:?}, large {large:?}, stats {stats:?}");
    assert_eq!(stats.rooms, 2);
    assert_eq!(stats.directional_rooms, 2);
    assert!(large.iter().sum::<f64>() > small.iter().sum::<f64>() * 100.);
    // Turn the listener 90 degrees: the remote aperture moves to the side.
    let mut scene = NativeScene::new(48000, &fixture(true, 0, 0., false, true, false)).unwrap();
    let mut f = frame([-5., 0., 0.]);
    f.ahead = [0., 1., 0.];
    let mut out = [0.; FRAMES * 2];
    for _ in 0..40 {
        scene.render(&f, &mut out).unwrap();
    }
    f.voices[0].pcm[0] = 0.5;
    scene.render(&f, &mut out).unwrap();
    f.voices[0].pcm.fill(0.);
    let mut ears = [0f64; 2];
    for _ in 0..180 {
        scene.render(&f, &mut out).unwrap();
        for p in out.chunks_exact(2) {
            ears[0] += (p[0] as f64).powi(2);
            ears[1] += (p[1] as f64).powi(2);
        }
    }
    println!("Aperture ear energy {ears:?}");
    assert!(ears[0].max(ears[1]) > ears[0].min(ears[1]) * 1.2);
    // Distinct source slots can share a sound path and occupy different rooms.
    f.voices[1] = f.voices[0];
    f.voices[1].id = 2;
    f.voices[1].position = [10., 0., 0.];
    assert_eq!(scene.render(&f, &mut out).unwrap().sources, 2);
    f.hrtf = 1;
    assert_eq!(scene.render(&f, &mut out).unwrap().hrtf, 1);
    f.hrtf = 0;
    assert_eq!(scene.render(&f, &mut out).unwrap().hrtf, 0);
}
#[test]
#[ignore = "requires EAL_TEST_MAP and built EAL_TEST_OPENAL_DLL"]
fn native_retail_dataset_renders_all_zones_without_property_errors() {
    let path = std::path::PathBuf::from(std::env::var_os("EAL_TEST_MAP").unwrap());
    let mut paths = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "eal"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut total = 0.;
    let mut maps = 0;
    for path in paths {
        let map = EalMap::decode(&std::fs::read(&path).unwrap()).unwrap();
        let mut scene =
            NativeScene::new(48000, &map).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut out = [0.; FRAMES * 2];
        for x in (-3000..=3000).step_by(1000) {
            for y in (-3000..=3000).step_by(1000) {
                let mut f = frame([x as f32, y as f32, 128.]);
                f.listener = [y as f32, x as f32, 64.];
                f.voices[0].pcm.fill(0.01);
                scene.render(&f, &mut out).unwrap_or_else(|e| {
                    panic!("{} listener {:?}: {e}", path.display(), f.listener)
                });
                total += energy(&out);
            }
        }
        maps += 1;
    }
    assert!(maps > 0 && total > 0.);
    println!("Retail EAL {maps} datasets grid output energy {total}");
}

#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn native_full_polyphony_and_output_rates_keep_source_data() {
    for rate in [44100, 48000] {
        let mut scene =
            NativeScene::new(rate, &fixture(true, -1000, 0.25, false, false, true)).unwrap();
        let mut f = frame([10., 0., 0.]);
        let mut out = [0.; FRAMES * 2];
        for i in 1..MAX_SOURCES {
            f.voices[i] = f.voices[0];
            f.voices[i].id = (i + 1) as u64;
            f.voices[i].position = [if i % 2 == 0 { 10. } else { -10. }, i as f32, 0.];
        }
        let mut max_us = 0;
        let mut total_us = 0;
        let mut e = 0.;
        for block in 0..96 {
            f.hrtf = u32::from(block >= 48);
            for voice in &mut f.voices {
                for i in 0..FRAMES {
                    voice.pcm[i] = 0.001
                        * (std::f32::consts::TAU * 440. * (block * FRAMES + i) as f32
                            / rate as f32)
                            .sin();
                }
            }
            let started = std::time::Instant::now();
            let stats = scene.render(&f, &mut out).unwrap();
            let us = started.elapsed().as_micros();
            max_us = max_us.max(us);
            total_us += us;
            assert_eq!(stats.sources, MAX_SOURCES as u32);
            assert_eq!(stats.rooms, 2);
            e += energy(&out);
        }
        assert!(e > 0.);
        println!(
            "Native EAL {rate} Hz, {MAX_SOURCES} sources: mean {} us, max {max_us} us",
            total_us / 96
        );
    }
}

#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn callback_default_matches_forced_mode_and_keeps_a_continuous_tone() {
    for rate in [44100, 48000] {
        let map = fixture(false, 0, 0., false, false, true);
        let mut normal = NativeScene::new(rate, &map).unwrap();
        let mut forced = NativeScene::new(rate, &map).unwrap();
        let mut f = frame([10., 0., 0.]);
        let mut a = [0.; FRAMES * 2];
        let mut b = [0.; FRAMES * 2];
        let mut previous = [0f32; 2];
        let mut max_step = 0f32;
        let mut underfeeds = None;
        for block in 0..200 {
            for i in 0..FRAMES {
                f.voices[0].pcm[i] = 0.05
                    * (std::f32::consts::TAU * 440. * (block * FRAMES + i) as f32 / rate as f32)
                        .sin();
            }
            f.debug_mode = 0;
            let stats = normal.render(&f, &mut a).unwrap();
            f.debug_mode = 7;
            let reference = forced.render(&f, &mut b).unwrap();
            assert_eq!(stats.transport, 1, "default must use the callback path");
            assert_eq!(reference.transport, 1);
            assert_eq!(stats.callback_dropped_samples, 0);
            assert_eq!(stats.restarts, 0);
            for (x, y) in a.iter().zip(b) {
                assert!(
                    (x - y).abs() < 0.000001,
                    "default differs from forced callback"
                );
            }
            if block == 20 {
                underfeeds = Some(stats.callback_underfeed_samples);
            }
            if block > 20 {
                assert_eq!(
                    Some(stats.callback_underfeed_samples),
                    underfeeds,
                    "ongoing PCM underfeed"
                );
                for pair in a.chunks_exact(2) {
                    for c in 0..2 {
                        max_step = max_step.max((pair[c] - previous[c]).abs());
                        previous[c] = pair[c];
                    }
                }
            } else {
                previous.copy_from_slice(&a[FRAMES * 2 - 2..]);
            }
        }
        assert!(
            max_step < 0.006,
            "discontinuous {rate} Hz callback tone: step {max_step}"
        );
        println!("Callback default/forced {rate} Hz: max sample step {max_step}");
    }
}

#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn callback_short_sources_drain_like_a_zero_fed_reference() {
    for rate in [44100, 48000] {
        for length in [1usize, 64, 255, 256, 257, 513] {
            let map = fixture(false, 0, 0., false, false, true);
            let mut ended = NativeScene::new(rate, &map).unwrap();
            let mut reference = NativeScene::new(rate, &map).unwrap();
            let mut f = frame([10., 0., 0.]);
            let mut actual = Vec::new();
            let mut expected = Vec::new();
            let mut a = [0.; FRAMES * 2];
            let mut b = [0.; FRAMES * 2];
            let blocks = length.div_ceil(FRAMES);
            for block in 0..blocks + 20 {
                f.voices[0].pcm.fill(0.);
                for i in 0..FRAMES {
                    let offset = block * FRAMES + i;
                    if offset < length {
                        f.voices[0].pcm[i] = if offset + 1 == length { 0.5 } else { 0.02 };
                    }
                }
                f.voices[0].active = u32::from(block < blocks);
                let stats = ended.render(&f, &mut a).unwrap();
                assert_eq!(stats.callback_dropped_samples, 0);
                f.voices[0].active = 1;
                reference.render(&f, &mut b).unwrap();
                actual.extend(a);
                expected.extend(b);
            }
            let peak = expected
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                .unwrap()
                .0;
            assert!(expected[peak].abs() > 0.05, "missing reference end marker");
            assert!(
                (actual[peak] - expected[peak]).abs() < 0.0001,
                "{rate} Hz {length} frame source lost its final marker: {} versus {}",
                actual[peak],
                expected[peak]
            );
            let error = actual
                .iter()
                .zip(&expected)
                .map(|(a, b)| (a - b).powi(2) as f64)
                .sum::<f64>();
            let signal = energy(&expected);
            println!("Callback drain {rate} Hz {length} frames: relative error {}", error / signal);
            assert!(
                error < signal * 0.001,
                "{rate} Hz {length} frame source truncated: relative error {}",
                error / signal
            );
        }
    }
}

#[test]
#[ignore = "requires built EAL_TEST_OPENAL_DLL"]
fn callback_mode_switches_and_hrtf_report_device_state() {
    let mut scene = NativeScene::new(48000, &fixture(false, 0, 0., false, false, true)).unwrap();
    let mut f = frame([10., 0., 0.]);
    let mut out = [0.; FRAMES * 2];
    for mode in [0, 9, 0, 7, 8, 0] {
        f.debug_mode = mode;
        for _ in 0..4 {
            let stats = scene.render(&f, &mut out).unwrap();
            assert_eq!(stats.transport, if mode == 9 { 2 } else { 1 });
        }
    }
    for enabled in [0, 1, 0, 1] {
        f.hrtf = enabled;
        let stats = scene.render(&f, &mut out).unwrap();
        assert_eq!(
            stats.hrtf, enabled,
            "device did not accept HRTF state; status {}",
            stats.hrtf_status
        );
        assert_eq!(
            stats.hrtf != 0,
            matches!(stats.hrtf_status, 1 | 3 | 4),
            "HRTF flag disagrees with device status"
        );
    }
}
