//! OpenJK `snd_ambient.cpp`: the ambient sound sets of `sound/sound.txt`.
//!
//! Three kinds share one name space:
//! * `bmodelSet`: brush-model sounds. `CS_AMBIENT_SET` names one; `EV_PLAYDOORSOUND`,
//!   `EV_PLAYDOORLOOPSOUND`, `EV_BMODEL_SOUND` and soundset `loopSound`s index its
//!   `subWaves` (BMS_START / BMS_MID / BMS_END = 0 / 1 / 2).
//! * `generalSet`: level ambience. The worldspawn `soundSet` is sent as
//!   `CS_GLOBAL_AMBIENT_SET`; cgame plays it at the listener every frame and
//!   crossfades when it changes (`S_UpdateAmbientSet`).
//! * `localSet`: regional ambience. Entities with a `soundSetIndex` (other than
//!   movers) play it around their origin (`S_AddLocalSet`).

use std::collections::HashMap;

pub const AMBIENT_SET_FILENAME: &str = "sound/sound.txt";
pub const CS_AMBIENT_SET: u16 = 37;
pub const CS_GLOBAL_AMBIENT_SET: u16 = 32;
pub const BMS_MID: i32 = 1;
const MAX_WAVES_PER_GROUP: usize = 8;
const MAX_SET_VOLUME: i32 = 255;
/// `crossDelay`: milliseconds a general set takes to fade in/out.
const CROSS_DELAY_MS: i32 = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetKind {
    General,
    Local,
    BModel,
}

#[derive(Debug, Clone)]
pub(crate) struct AmbientSet {
    kind: SetKind,
    /// Seconds between one-shot subwaves.
    time_start: i32,
    time_end: i32,
    vol_start: i32,
    vol_end: i32,
    /// `sound/<dir>/<wave>.wav` per subWave.
    sub_waves: Vec<String>,
    /// `sound/<name>.wav`.
    looped_wave: Option<String>,
    radius: i32,
}

impl AmbientSet {
    fn new(kind: SetKind) -> Self {
        Self {
            kind,
            time_start: 10,
            time_end: 25,
            vol_start: MAX_SET_VOLUME,
            vol_end: MAX_SET_VOLUME,
            sub_waves: Vec::new(),
            looped_wave: None,
            radius: 250,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AmbientSets {
    /// Lower-cased set name -> set.
    sets: HashMap<String, AmbientSet>,
}

fn two_ints<'a>(mut tokens: impl Iterator<Item = &'a str>) -> Option<(i32, i32)> {
    let a = tokens.next()?.parse().ok()?;
    let b = tokens.next()?.parse().ok()?;
    Some((a, b))
}

impl AmbientSets {
    pub(crate) fn parse(text: &str) -> Self {
        let mut sets: HashMap<String, AmbientSet> = HashMap::new();
        let mut current: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            let mut tokens = line.split_whitespace();
            let Some(first) = tokens.next() else { continue };
            if first.starts_with("//") || first.starts_with(';') {
                continue;
            }
            let kind = match first {
                "generalSet" => Some(SetKind::General),
                "localSet" => Some(SetKind::Local),
                "bmodelSet" => Some(SetKind::BModel),
                _ => None,
            };
            if let Some(kind) = kind {
                current = tokens.next().map(str::to_ascii_lowercase);
                if let Some(name) = &current {
                    sets.insert(name.clone(), AmbientSet::new(kind));
                }
                continue;
            }
            let Some(set) = current.as_ref().and_then(|name| sets.get_mut(name)) else { continue };
            match first {
                "timeBetweenWaves" => {
                    if let Some((mut start, mut end)) = two_ints(tokens) {
                        if start > end {
                            std::mem::swap(&mut start, &mut end);
                        }
                        set.time_start = start;
                        set.time_end = end;
                    }
                }
                "subWaves" => {
                    let Some(dir) = tokens.next() else { continue };
                    for wave in tokens {
                        if set.sub_waves.len() >= MAX_WAVES_PER_GROUP {
                            break;
                        }
                        set.sub_waves.push(format!("sound/{dir}/{wave}.wav"));
                    }
                }
                "loopedWave" => {
                    if let Some(name) = tokens.next() {
                        set.looped_wave = Some(format!("sound/{name}.wav"));
                    }
                }
                "volRange" => {
                    if let Some((mut low, mut high)) = two_ints(tokens) {
                        if low > high {
                            std::mem::swap(&mut low, &mut high);
                        }
                        set.vol_start = low;
                        set.vol_end = high;
                    }
                }
                "radius" => {
                    if let Some(radius) = tokens.next().and_then(|value| value.parse().ok()) {
                        set.radius = radius;
                    }
                }
                _ => {}
            }
        }
        Self { sets }
    }

    /// `AS_GetBModelSound(name, stage)`.
    pub(crate) fn bmodel_sound(&self, set: &str, stage: i32) -> Option<&str> {
        let stage = usize::try_from(stage).ok()?;
        let set = self.sets.get(&set.to_ascii_lowercase()).filter(|set| set.kind == SetKind::BModel)?;
        set.sub_waves.get(stage).map(String::as_str)
    }

    fn get(&self, name: &str, kind: SetKind) -> Option<&AmbientSet> {
        self.sets.get(&name.trim().to_ascii_lowercase()).filter(|set| set.kind == kind)
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.sets.is_empty()
    }
}

/// One looping ambient sound this frame (`S_AddAmbientLoopingSound`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AmbientLoop {
    pub qpath: String,
    pub origin: [f32; 3],
    /// 0..=255, as in the engine.
    pub volume: u8,
}

