//! Tooling that builds a whole emote library from a Fortnite-style source
//! export (one folder per item with `Item.json`, `PlaybackMetadata.json`, PSA
//! clips and decoded audio next to it).
//!
//! * `batch` converts every item to a bundle (montages grouped into variants,
//!   sound graphs mixed, retargeted to the chosen model);
//! * `discovery` reads the montage structure for `batch`;
//! * `catalog` derives which items are variations of one another from item
//!   links and asset paths, never from names or ID prefixes;
//! * `presentation_import` bakes props and effects into existing bundles;
//! * `shipping`, `reground` and `reconcile` re-pack, recalibrate and repair an
//!   existing library without touching what did not change;
//! * `loop_audit` measures loop seams.
//!
//! The source export itself is produced by an external exporter that is not part
//! of this repository (see EXPORT-AUDIT.md for the layout the tooling expects).
//! Nothing here is needed to *play* emotes.
#![allow(clippy::too_many_arguments)]

pub mod batch;
pub mod catalog;
pub mod discovery;
pub mod loop_audit;
pub mod presentation_import;
pub mod reconcile;
pub mod reground;
pub mod shipping;

// What the tool modules share (they glob-import this module as `super::*`).
pub(crate) use crate::compiler::{
    compile_recipe, ground_mesh, read, relative, target_from_model, text, DEFAULT_TARGET_MODEL,
};
pub(crate) use crate::{audio_mix, montage, Bundle, MAX_MEMBER_BYTES};
pub(crate) use serde_json::{json, Value};
pub(crate) use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
