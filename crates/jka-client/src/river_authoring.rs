//! Local patch identity, independent materials and source-edit transactions.
use crate::{river_runtime::LocalSource, river_settings::Settings};

pub type Id = u64;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceParameters {
    pub height: f32,
    pub radius: f32,
    pub width: f32,
    pub cells: u32,
    pub speed: f32,
    /// None preserves Unreal's held-water-level source.
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
            discharge_cfs: None,
        }
    }
}
impl SourceParameters {
    pub fn at(self, center: [f32; 3]) -> LocalSource {
        LocalSource {
            center,
            radius: self.radius,
            water_level: center[2] + self.height,
            source_width: self.width,
            resolution: self.cells,
            rate: self.speed,
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
            discharge_cfs: s.discharge_cfs,
        }
    }
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
    next_id: Id,
}
impl Default for Editor {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            selected: None,
            defaults: SourceParameters::default(),
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
}