/// One ambient one-shot (`S_StartAmbientSound`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AmbientShot {
    pub qpath: String,
    pub origin: [f32; 3],
    pub entity: u16,
    pub volume: u8,
}

#[derive(Debug, Default)]
pub(crate) struct AmbientFrame {
    pub loops: Vec<AmbientLoop>,
    pub shots: Vec<AmbientShot>,
}

/// Cross-frame state: the current/old general set and their fades, plus the
/// per-set and per-entity one-shot timers.
#[derive(Debug, Default)]
pub(crate) struct AmbientRuntime {
    current: Option<String>,
    old: Option<String>,
    /// name -> (masterVolume, fadeTime)
    volumes: HashMap<String, (i32, i32)>,
    current_time: i32,
    old_time: i32,
    local_times: HashMap<u16, i32>,
    rng: u32,
}

impl AmbientRuntime {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    fn irand(&mut self, low: i32, high: i32) -> i32 {
        if self.rng == 0 {
            self.rng = 0x2545_F491;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        if high <= low {
            return low;
        }
        low + (self.rng % (high - low + 1) as u32) as i32
    }

    /// `S_UpdateAmbientSet`: play the level's general set at `origin`, crossfading
    /// when the name changes. `now` is wall-clock milliseconds (`cls.realtime`).
    pub(crate) fn update_global(&mut self, sets: &AmbientSets, name: &str, origin: [f32; 3], now: i32, out: &mut AmbientFrame) {
        let name = name.trim().to_ascii_lowercase();
        if sets.get(&name, SetKind::General).is_none() {
            return;
        }
        // AS_UpdateCurrentSet.
        if self.current.as_deref() != Some(name.as_str()) {
            self.old = self.current.take();
            self.current = Some(name.clone());
            if let Some(old) = &self.old {
                self.volumes.insert(old.clone(), (MAX_SET_VOLUME, now));
            }
            self.volumes.insert(name.clone(), (0, now));
        }
        // AS_UpdateSetVolumes.
        if let Some(current) = &self.current {
            if let Some((master, fade_time)) = self.volumes.get_mut(current) {
                if *master < MAX_SET_VOLUME {
                    let scale = (now - *fade_time) as f32 / CROSS_DELAY_MS as f32;
                    *master = (scale * MAX_SET_VOLUME as f32) as i32;
                }
                *master = (*master).min(MAX_SET_VOLUME);
            }
        }
        if let Some(old) = self.old.clone() {
            let expired = match self.volumes.get_mut(&old) {
                Some((master, fade_time)) => {
                    if *master > 0 {
                        let scale = (now - *fade_time) as f32 / CROSS_DELAY_MS as f32;
                        *master = MAX_SET_VOLUME - (scale * MAX_SET_VOLUME as f32) as i32;
                    }
                    if *master <= 0 {
                        *master = 0;
                        true
                    } else {
                        false
                    }
                }
                None => true,
            };
            if expired {
                self.old = None;
            }
        }
        let current = self.current.clone();
        let old = self.old.clone();
        if let Some(current) = current {
            let mut last = self.current_time;
            self.play_global(sets, &current, origin, now, &mut last, out);
            self.current_time = last;
        }
        if let Some(old) = old {
            let mut last = self.old_time;
            self.play_global(sets, &old, origin, now, &mut last, out);
            self.old_time = last;
        }
    }

