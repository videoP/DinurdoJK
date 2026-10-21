//! Profile selection uses the source demo's colored-triangle overlay approach.
//! Reuse the rendered CPU vertices so remeshing, animation and jiggle stay exact.
use super::{
    Arc, DynamicModelAlphaMode, DynamicModelSurface, Ghoul2GpuMeshSource, HashMap, PlayerPresenter,
};

#[derive(Default)]
pub(super) struct ProfileSelection {
    group: Option<Arc<str>>,
    /// Triangle labels are independent of animation and the selected group.
    triangles: HashMap<Arc<str>, Arc<Vec<&'static str>>>,
    /// The mesh index stream and anatomical labels are invariant for a mesh
    /// key. Only pose/vertex positions move; do not rebuild the same colored
    /// region's index buffer on every Profile animation frame.
    selected: HashMap<Arc<str>, HashMap<String, Arc<Vec<u32>>>>,
}

impl ProfileSelection {
    fn indices_for(&mut self, key: &Arc<str>, indices: &[u32], labels: &[&str], group: &str) -> Arc<Vec<u32>> {
        if let Some(cached) = self.selected.get(key).and_then(|groups| groups.get(group)) {
            return Arc::clone(cached);
        }
        let selected = Arc::new(selected_indices(indices, labels, group));
        let groups = self.selected.entry(Arc::clone(key)).or_default();
        // Preserve the active and recently selected regions, not every
        // anatomical overlay ever selected on each large Ghoul2 mesh.
        if groups.len() >= 2 {
            groups.clear();
        }
        groups.insert(group.to_owned(), Arc::clone(&selected));
        selected
    }
}

impl PlayerPresenter {
    pub(crate) fn set_profile_body_highlight(&mut self, group: Option<&str>) {
        if self.profile_selection.group.as_deref() != group {
            self.profile_selection.group = group.map(Arc::from);
        }
    }

    pub(super) fn push_profile_selection(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        base_index: usize,
        mesh: &Ghoul2GpuMeshSource,
        promoted: bool,
    ) {
        if !self.preview_cpu_remesh {
            return;
        }
        let Some(group) = self.profile_selection.group.as_ref().map(Arc::clone) else {
            return;
        };
        let Some(pick) = self.profile_body_pick.as_ref() else {
            return;
        };
        let Some(base) = draws.get(base_index) else {
            return;
        };
        let (key, vertices) = if promoted {
            (&mesh.key, &mesh.vertices)
        } else {
            (&mesh.base_key, &mesh.base_vertices)
        };
        let labels = if let Some(labels) = self.profile_selection.triangles.get(key) {
            Arc::clone(labels)
        } else {
            let centroids = base.indices.chunks_exact(3).map(|face| {
                std::array::from_fn(|k| {
                    face.iter()
                        .map(|&i| vertices[i as usize].position[k])
                        .sum::<f32>()
                        / 3.0
                })
            });
            let Ok(labels) = jka_assets::body_shape::anny::regions(
                &pick.model.glm,
                &pick.model.gla,
                pick.model
                    .source_native_pivots
                    .as_deref()
                    .map(|p| p.as_slice()),
                centroids,
            ) else {
                return;
            };
            let labels = Arc::new(labels);
            if self.profile_selection.triangles.len() >= 32 {
                self.profile_selection.triangles.clear();
                self.profile_selection.selected.clear();
            }
            self.profile_selection
                .triangles
                .insert(Arc::clone(key), Arc::clone(&labels));
            labels
        };
        let indices = self.profile_selection.indices_for(key, &base.indices, &labels, &group);
        if indices.is_empty() {
            return;
        }
        // Same red/alpha as Anny's mesh diagnostic overlay, drawn through the
        // existing material overlay path. Normal depth testing hides occluded
        // regions; no shadow or glow contribution leaks into the preview.
        let mut overlay = Self::clone_surface_with_material(
            base,
            None,
            DynamicModelAlphaMode::BlendUnlit,
            [1.0, 0.2, 0.2, 0.5],
        );
        overlay.indices = indices;
        overlay.rt_rigid = None;
        overlay.rt_skinned_key = None;
        overlay.authored_glow = false;
        overlay.lighting_origin = None;
        draws.push(overlay);
    }
}

fn selected_indices(indices: &[u32], labels: &[&str], group: &str) -> Vec<u32> {
    indices
        .chunks_exact(3)
        .zip(labels)
        .filter(|(_, label)| **label == group)
        .flat_map(|(face, _)| face.iter().copied())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::selected_indices;

    #[test]
    fn animated_preview_reuses_selection_indices() {
        let mut cache = super::ProfileSelection::default();
        let key: super::Arc<str> = super::Arc::from("mesh/torso");
        let indices = [0, 1, 2, 3, 4, 5];
        let labels = ["torso", "arms"];
        let first = cache.indices_for(&key, &indices, &labels, "torso");
        let second = cache.indices_for(&key, &indices, &labels, "torso");
        assert!(super::Arc::ptr_eq(&first, &second));
        assert_eq!(first.as_slice(), &[0, 1, 2]);
        let other = cache.indices_for(&key, &indices, &labels, "arms");
        assert_eq!(other.as_slice(), &[3, 4, 5]);
    }

    #[test]
    fn selection_keeps_only_region_triangles_and_preserves_winding() {
        let indices = [4, 8, 2, 1, 5, 3, 6, 7, 0];
        let labels = ["arms", "torso", "arms"];
        assert_eq!(
            selected_indices(&indices, &labels, "arms"),
            [4, 8, 2, 6, 7, 0]
        );
        assert_eq!(selected_indices(&indices, &labels, "torso"), [1, 5, 3]);
        assert!(selected_indices(&indices, &labels, "eyes").is_empty());
    }
}
