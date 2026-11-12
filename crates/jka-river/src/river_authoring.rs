//! Local patch identity, independent materials and source-edit transactions.
use crate::{
    river_runtime::{LocalDrain, LocalSource, SourceMode},
    river_settings::Settings,
};

pub type Id = u64;

/// Controls that can change without recapturing the bed or resetting water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveControls {
    pub mode: SourceMode,
    pub discharge_cfs: Option<f32>,
    pub speed: f32,
}
impl LiveControls {
    pub fn from_source(source: LocalSource) -> Self {
        Self {
            mode: source.mode,
            discharge_cfs: source.discharge_cfs,
            speed: source.rate,
        }
    }
    pub fn apply(self, mut source: LocalSource) -> Result<LocalSource, String> {
        source.mode = self.mode;
        source.discharge_cfs = self.discharge_cfs;
        source.rate = self.speed;
        source.validate()?;
        Ok(source)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceParameters {
    pub height: f32,
    pub radius: f32,
    pub width: f32,
    pub cells: u32,
    pub speed: f32,
    /// Selected UI mode; not inferred from the presence of an optional CFS cap.
    pub mode: SourceMode,
    /// In WaterLevel mode, None means unlimited, Some(q) caps inflow.
    /// In Discharge mode, Some(q) is required.
    pub discharge_cfs: Option<f32>,
}
impl Default for SourceParameters {
    fn default() -> Self {
        Self {
            height: 48.0,
            radius: 1024.0,
            width: 96.0,
            cells: 128,
            speed: 1.0,
            mode: SourceMode::WaterLevel,
            discharge_cfs: None,
        }
    }
}
impl SourceParameters {
    pub fn live(self) -> LiveControls {
        LiveControls {
            mode: self.mode,
            discharge_cfs: self.discharge_cfs,
            speed: self.speed,
        }
    }
    pub fn at(self, center: [f32; 3]) -> LocalSource {
        LocalSource {
            center,
            radius: self.radius,
            water_level: center[2] + self.height,
            source_width: self.width,
            resolution: self.cells,
            rate: self.speed,
            mode: self.mode,
            discharge_cfs: self.discharge_cfs,
        }
    }
    pub fn from_source(s: LocalSource) -> Self {
        Self {
            height: s.water_level - s.center[2],
            radius: s.radius,
            width: s.source_width,
            cells: s.resolution,
            speed: s.rate,
            mode: s.mode,
            discharge_cfs: s.discharge_cfs,
        }
    }
}

/// Drain controls (spawndrain / River menu): everything is live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrainParameters {
    pub radius: f32,
    /// None: floor drain (empties). Some(h): overflow drain holding water at
    /// h map units above the placement point.
    pub overflow_height: Option<f32>,
    /// None: unlimited. Some(q): at most q cubic feet per simulated second.
    pub max_cfs: Option<f32>,
}
impl Default for DrainParameters {
    fn default() -> Self {
        Self { radius: 32.0, overflow_height: None, max_cfs: None }
    }
}
impl DrainParameters {
    pub fn at(self, center: [f32; 3]) -> LocalDrain {
        LocalDrain {
            center,
            radius: self.radius,
            level: self.overflow_height.map(|h| center[2] + h),
            max_cfs: self.max_cfs,
        }
    }
    pub fn from_drain(d: LocalDrain) -> Self {
        Self { radius: d.radius, overflow_height: d.level.map(|z| z - d.center[2]), max_cfs: d.max_cfs }
    }
}

pub struct DrainEntry {
    pub id: Id,
    pub name: String,
    pub drain: LocalDrain,
}

#[derive(Clone, Copy, Debug)]
pub struct Definition {
    pub id: Id,
    pub source: LocalSource,
    pub appearance: Settings,
}

pub struct Entry {
    pub definition: Definition,
    pub name: String,
    pub draft: SourceParameters,
    pub pending: bool,
    pub error: Option<String>,
}
pub struct Editor {
    pub entries: Vec<Entry>,
    pub selected: Option<Id>,
    pub defaults: SourceParameters,
    /// Drains apply to every river; edits are live.
    pub drains: Vec<DrainEntry>,
    pub selected_drain: Option<Id>,
    pub drain_defaults: DrainParameters,
    next_id: Id,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            defaults: SourceParameters::default(),
            drains: Vec::new(),
            selected_drain: None,
            drain_defaults: DrainParameters::default(),
            next_id: 1,
        }
    }
}
impl Editor {
    pub fn add(&mut self, source: LocalSource, appearance: Settings) -> Definition {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("river IDs exhausted");
        let definition = Definition {
            id,
            source,
            appearance,
        };
        self.entries.push(Entry {
            definition,
            name: format!("River {id}"),
            draft: SourceParameters::from_source(source),
            pending: true,
            error: None,
        });
        self.selected = Some(id);
        definition
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.selected = None;
        self.drains.clear();
        self.selected_drain = None;
    }
    pub fn add_drain(&mut self, drain: LocalDrain) -> Id {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("river IDs exhausted");
        let number = self.drains.len() + 1;
        self.drains.push(DrainEntry { id, name: format!("Drain {number}"), drain });
        self.selected_drain = Some(id);
        id
    }
    /// The drains every river receives (RenderCommand::RiverDrains).
    pub fn drain_list(&self) -> Vec<LocalDrain> {
        self.drains.iter().map(|d| d.drain).collect()
    }
    pub fn entry_mut(&mut self, id: Id) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.definition.id == id)
    }
    pub fn finish(&mut self, id: Id, result: Result<LocalSource, String>) {
        if let Some(entry) = self.entry_mut(id) {
            entry.pending = false;
            match result {
                Ok(source) => {
                    entry.definition.source = source;
                    entry.error = None;
                }
                Err(error) => entry.error = Some(error),
            }
        }
    }

    pub fn finish_live(
        &mut self,
        id: Id,
        requested: LiveControls,
        source: LocalSource,
        error: Option<String>,
    ) {
        if let Some(entry) = self.entry_mut(id) {
            // Newer drags, rebuilds and removals supersede stale replies.
            let mut same_layout = entry.definition.source;
            same_layout.mode = source.mode;
            same_layout.rate = source.rate;
            same_layout.discharge_cfs = source.discharge_cfs;
            if same_layout == source && entry.draft.live() == requested {
                entry.definition.source = source;
                entry.error = error;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn river_rebuild_failure_preserves_source_and_material_and_ids_do_not_recycle() {
        let mut editor = Editor::default();
        let source = editor.defaults.at([12.0, 34.0, 56.0]);
        let a = editor.add(source, Settings::default());
        editor.finish(a.id, Ok(source));
        let b = editor.add(source, Settings::default());
        editor
            .entry_mut(a.id)
            .unwrap()
            .definition
            .appearance
            .foam
            .base_color = [1.0; 3];
        editor.finish(a.id, Err("invalid source".into()));
        let first = editor.entry_mut(a.id).unwrap();
        assert_eq!(first.definition.source, source);
        assert_eq!(first.definition.appearance.foam.base_color, [1.0; 3]);
        assert_ne!(
            editor
                .entry_mut(b.id)
                .unwrap()
                .definition
                .appearance
                .foam
                .base_color,
            [1.0; 3]
        );
        editor.clear();
        let c = editor.add(source, Settings::default());
        assert!(c.id > b.id);
        editor.finish(a.id, Ok(source));
        assert_eq!(editor.entries.len(), 1);
    }
    #[test]
    fn river_height_mode_with_cfs_roundtrips_without_becoming_discharge_mode() {
        let defaults = SourceParameters::default();
        assert_eq!(defaults.mode, SourceMode::WaterLevel);
        assert_eq!(defaults.discharge_cfs, None);
        let limited = SourceParameters {
            mode: SourceMode::WaterLevel,
            discharge_cfs: Some(500.0),
            ..defaults
        };
        let source = limited.at([0.0, 20.0, 50.0]);
        assert_eq!(source.mode, SourceMode::WaterLevel);
        assert_eq!(source.discharge_cfs, Some(500.0));
        assert!(source.validate().is_ok());
        assert_eq!(SourceParameters::from_source(source), limited);
        let mut editor = Editor::default();
        let definition = editor.add(source, Settings::default());
        editor.finish(definition.id, Ok(source));
        let entry = editor.entry_mut(definition.id).unwrap();
        assert_eq!(entry.draft.mode, SourceMode::WaterLevel);
        assert_eq!(entry.draft.discharge_cfs, Some(500.0));
        assert_eq!(entry.definition.source.mode, SourceMode::WaterLevel);
        assert_eq!(entry.draft.at(entry.definition.source.center), source);
    }
    #[test]
    fn river_discharge_mode_requires_a_rate_and_roundtrips() {
        let discharge = SourceParameters {
            mode: SourceMode::Discharge,
            discharge_cfs: Some(150.0),
            ..Default::default()
        };
        let source = discharge.at([10.0, 20.0, 30.0]);
        assert!(source.validate().is_ok());
        assert_eq!(SourceParameters::from_source(source), discharge);
        assert!(LocalSource {
            discharge_cfs: None,
            ..source
        }
        .validate()
        .is_err());
    }
    #[test]
    fn river_discharge_validation_keeps_speed_and_cfs_separate() {
        let p = SourceParameters {
            speed: 10.0,
            discharge_cfs: Some(123.5),
            ..Default::default()
        };
        let s = p.at([10.0, 20.0, 30.0]);
        assert!(s.validate().is_ok());
        assert_eq!(s.rate, 10.0);
        assert_eq!(s.discharge_cfs, Some(123.5));
        assert_eq!(SourceParameters::from_source(s), p);
        for q in [-1.0, f32::NAN, f32::INFINITY, 1_000_001.0] {
            assert!(LocalSource {
                discharge_cfs: Some(q),
                ..s
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn river_live_controls_keep_geometry_and_ignore_stale_replies() {
        let mut editor = Editor::default();
        let source = editor.defaults.at([12.0, 34.0, 56.0]);
        let definition = editor.add(source, Settings::default());
        editor.finish(definition.id, Ok(source));
        let controls = LiveControls {
            mode: SourceMode::WaterLevel,
            discharge_cfs: Some(75.0),
            speed: 4.0,
        };
        let updated = controls.apply(source).unwrap();
        assert_eq!(updated.center, source.center);
        assert_eq!(updated.water_level, source.water_level);
        assert_eq!(updated.source_width, source.source_width);
        assert_eq!(updated.resolution, source.resolution);
        let entry = editor.entry_mut(definition.id).unwrap();
        entry.draft.speed = controls.speed;
        entry.draft.discharge_cfs = controls.discharge_cfs;
        editor.finish_live(
            definition.id,
            LiveControls::from_source(source),
            source,
            None,
        );
        assert_eq!(
            editor.entry_mut(definition.id).unwrap().definition.source,
            source
        );
        editor.finish_live(definition.id, controls, updated, None);
        let entry = editor.entry_mut(definition.id).unwrap();
        assert_eq!(entry.definition.source, updated);
        assert!(!entry.pending, "live controls do not enter rebuild state");
        editor.finish_live(
            definition.id,
            controls,
            updated,
            Some("inlet rejected".into()),
        );
        assert!(editor.entry_mut(definition.id).unwrap().error.is_some());
        editor.clear();
        editor.finish_live(definition.id, controls, updated, None);
        assert!(editor.entries.is_empty());
    }
}
