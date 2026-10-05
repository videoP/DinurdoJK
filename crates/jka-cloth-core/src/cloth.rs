use crate::{ClothError, ClothMaterial, ClothMesh, Real, SurfaceView, Vec3};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// A cloth's physical state; rest topology is shared when making checkpoints.
#[derive(Clone, Debug)]
pub struct Cloth {
    pub(crate) mesh: Arc<ClothMesh>,
    pub(crate) material: ClothMaterial,
    pub(crate) positions: Vec<Vec3>,
    pub(crate) previous: Vec<Vec3>,
    pub(crate) velocities: Vec<Vec3>,
    pub(crate) masses: Vec<Real>,
    pub(crate) inverse_masses: Vec<Real>,
    pub(crate) forces: Vec<Vec3>,
    pub(crate) pins: BTreeMap<u32, Vec3>,
}

impl Cloth {
    pub fn new(mesh: ClothMesh, material: ClothMaterial) -> Result<Self, ClothError> {
        material.validate()?;
        let positions = mesh.rest_positions().to_vec();
        let n = positions.len();
        let masses: Vec<_> = mesh
            .vertex_areas()
            .iter()
            .map(|a| a * material.surface_density)
            .collect();
        if masses
            .iter()
            .any(|m| !m.is_finite() || *m <= 0.0 || !(1.0 / m).is_finite())
        {
            return Err(ClothError::InvalidParameter(
                "particle masses are not representable",
            ));
        }
        Ok(Self {
            mesh: Arc::new(mesh),
            material,
            previous: positions.clone(),
            positions,
            velocities: vec![Vec3::ZERO; n],
            inverse_masses: masses.iter().map(|m| 1.0 / m).collect(),
            masses,
            forces: vec![Vec3::ZERO; n],
            pins: BTreeMap::new(),
        })
    }
    pub fn mesh(&self) -> &ClothMesh {
        &self.mesh
    }
    pub fn material(&self) -> ClothMaterial {
        self.material
    }
    pub fn positions(&self) -> &[Vec3] {
        &self.positions
    }
    pub fn previous_positions(&self) -> &[Vec3] {
        &self.previous
    }
    pub fn velocities(&self) -> &[Vec3] {
        &self.velocities
    }
    pub fn masses(&self) -> &[Real] {
        &self.masses
    }
    pub fn pins(&self) -> &BTreeMap<u32, Vec3> {
        &self.pins
    }
    pub fn surface(&self) -> SurfaceView<'_> {
        SurfaceView {
            positions: &self.positions,
            triangles: self.mesh.triangles(),
        }
    }
    fn check_particle(&self, i: u32) -> Result<usize, ClothError> {
        if (i as usize) >= self.positions.len() {
            Err(ClothError::InvalidParticle(i))
        } else {
            Ok(i as usize)
        }
    }
    /// Change a world pin without discarding the physical mass.
    pub fn pin(&mut self, i: u32, target: Vec3) -> Result<(), ClothError> {
        self.check_particle(i)?;
        if !target.is_finite() {
            return Err(ClothError::InvalidParameter("pin position"));
        }
        self.pins.insert(i, target);
        Ok(())
    }
    pub fn unpin(&mut self, i: u32) -> Result<(), ClothError> {
        self.check_particle(i)?;
        self.pins.remove(&i);
        Ok(())
    }
    pub fn set_velocity(&mut self, i: u32, v: Vec3) -> Result<(), ClothError> {
        let i = self.check_particle(i)?;
        if !v.is_finite() {
            return Err(ClothError::InvalidParameter("velocity"));
        }
        self.velocities[i] = v;
        Ok(())
    }
    /// Persistent external force, in newtons; set to zero to clear it.
    pub fn set_force(&mut self, i: u32, force: Vec3) -> Result<(), ClothError> {
        let i = self.check_particle(i)?;
        if !force.is_finite() {
            return Err(ClothError::InvalidParameter("force"));
        }
        self.forces[i] = force;
        Ok(())
    }
    /// Explicit teleport; topology/rest lengths are retained and velocities reset.
    pub fn set_positions(&mut self, p: &[Vec3]) -> Result<(), ClothError> {
        if p.len() != self.positions.len() || p.iter().any(|p| !p.is_finite()) {
            return Err(ClothError::InvalidParameter("positions"));
        }
        self.positions.copy_from_slice(p);
        self.previous.copy_from_slice(p);
        self.velocities.fill(Vec3::ZERO);
        Ok(())
    }
}

/// Includes arena identity as well as a generation, so handles from another
/// ClothSet cannot accidentally resolve even when indices coincide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ClothHandle {
    set: u64,
    index: u32,
    generation: u32,
}
impl ClothHandle {
    pub fn index(self) -> u32 {
        self.index
    }
    pub fn generation(self) -> u32 {
        self.generation
    }
}
#[derive(Clone, Debug)]
struct Slot {
    generation: u32,
    value: Option<Cloth>,
}
#[derive(Clone, Debug)]
pub struct ClothSet {
    identity: u64,
    slots: Vec<Slot>,
}
static NEXT_SET: AtomicU64 = AtomicU64::new(1);
impl Default for ClothSet {
    fn default() -> Self {
        Self::new()
    }
}
impl ClothSet {
    pub fn new() -> Self {
        let identity = NEXT_SET
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |x| x.checked_add(1))
            .expect("cloth arena identity exhausted");
        Self {
            identity,
            slots: Vec::new(),
        }
    }
    pub fn insert(&mut self, cloth: Cloth) -> ClothHandle {
        let i = self
            .slots
            .iter()
            .position(|s| s.value.is_none() && s.generation < u32::MAX)
            .unwrap_or(self.slots.len());
        if i == self.slots.len() {
            assert!(i < u32::MAX as usize, "cloth arena full");
            self.slots.push(Slot {
                generation: 0,
                value: None,
            });
        }
        self.slots[i].value = Some(cloth);
        ClothHandle {
            set: self.identity,
            index: i as u32,
            generation: self.slots[i].generation,
        }
    }
    pub fn get(&self, h: ClothHandle) -> Result<&Cloth, ClothError> {
        if h.set != self.identity {
            return Err(ClothError::InvalidHandle);
        }
        self.slots
            .get(h.index as usize)
            .filter(|s| s.generation == h.generation)
            .and_then(|s| s.value.as_ref())
            .ok_or(ClothError::InvalidHandle)
    }
    pub fn get_mut(&mut self, h: ClothHandle) -> Result<&mut Cloth, ClothError> {
        if h.set != self.identity {
            return Err(ClothError::InvalidHandle);
        }
        self.slots
            .get_mut(h.index as usize)
            .filter(|s| s.generation == h.generation)
            .and_then(|s| s.value.as_mut())
            .ok_or(ClothError::InvalidHandle)
    }
    pub fn remove(&mut self, h: ClothHandle) -> Result<Cloth, ClothError> {
        self.get(h)?;
        let s = &mut self.slots[h.index as usize];
        let c = s.value.take().unwrap();
        s.generation = s.generation.saturating_add(1);
        Ok(c)
    }
    pub fn iter(&self) -> impl Iterator<Item = (ClothHandle, &Cloth)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.value.as_ref().map(|c| {
                (
                    ClothHandle {
                        set: self.identity,
                        index: i as u32,
                        generation: s.generation,
                    },
                    c,
                )
            })
        })
    }
    pub fn len(&self) -> usize {
        self.iter().count()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
