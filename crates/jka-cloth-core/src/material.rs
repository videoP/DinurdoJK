use crate::{ClothError, Real};

/// Metres, kilograms, seconds. Compliances parameterize discrete constraints,
/// not calibrated continuum fabric moduli.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClothMaterial {
    pub surface_density: Real,
    pub stretch_compliance: Real,
    pub bend_compliance: Real,
    /// Exponential velocity damping coefficient in inverse seconds.
    pub damping: Real,
    pub friction: Real,
    /// Numerical particle radius; need not equal physical cloth thickness.
    pub contact_radius: Real,
}

impl Default for ClothMaterial {
    fn default() -> Self {
        Self {
            surface_density: 0.2,
            stretch_compliance: 0.0,
            bend_compliance: 1.0e-4,
            damping: 0.1,
            friction: 0.5,
            contact_radius: 0.005,
        }
    }
}

impl ClothMaterial {
    pub fn validate(&self) -> Result<(), ClothError> {
        if !self.surface_density.is_finite() || self.surface_density <= 0.0 {
            return Err(ClothError::InvalidParameter(
                "surface_density must be finite and positive",
            ));
        }
        if !self.contact_radius.is_finite() || self.contact_radius <= 0.0 {
            return Err(ClothError::InvalidParameter(
                "contact_radius must be finite and positive",
            ));
        }
        for (value, name) in [
            (self.stretch_compliance, "stretch_compliance"),
            (self.bend_compliance, "bend_compliance"),
            (self.damping, "damping"),
            (self.friction, "friction"),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(ClothError::InvalidParameter(name));
            }
        }
        Ok(())
    }
}