    /// AS_PlayAmbientSet.
    fn play_global(&mut self, sets: &AmbientSets, name: &str, origin: [f32; 3], now: i32, last: &mut i32, out: &mut AmbientFrame) {
        let Some(set) = sets.get(name, SetKind::General) else { return };
        let master = self.volumes.get(name).map_or(MAX_SET_VOLUME, |&(master, _)| master);
        if let Some(wave) = &set.looped_wave {
            if master > 0 {
                out.loops.push(AmbientLoop { qpath: wave.clone(), origin, volume: master.clamp(0, 255) as u8 });
            }
        }
        if now - *last < self.irand(set.time_start, set.time_end) * 1000 {
            return;
        }
        *last = now;
        let scale = master as f32 / MAX_SET_VOLUME as f32;
        let volume = self.irand((scale * set.vol_start as f32) as i32, (scale * set.vol_end as f32) as i32).min(master);
        if !set.sub_waves.is_empty() && volume > 0 {
            let wave = &set.sub_waves[self.irand(0, set.sub_waves.len() as i32 - 1) as usize];
            // Entity 0 in the engine; a non-client number keeps it off the head-locked path.
            out.shots.push(AmbientShot { qpath: wave.clone(), origin, entity: 1022, volume: volume.clamp(0, 255) as u8 });
        }
    }

