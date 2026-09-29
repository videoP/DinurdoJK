//! RBSP cluster PVS, following the tree/bitset approach in japro-web's viewer.
//! Area-portal state is not supplied by the map viewer.
use super::{index, int, invalid, range, Plane, Result};

#[derive(Debug, Clone)]
struct Node {
    plane: usize,
    children: [i32; 2],
}
/// CM_PointLeafnum: walk the BSP tree to the leaf containing `point`.
fn leaf_at(nodes: &[Node], planes: &[Plane], point: [f32; 3]) -> Option<usize> {
    if !point.iter().all(|v| v.is_finite()) {
        return None;
    }
    let mut node = 0i32;
    for _ in 0..=nodes.len() {
        if node < 0 {
            return Some(-(node + 1) as usize);
        }
        let record = &nodes[node as usize];
        let plane = planes[record.plane];
        let distance = (0..3).map(|i| point[i] * plane.normal[i]).sum::<f32>() - plane.distance;
        node = record.children[usize::from(distance < 0.0)];
    }
    None
}

/// Point -> BSP portal area lookup for a server-side areamask
/// (CM_PointLeafnum + CM_LeafArea).
#[derive(Debug, Clone)]
pub struct AreaLocator {
    nodes: Vec<Node>,
    planes: Vec<Plane>,
    leaf_areas: Vec<i32>,
    area_count: usize,
}

impl AreaLocator {
    /// Area of the leaf containing `point`; `None` for solid/unassigned leaves.
    pub fn area_at(&self, point: [f32; 3]) -> Option<usize> {
        leaf_at(&self.nodes, &self.planes, point)
            .and_then(|leaf| usize::try_from(*self.leaf_areas.get(leaf)?).ok())
    }

    pub fn area_count(&self) -> usize {
        self.area_count
    }
}

