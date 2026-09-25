//! Collision tree validation is independent of the optional visibility lump.
use super::{index, int, invalid, range, Result};
use std::ops::Range;

#[derive(Debug)]
pub struct CollisionNode {
    pub plane: usize,
    /// Negative children encode leaves as -(index + 1), exactly as in RBSP.
    pub children: [i32; 2],
}
#[derive(Debug)]
pub struct CollisionLeaf {
    pub cluster: i32,
    pub area: i32,
    pub brushes: Range<usize>,
    pub surfaces: Range<usize>,
}
#[derive(Debug)]
pub struct CollisionTree {
    pub nodes: Vec<CollisionNode>,
    pub leaves: Vec<CollisionLeaf>,
    pub leaf_brushes: Vec<usize>,
    pub leaf_surfaces: Vec<usize>,
}
impl CollisionTree {
    pub(super) fn parse(
        nodes: &[u8],
        leaves: &[u8],
        surfaces: &[u8],
        brushes: &[u8],
        plane_count: usize,
        surface_count: usize,
        brush_count: usize,
    ) -> Result<Option<Self>> {
        if nodes.is_empty() && leaves.is_empty() && brushes.is_empty() && surfaces.is_empty() {
            return Ok(None);
        }
        if nodes.is_empty()
            || leaves.is_empty()
            || !nodes.len().is_multiple_of(36)
            || !leaves.len().is_multiple_of(48)
            || !brushes.len().is_multiple_of(4)
            || !surfaces.len().is_multiple_of(4)
        {
            return Err(invalid("invalid collision tree lump sizes"));
        }
        let leaf_brushes = brushes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| index(int(r, 0), brush_count, "leaf brush"))
            .collect::<Result<Vec<_>>>()?;
        let leaf_surfaces = surfaces
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| index(int(r, 0), surface_count, "leaf surface"))
            .collect::<Result<Vec<_>>>()?;
        let mut parsed_leaves = Vec::new();
        for row in leaves.as_chunks::<48>().0 {
            let cluster = int(row, 0);
            let area = int(row, 4);
            // CM allocates a square area-connectivity matrix.
            if !(-1..=131071).contains(&cluster) || !(-1..=255).contains(&area) {
                return Err(invalid("collision leaf cluster/area out of range"));
            }
            parsed_leaves.push(CollisionLeaf {
                cluster,
                area,
                brushes: range(
                    int(row, 40),
                    int(row, 44),
                    leaf_brushes.len(),
                    "leaf brushes",
                )?,
                surfaces: range(
                    int(row, 32),
                    int(row, 36),
                    leaf_surfaces.len(),
                    "leaf surfaces",
                )?,
            });
        }
        let mut parsed_nodes = Vec::new();
        for row in nodes.as_chunks::<36>().0 {
            let children = [int(row, 4), int(row, 8)];
            for child in children {
                if child >= 0 {
                    index(child, nodes.len() / 36, "collision child node")?;
                } else {
                    index(-(child + 1), parsed_leaves.len(), "collision child leaf")?;
                }
            }
            parsed_nodes.push(CollisionNode {
                plane: index(int(row, 0), plane_count, "collision node plane")?,
                children,
            });
        }
        let mut state = vec![0u8; parsed_nodes.len()];
        let mut depths = vec![0usize; parsed_nodes.len()];
        for root in 0..parsed_nodes.len() {
            let mut stack = vec![(root, false)];
            while let Some((node, finish)) = stack.pop() {
                if finish {
                    state[node] = 2;
                    depths[node] = 1 + parsed_nodes[node]
                        .children
                        .iter()
                        .filter(|&&c| c >= 0)
                        .map(|&c| depths[c as usize])
                        .max()
                        .unwrap_or(0);
                    if depths[node] > 256 {
                        return Err(invalid("collision tree too deep"));
                    }
                } else {
                    if state[node] == 1 {
                        return Err(invalid("cycle in collision tree"));
                    }
                    if state[node] == 2 {
                        continue;
                    }
                    state[node] = 1;
                    stack.push((node, true));
                    for child in parsed_nodes[node].children.into_iter().filter(|&c| c >= 0) {
                        stack.push((child as usize, false));
                    }
                }
            }
        }
        Ok(Some(Self {
            nodes: parsed_nodes,
            leaves: parsed_leaves,
            leaf_brushes,
            leaf_surfaces,
        }))
    }
}