    /// `S_AddLocalSet` / AS_PlayLocalSet for one entity. `now` is server time.
    ///
    /// The one-shot timer is kept per entity, as stock OpenJK does with
    /// `cent->miscTime` (TaystJK passes `cg.time` and never fires subwaves).
    pub(crate) fn update_local(
        &mut self,
        sets: &AmbientSets,
        name: &str,
        listener: [f32; 3],
        origin: [f32; 3],
        entity: u16,
        now: i32,
        out: &mut AmbientFrame,
    ) {
        let Some(set) = sets.get(name, SetKind::Local) else { return };
        let dist = (0..3).map(|i| (origin[i] - listener[i]).powi(2)).sum::<f32>().sqrt();
        let radius = set.radius as f32;
        let dist_scale = if dist < radius * 0.5 { 1.0 } else { (radius - dist) / (radius * 0.5) };
        let mut volume = if !(0.0..=1.0).contains(&dist_scale) { 0 } else { (MAX_SET_VOLUME as f32 * dist_scale) as i32 };
        if let Some(wave) = &set.looped_wave {
            if volume > 0 {
                out.loops.push(AmbientLoop { qpath: wave.clone(), origin, volume: volume.clamp(0, 255) as u8 });
            }
        }
        let last = self.local_times.get(&entity).copied().unwrap_or(0);
        if now - last < self.irand(set.time_start, set.time_end) * 1000 {
            return;
        }
        self.local_times.insert(entity, now);
        let scale = volume as f32 / MAX_SET_VOLUME as f32;
        volume = self.irand((scale * set.vol_start as f32) as i32, (scale * set.vol_end as f32) as i32);
        if !set.sub_waves.is_empty() && volume > 0 {
            let wave = &set.sub_waves[self.irand(0, set.sub_waves.len() as i32 - 1) as usize];
            out.shots.push(AmbientShot { qpath: wave.clone(), origin, entity, volume: volume.clamp(0, 255) as u8 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "type ambientSet\n\
        generalSet\tyavin_swamp\n\
        timeBetweenWaves 3 1\n\
        subWaves ambience/swamp frog1 frog2\n\
        loopedWave ambience/swamp/loop\n\
        volRange 100 200\n\
        localSet\tdrip\n\
        radius 100\n\
        subWaves ambience/drip a\n\
        bmodelSet\tdoor_metal\n\
        subWaves movers/doors dr1_start dr1_mid dr1_end\n";

    #[test]
    fn bmodel_sets_resolve_stages_and_ignore_other_kinds() {
        let sets = AmbientSets::parse(TEXT);
        assert_eq!(sets.bmodel_sound("DOOR_METAL", 0), Some("sound/movers/doors/dr1_start.wav"));
        assert_eq!(sets.bmodel_sound("door_metal", 2), Some("sound/movers/doors/dr1_end.wav"));
        assert_eq!(sets.bmodel_sound("door_metal", 3), None);
        assert_eq!(sets.bmodel_sound("yavin_swamp", 0), None, "a generalSet is not a bmodel sound");
        assert!(!sets.is_empty());
    }

    #[test]
    fn general_and_local_sets_parse_their_keywords() {
        let sets = AmbientSets::parse(TEXT);
        let swamp = sets.get("Yavin_Swamp", SetKind::General).unwrap();
        assert_eq!((swamp.time_start, swamp.time_end), (1, 3), "swapped times are corrected");
        assert_eq!((swamp.vol_start, swamp.vol_end), (100, 200));
        assert_eq!(swamp.looped_wave.as_deref(), Some("sound/ambience/swamp/loop.wav"));
        assert_eq!(swamp.sub_waves, ["sound/ambience/swamp/frog1.wav", "sound/ambience/swamp/frog2.wav"]);
        assert_eq!(sets.get("drip", SetKind::Local).unwrap().radius, 100);
        assert!(sets.get("drip", SetKind::General).is_none());
    }

    #[test]
    fn global_set_fades_in_loops_and_crossfades_on_change() {
        let sets = AmbientSets::parse(&format!("{TEXT}generalSet\tother\nloopedWave ambience/other\n"));
        let mut runtime = AmbientRuntime::default();
        let mut frame = AmbientFrame::default();
        runtime.update_global(&sets, "yavin_swamp", [1.0; 3], 10_000, &mut frame);
        assert!(frame.loops.is_empty(), "starts silent");
        let mut frame = AmbientFrame::default();
        runtime.update_global(&sets, "yavin_swamp", [1.0; 3], 10_500, &mut frame);
        assert_eq!(frame.loops.len(), 1);
        assert_eq!(frame.loops[0].volume, 127, "half faded in");
        let mut frame = AmbientFrame::default();
        runtime.update_global(&sets, "yavin_swamp", [1.0; 3], 11_200, &mut frame);
        assert_eq!(frame.loops[0].volume, 255);
        // Switching sets crossfades: the old one fades out while the new fades in.
        runtime.update_global(&sets, "other", [1.0; 3], 12_000, &mut AmbientFrame::default());
        let mut frame = AmbientFrame::default();
        runtime.update_global(&sets, "other", [1.0; 3], 12_500, &mut frame);
        let old = frame.loops.iter().find(|l| l.qpath.contains("swamp")).expect("old set still fading");
        let new = frame.loops.iter().find(|l| l.qpath.contains("other")).expect("new set fading in");
        assert!(old.volume < 255 && new.volume < 255);
        let mut frame = AmbientFrame::default();
        runtime.update_global(&sets, "other", [1.0; 3], 13_100, &mut frame);
        assert!(frame.loops.iter().all(|l| !l.qpath.contains("swamp")), "old set is gone");
    }

    #[test]
    fn local_sets_attenuate_by_radius_and_keep_a_timer_per_entity() {
        let sets = AmbientSets::parse(TEXT);
        let mut runtime = AmbientRuntime::default();
        let mut frame = AmbientFrame::default();
        // radius 100: full inside 50, faded to nothing at 100.
        runtime.update_local(&sets, "drip", [0.0; 3], [200.0, 0.0, 0.0], 7, 100_000, &mut frame);
        assert!(frame.shots.is_empty() && frame.loops.is_empty(), "out of radius is silent");
        runtime.update_local(&sets, "drip", [0.0; 3], [25.0, 0.0, 0.0], 8, 100_000, &mut frame);
        assert_eq!(frame.shots.len(), 1, "first subwave fires straight away");
        assert_eq!(frame.shots[0].entity, 8);
        assert_eq!(frame.shots[0].volume, 255);
        let mut frame = AmbientFrame::default();
        runtime.update_local(&sets, "drip", [0.0; 3], [25.0, 0.0, 0.0], 8, 101_000, &mut frame);
        assert!(frame.shots.is_empty(), "the next one waits 10-25 s");
    }
}