#[derive(Debug, Clone)]
pub struct Visibility {
    nodes: Vec<Node>,
    planes: Vec<Plane>,
    leaves: Vec<i32>,
    leaf_areas: Vec<i32>,
    pub surface_clusters: Vec<Vec<usize>>,
    /// BSP portal-area membership for each surface. Bit N means the surface is
    /// referenced by at least one non-solid leaf in area N. JKA RBSP limits
    /// area ids to 0..255, matching the 32-byte snapshot areamask.
    pub surface_area_masks: Vec<[u64; 4]>,
    pub clusters: usize,
    stride: usize,
    bits: Vec<u8>,
}
impl Visibility {
    pub(super) fn parse(
        nodes: &[u8],
        leaves: &[u8],
        faces: &[u8],
        vis: &[u8],
        planes: &[Plane],
        surfaces: usize,
    ) -> Result<Option<Self>> {
        if vis.is_empty() {
            return Ok(None);
        }
        if vis.len() < 8
            || !nodes.len().is_multiple_of(36)
            || !leaves.len().is_multiple_of(48)
            || !faces.len().is_multiple_of(4)
        {
            return Err(invalid("invalid visibility lump sizes"));
        }
        let clusters =
            usize::try_from(int(vis, 0)).map_err(|_| invalid("negative PVS cluster count"))?;
        let stride = usize::try_from(int(vis, 4)).map_err(|_| invalid("negative PVS stride"))?;
        if clusters == 0
            || clusters > 131072
            || stride < clusters.div_ceil(8)
            || clusters.checked_mul(stride) != Some(vis.len() - 8)
        {
            return Err(invalid("invalid PVS dimensions"));
        }
        let face_indices: Vec<_> = faces
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| index(int(r, 0), surfaces, "leaf surface"))
            .collect::<Result<_>>()?;
        let mut surface_clusters = vec![Vec::new(); surfaces];
        let mut surface_area_masks = vec![[0_u64; 4]; surfaces];
        let mut leaf_clusters = Vec::new();
        let mut leaf_areas = Vec::new();
        for row in leaves.as_chunks::<48>().0 {
            let cluster = int(row, 0);
            let area = int(row, 4);
            if cluster < -1 || cluster >= clusters as i32 {
                return Err(invalid("leaf cluster outside PVS"));
            }
            if !(-1..=255).contains(&area) {
                return Err(invalid("leaf area outside snapshot areamask"));
            }
            let span = range(
                int(row, 32),
                int(row, 36),
                face_indices.len(),
                "leaf surfaces",
            )?;
            if cluster >= 0 {
                for &face in &face_indices[span] {
                    surface_clusters[face].push(cluster as usize);
                    if area >= 0 {
                        let area = area as usize;
                        surface_area_masks[face][area / 64] |= 1_u64 << (area % 64);
                    }
                }
            }
            leaf_clusters.push(cluster);
            leaf_areas.push(area);
        }
        for clusters in &mut surface_clusters {
            clusters.sort_unstable();
            clusters.dedup();
        }
        let node_count = nodes.len() / 36;
        let mut parsed = Vec::new();
        for row in nodes.as_chunks::<36>().0 {
            let children = [int(row, 4), int(row, 8)];
            for child in children {
                if child >= 0 {
                    index(child, node_count, "BSP child node")?;
                } else {
                    index(-(child + 1), leaf_clusters.len(), "BSP child leaf")?;
                }
            }
            parsed.push(Node {
                plane: index(int(row, 0), planes.len(), "BSP node plane")?,
                children,
            });
        }
        if parsed.is_empty() || leaf_clusters.is_empty() {
            return Err(invalid("PVS without BSP tree"));
        }
        // Reject cycles without recursive stack growth on hostile maps.
        let mut state = vec![0u8; parsed.len()];
        for root in 0..parsed.len() {
            if state[root] != 0 {
                continue;
            }
            let mut stack = vec![(root, false)];
            while let Some((node, finish)) = stack.pop() {
                if finish {
                    state[node] = 2;
                    continue;
                }
                if state[node] == 1 {
                    return Err(invalid("cycle in BSP visibility tree"));
                }
                if state[node] == 2 {
                    continue;
                }
                state[node] = 1;
                stack.push((node, true));
                for child in parsed[node].children.into_iter().rev().filter(|&v| v >= 0) {
                    stack.push((child as usize, false));
                }
            }
        }
        Ok(Some(Self {
            nodes: parsed,
            planes: planes.to_vec(),
            leaves: leaf_clusters,
            leaf_areas,
            surface_clusters,
            surface_area_masks,
            clusters,
            stride,
            bits: vis[8..].to_vec(),
        }))
    }
    fn leaf_at(&self, point: [f32; 3]) -> Option<usize> {
        leaf_at(&self.nodes, &self.planes, point)
    }

    /// The point -> area half of this data, without the cluster bitset.
    pub fn area_locator(&self) -> AreaLocator {
        AreaLocator {
            nodes: self.nodes.clone(),
            planes: self.planes.clone(),
            area_count: self
                .leaf_areas
                .iter()
                .filter_map(|&area| usize::try_from(area).ok())
                .max()
                .map_or(0, |area| area + 1),
            leaf_areas: self.leaf_areas.clone(),
        }
    }

    pub fn cluster_at(&self, point: [f32; 3]) -> Option<usize> {
        self.leaf_at(point)
            .and_then(|leaf| usize::try_from(self.leaves[leaf]).ok())
    }

    /// BSP portal area containing `point`, when the leaf is assigned to one.
    pub fn area_at(&self, point: [f32; 3]) -> Option<usize> {
        self.leaf_at(point)
            .and_then(|leaf| usize::try_from(self.leaf_areas[leaf]).ok())
    }
    /// Missing/invalid source or unassigned surfaces conservatively remain visible.
    pub fn visible(&self, from: Option<usize>, targets: &[usize]) -> bool {
        let Some(from) = from.filter(|&v| v < self.clusters) else {
            return true;
        };
        targets.is_empty()
            || targets.iter().any(|&to| {
                to == from
                    || to >= self.clusters
                    || self.bits[from * self.stride + to / 8] & (1 << (to % 8)) != 0
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bytes(values: &[i32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn fixture(child: i32) -> Result<Option<Visibility>> {
        let nodes = bytes(&[0, child, -2, 0, 0, 0, 0, 0, 0]);
        let leaves = bytes(&[
            0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 2, 0, 0,
        ]);
        let mut vis = bytes(&[2, 1]);
        vis.extend([1, 2]);
        Visibility::parse(
            &nodes,
            &leaves,
            &bytes(&[0, 0, 1]),
            &vis,
            &[Plane {
                normal: [1.0, 0.0, 0.0],
                distance: 0.0,
            }],
            2,
        )
    }
    #[test]
    fn tree_walk_multicluster_and_fallback() {
        let vis = fixture(-1).unwrap().unwrap();
        assert_eq!(vis.cluster_at([1.0, 0.0, 0.0]), Some(0));
        assert_eq!(vis.cluster_at([-1.0, 0.0, 0.0]), Some(1));
        assert_eq!(vis.surface_clusters[0], [0, 1]);
        assert_eq!(vis.surface_area_masks[0][0] & 1, 1);
        assert!(vis.visible(Some(0), &vis.surface_clusters[0]));
        assert!(!vis.visible(Some(0), &vis.surface_clusters[1]));
        assert!(vis.visible(None, &[1]));
        assert!(vis.visible(Some(0), &[]));
    }
    #[test]
    fn rejects_cycles_bad_children_and_truncated_bits() {
        assert!(fixture(0).is_err());
        assert!(fixture(-3).is_err());
        assert!(Visibility::parse(&[], &[], &[], &bytes(&[2, 1]), &[], 0).is_err());
    }
}
