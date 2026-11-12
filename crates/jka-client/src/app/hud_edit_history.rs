//! Undo/redo for the interactive HUD editor.  Snapshot only on real edits;
//! combine all intermediate frames of a pointer drag/slider gesture into one step.
//! This is editor-only state, never touched by the gameplay or render loop.
use std::collections::BTreeMap;

use crate::extensions::HudPlacement;
use crate::ui::HudLayout;

#[derive(Clone, PartialEq)]
#[cfg_attr(test, derive(Debug))]
pub(in crate::app) struct HudEditState {
    pub layout: HudLayout,
    pub extension_placements: BTreeMap<String, HudPlacement>,
}

#[derive(Default)]
pub(in crate::app) struct HudEditHistory {
    undo: Vec<HudEditState>,
    redo: Vec<HudEditState>,
    pending: Option<HudEditState>,
}

impl HudEditHistory {
    const MAX_STEPS: usize = 64;

    pub fn mark_changed(&mut self, before: HudEditState) {
        if self.pending.is_none() {
            self.pending = Some(before);
        }
    }

    pub fn commit(&mut self, now: &HudEditState) {
        let Some(before) = self.pending.take() else { return; };
        if before == *now { return; }
        Self::push(&mut self.undo, before);
        self.redo.clear();
    }

    pub fn undo(&mut self, now: HudEditState) -> Option<HudEditState> {
        self.commit(&now);
        let previous = self.undo.pop()?;
        Self::push(&mut self.redo, now);
        Some(previous)
    }

    pub fn redo(&mut self, now: HudEditState) -> Option<HudEditState> {
        self.commit(&now);
        let next = self.redo.pop()?;
        Self::push(&mut self.undo, now);
        Some(next)
    }

    fn push(stack: &mut Vec<HudEditState>, state: HudEditState) {
        if stack.len() == Self::MAX_STEPS { stack.remove(0); }
        stack.push(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_frames_become_one_undo_step_and_redo_is_reversible() {
        let a = HudEditState { layout: HudLayout::default(), extension_placements: BTreeMap::new() };
        let mut b = a.clone();
        b.layout.health.offset[0] += 10.0;
        let mut c = b.clone();
        c.extension_placements.insert("hud/force".into(), HudPlacement { x: 3.0, y: -128.0, scale: 1.2, visible: true });
        let mut history = HudEditHistory::default();
        history.mark_changed(a.clone());
        history.mark_changed(b);
        history.commit(&c);
        assert_eq!(history.undo(c.clone()), Some(a.clone()));
        assert_eq!(history.redo(a), Some(c));
    }

    #[test]
    fn no_op_does_not_create_undo_entry() {
        let state = HudEditState { layout: HudLayout::default(), extension_placements: BTreeMap::new() };
        let mut history = HudEditHistory::default();
        history.mark_changed(state.clone());
        history.commit(&state);
        assert!(history.undo(state).is_none());
    }
}
