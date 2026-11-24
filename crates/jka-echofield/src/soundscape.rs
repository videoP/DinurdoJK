//! World-space soundscape policy. BSP contacts play the role of upstream
//! voxel candidates. Playback and the source budget are owned by jka-audio.
use crate::{
    environment::{EnvironmentConditions, EnvironmentInputs},
    excitation::{self, Kind},
    geometry::Vec3,
    procedural::{self, Category},
    profile::{add, length, scale, sub, EnvironmentProfile, SpectralCategory},
    random::JavaRandom,
    scheduler::{SampleScheduler, TriggerKind},
    settings::Settings,
    terrain::{Family, TerrainFeature},
    terrain_audio,
    world::Contact,
};
use std::collections::{HashMap, HashSet};

const MAX_EMITTERS: usize = 32;
const MAX_TERRAIN: usize = 12;

#[derive(Clone, Copy, PartialEq)]
pub struct Weather {
    pub hour: f32,
    pub raining: bool,
    pub storm: bool,
    pub wind_metres_per_second: f32,
    pub temperature: f32,
    pub submerged: bool,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            hour: 12.0,
            raining: false,
            storm: false,
            wind_metres_per_second: 0.0,
            temperature: 0.7,
            submerged: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Clip {
    Excitation(Kind),
    Emitter(Category, u8),
    Terrain(Family, u8),
}

impl Clip {
    pub fn generate(self) -> Option<Vec<f32>> {
        match self {
            Self::Excitation(kind) => Some(excitation::generate(kind)),
            Self::Terrain(family, variant) => Some(terrain_audio::generate(family, variant.into())),
            Self::Emitter(category, variant) if !category.info().2 => {
                let mut waveform =
                    procedural::generate_loop(category, variant.into(), 0x4264_6934)?;
                terrain_audio::normalize(&mut waveform, 0.75);
                Some(waveform)
            }
            Self::Emitter(category, variant) => {
                let mut rng =
                    JavaRandom::new(0x2879_3921 ^ ((category as i64) << 32) ^ i64::from(variant));
                let mut waveform = vec![0.0; procedural::RATE * 4];
                let count = waveform.len();
                // Fauna is excluded. Water one-shots play recordings only: the
                // synthesized Minnaert bubbles read as isolated pitched pings.
                match category {
                    // Not normalized: the drips that set the upstream peak are
                    // omitted, and rescaling would boost the quiet bed.
                    Category::CaveAmbient => {
                        procedural::generate_cave_ambient(&mut waveform, &mut rng, 10.0, count)
                    }
                    Category::Mechanical => {
                        procedural::generate_fire(&mut waveform, &mut rng, count);
                        terrain_audio::normalize(&mut waveform, 0.75);
                    }
                    _ => return None,
                }
                Some(waveform)
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct Voice {
    pub id: u64,
    pub origin: Vec3,
    pub gain: f32,
    pub gain_hf: f32,
    pub range: f32,
    pub speed: f32,
    pub clip: Clip,
}

pub struct Frame {
    pub loops: Vec<Voice>,
    pub triggered: Vec<Voice>,
}

struct Emitter {
    voice: Voice,
    category: Category,
    next_play: u64,
    last_variant: u8,
}

pub struct Soundscape {
    emitters: Vec<Emitter>,
    terrain: Vec<Voice>,
    scheduler: SampleScheduler,
    rng: JavaRandom,
    excitation: [Option<Voice>; 5],
}

impl Default for Soundscape {
    fn default() -> Self {
        Self {
            emitters: Vec::new(),
            terrain: Vec::new(),
            scheduler: SampleScheduler::default(),
            rng: JavaRandom::new(0x7b34_e891),
            excitation: [None; 5],
        }
    }
}

fn spatial_id(position: Vec3, salt: u64) -> u64 {
    let mut id = salt;
    for coordinate in position {
        let cell = (coordinate / 2.0).floor() as i32;
        id = (id ^ u64::from(cell as u32)).wrapping_mul(0x100_0000_01b3);
    }
    id
}

fn mean_contacts(contacts: &[Contact], matches: impl Fn(Contact) -> bool) -> Option<(Vec3, usize)> {
    let (sum, count) = contacts
        .iter()
        .copied()
        .filter(|contact| matches(*contact))
        .fold(([0.0; 3], 0), |(sum, count), contact| {
            (add(sum, contact.position), count + 1)
        });
    (count != 0).then(|| (scale(sum, 1.0 / count as f64), count))
}

impl Soundscape {
    pub fn scan(
        &mut self,
        listener: Vec3,
        profile: &EnvironmentProfile,
        contacts: &[Contact],
        mut features: Vec<TerrainFeature<SpectralCategory>>,
        settings: Settings,
    ) {
        let mut candidates = Vec::with_capacity(contacts.len());
        let mut unique = HashSet::new();
        for &contact in contacts {
            let distance = length(sub(listener, contact.position)) as f32;
            let hash = spatial_id(contact.position, 137);
            let category = if contact.lava() {
                Some(Category::Lava)
            } else if contact.water() {
                // EmitterScanner.java: about 3% of still surfaces (hash < 8 of
                // 256). A BSP contact carries no current or overhead-drip
                // evidence, and the hydraulic river layer owns flow sounds.
                (!contact.river && (hash >> 8) & 255 < 8).then_some(Category::WaterStill)
            } else {
                match contact.material {
                    19 | 20 => Some(Category::WindLeaf),
                    5 | 6 => Some(Category::WindGrass),
                    15 => Some(Category::Mechanical),
                    // EmitterScanner.java places WATER_DRIP only under water in
                    // dark air. A dry BSP ceiling carries no such evidence, so
                    // enclosed ceilings get cave ambience, never drips.
                    _ if profile.enclosure > 0.82 && contact.normal[1] < -0.35 => {
                        if hash % 4 == 0 {
                            Some(Category::CaveAmbient)
                        } else {
                            Some(Category::CaveDrone)
                        }
                    }
                    _ if profile.wind_exposure > 0.15
                        && contact.normal[1].abs() < 0.2
                        && hash % 11 == 0 =>
                    {
                        Some(Category::WindWhistle)
                    }
                    _ if contact.normal[1] > 0.5 && hash % 9 == 0 => Some(Category::WaterRain),
                    _ => None,
                }
            };
            let position = add(contact.position, scale(contact.normal, 0.1));
            if let Some(category) = category {
                if !settings.emitter(category) {
                    continue;
                }
                let (range, priority, _) = category.info();
                let id = spatial_id(position, 0x4f02 + category as u64);
                if distance < range
                    && unique.insert(id)
                    && (distance < range * 0.5
                        || (hash & 255) as f32 <= settings.emitter_density * 255.0)
                {
                    let voice = Voice {
                        id,
                        origin: position,
                        gain: 0.0,
                        gain_hf: 1.0,
                        range,
                        speed: 1.0,
                        clip: Clip::Emitter(category, (hash % 4) as u8),
                    };
                    candidates.push((f32::from(priority) / (1.0 + distance), category, voice));
                }
            }
        }
        candidates.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.2.id.cmp(&b.2.id)));
        features.sort_by(|a, b| {
            let score = |f: &TerrainFeature<SpectralCategory>| f.saliency * f.kind.family_weight();
            score(b).total_cmp(&score(a))
        });
        self.terrain.clear();
        for feature in features {
            if !settings.enhancements || !settings.terrain {
                break;
            }
            let family = feature.family();
            let position = feature.position.map(f64::from);
            let range = feature.kind.max_range().min(settings.terrain_range as f32);
            if length(sub(listener, position)) >= f64::from(range) {
                continue;
            }
            let id = spatial_id(position, 0x7ca3 + feature.kind as u64);
            if !unique.insert(id)
                || self.terrain.iter().any(|voice| {
                    matches!(voice.clip, Clip::Terrain(previous, _) if previous == family)
                        && length(sub(voice.origin, position)) < 2.0
                })
            {
                continue;
            }
            let variant =
                terrain_audio::variant_for_position(position[0] as i32, position[2] as i32);
            self.terrain.push(Voice {
                id,
                origin: add(position, scale(feature.normal.map(f64::from), 0.08)),
                gain: family.base_gain() * feature.saliency.sqrt(),
                gain_hf: 1.0 - feature.material.absorption()[2],
                range,
                speed: 1.0,
                clip: Clip::Terrain(family, variant as u8),
            });
            if self.terrain.len() == MAX_TERRAIN {
                break;
            }
        }
        let mut previous: HashMap<_, _> = self
            .emitters
            .drain(..)
            .map(|emitter| (emitter.voice.id, emitter))
            .collect();
        let mut counts = [0; 15];
        for (_, category, voice) in candidates {
            let limit = match category {
                Category::WindLeaf | Category::WaterFlow => 6,
                Category::WindGrass | Category::Lava => 4,
                Category::WindWhistle => 2,
                Category::CaveDrone => 3,
                _ => 4,
            };
            if counts[category as usize] >= limit {
                continue;
            }
            counts[category as usize] += 1;
            let mut emitter = previous.remove(&voice.id).unwrap_or(Emitter {
                voice,
                category,
                next_play: 0,
                last_variant: 0,
            });
            emitter.voice.origin = voice.origin;
            self.emitters.push(emitter);
            if self.emitters.len() >= MAX_EMITTERS {
                break;
            }
        }
        for &id in previous.keys() {
            self.scheduler.remove(id);
        }
    }

    pub fn tick(
        &mut self,
        tick: u64,
        listener: Vec3,
        profile: &EnvironmentProfile,
        contacts: &[Contact],
        weather: Weather,
        settings: Settings,
    ) -> Frame {
        let mut output = Frame {
            loops: Vec::new(),
            triggered: Vec::new(),
        };
        if weather.submerged || !settings.enhancements || settings.master_volume == 0.0 {
            return output;
        }
        let wind_scale = (1.0 + weather.wind_metres_per_second * 0.08).clamp(0.25, 2.5);
        let wind = (profile.wind_exposure * wind_scale).min(1.0);
        let conditions = EnvironmentConditions::from(EnvironmentInputs {
            day_time: ((weather.hour - 6.0).rem_euclid(24.0) * 1000.0) as i64,
            is_raining: weather.raining,
            is_thundering: weather.storm,
            temperature: weather.temperature,
            is_forest: profile.canopy > 0.15,
            is_swamp: contacts.iter().any(|contact| contact.water()),
            is_desert: !contacts.is_empty()
                && contacts
                    .iter()
                    .filter(|contact| contact.material == 14)
                    .count()
                    * 2
                    > contacts.len(),
            wind_exposure: wind,
            enclosure: profile.enclosure,
            room_size: profile.average_return_distance,
        });
        let excitation = excitation::targets(
            profile,
            listener,
            mean_contacts(contacts, |c| matches!(c.material, 19 | 20)),
            mean_contacts(contacts, |c| matches!(c.material, 5 | 6)),
            mean_contacts(contacts, |contact| contact.water() && !contact.river),
            mean_contacts(contacts, Contact::lava),
        );
        for (idx, (kind, target)) in Kind::ALL.into_iter().zip(excitation).enumerate() {
            if !settings.excitation(kind) {
                self.excitation[idx] = None;
                continue;
            }
            // EapSystem.java: with the emitter layer enabled, foliage, grass,
            // water and lava excitation are silenced to prevent double audio.
            let duplicated = kind != Kind::Wind && settings.emitters;
            let mut voice = self.excitation[idx].unwrap_or(Voice {
                id: idx as u64 + 1,
                origin: target.position,
                gain: 0.0,
                gain_hf: target.gain_hf,
                range: 64.0,
                speed: 1.0,
                clip: Clip::Excitation(kind),
            });
            // Weather wind drives only the wind-excited layers. Water and lava
            // are self-excited upstream.
            let weather_scale = match kind {
                Kind::Wind | Kind::Foliage | Kind::Grass => wind_scale,
                Kind::Water | Kind::Lava => 1.0,
            };
            let gain = if duplicated {
                0.0
            } else {
                (target.gain * weather_scale).min(1.0)
                    * settings.master_volume
                    * settings.excitation_volume
            };
            voice.gain += (gain - voice.gain) * 0.1;
            voice.origin = add(voice.origin, scale(sub(target.position, voice.origin), 0.1));
            voice.gain_hf += (target.gain_hf - voice.gain_hf) * 0.1;
            self.excitation[idx] = Some(voice);
            if voice.gain > 0.001 {
                output.loops.push(voice);
            }
        }
        for emitter in &mut self.emitters {
            if !settings.emitter(emitter.category) {
                continue;
            }
            let distance = length(sub(listener, emitter.voice.origin)) as f32;
            if distance >= emitter.voice.range {
                continue;
            }
            let (gain, trigger) = emitter_gain(emitter.category, &conditions);
            if gain <= 0.001 {
                continue;
            }
            emitter.voice.gain = gain * settings.master_volume;
            emitter.voice.gain_hf = (1.0 - distance / emitter.voice.range * 0.6).max(0.25);
            let Some(trigger) = trigger else {
                output.loops.push(emitter.voice);
                continue;
            };
            if tick < emitter.next_play {
                continue;
            }
            let triggered = self
                .scheduler
                .should_trigger(emitter.voice.id, trigger, &conditions, &mut self.rng);
            if triggered {
                let mut voice = emitter.voice;
                let Clip::Emitter(_, group) = voice.clip else {
                    continue;
                };
                let variation = (emitter.last_variant + 1 + self.rng.next_int(7) as u8) % 8;
                emitter.last_variant = variation;
                voice.clip = Clip::Emitter(emitter.category, group + 4 * variation);
                voice.speed = 0.95 + self.rng.next_float() * 0.10;
                voice.gain *= 10.0_f32.powf((self.rng.next_float() * 4.0 - 2.0) / 20.0);
                output.triggered.push(voice);
                emitter.next_play = tick + 80;
            }
        }
        output
            .loops
            .extend(self.terrain.iter().copied().map(|mut voice| {
                voice.gain *= (0.35 + 0.65 * profile.enclosure)
                    * settings.master_volume
                    * settings.terrain_intensity;
                voice
            }));
        output
    }
}

fn emitter_gain(category: Category, c: &EnvironmentConditions) -> (f32, Option<TriggerKind>) {
    let calm = 0.6 + c.daylight * 0.4;
    let wind = c.wind_exposure;
    match category {
        Category::WindLeaf => (wind * 0.55 * calm, None),
        Category::WindGrass => (wind * 0.45 * calm, None),
        Category::WindWhistle => (wind * wind * 0.3, None),
        Category::WaterFlow => (0.55, None),
        Category::Lava => (0.4, None),
        Category::CaveDrone => (0.03 * c.enclosure, None),
        Category::Insect => (
            0.35 * c.temperature * (1.0 - c.daylight * 0.8),
            Some(TriggerKind::Insect),
        ),
        Category::Frog => (0.35 * (1.0 - c.daylight * 0.6), Some(TriggerKind::Frog)),
        Category::WaterDrip => (0.50, Some(TriggerKind::WaterDrip)),
        Category::WaterRain => (
            if c.is_raining { wind * 0.45 } else { 0.0 },
            Some(TriggerKind::WaterRain),
        ),
        Category::WaterStill => (0.25, Some(TriggerKind::WaterStill)),
        Category::CaveAmbient => (0.20 * c.enclosure, Some(TriggerKind::CaveAmbient)),
        Category::Mechanical => (0.35, Some(TriggerKind::Mechanical)),
        Category::Bird | Category::Bat => (0.0, None),
    }
}
