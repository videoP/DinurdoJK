use crate::{ClothError, Real, Vec3};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContactKey {
    pub particle: u32,
    pub external: u64,
    pub feature: u32,
}

/// A unilateral plane constraint n dot (x - point) >= particle radius.
/// normal is a unit vector from the obstacle towards the cloth. All vectors
/// are in the application's world frame; friction is already combined.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub key: ContactKey,
    pub normal: Vec3,
    pub point: Vec3,
    pub surface_velocity: Vec3,
    pub friction: Real,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContactStage {
    Stabilization,
    Prediction,
    Iteration,
    Final,
}

/// A narrow boundary for refreshing external constraints. Output is cleared
/// by the solver before every call. Failure aborts the entire cloth substep.
pub trait ContactSource {
    fn contacts(
        &mut self,
        previous: &[Vec3],
        positions: &[Vec3],
        radius: Real,
        stage: ContactStage,
        out: &mut Vec<Contact>,
    ) -> Result<(), ClothError>;
}
#[derive(Default)]
pub struct NoContacts;
impl ContactSource for NoContacts {
    fn contacts(
        &mut self,
        _: &[Vec3],
        _: &[Vec3],
        _: Real,
        _: ContactStage,
        _: &mut Vec<Contact>,
    ) -> Result<(), ClothError> {
        Ok(())
    }
}

/// Coulomb-limited kinetic friction in velocity units. Cannot reverse slip.
pub fn friction_velocity(
    velocity: Vec3,
    surface_velocity: Vec3,
    normal: Vec3,
    normal_delta: Real,
    mu: Real,
) -> Vec3 {
    let relative = velocity - surface_velocity;
    let tangent = relative - normal * relative.dot(normal);
    let speed = tangent.length();
    if speed <= Real::MIN_POSITIVE {
        return velocity;
    }
    velocity - tangent * ((mu * normal_delta.max(0.0)).min(speed) / speed)
}
