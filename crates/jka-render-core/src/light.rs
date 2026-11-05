//! Classic (lightgrid) entity lighting sample.

/// Rec. 709 luminance of a linear RGB triple.
pub fn light_luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ClassicEntityLight {
    /// OpenJK light units, nominally 0..255 after interpolation/scaling.
    pub ambient: [f32; 3],
    /// OpenJK light units, nominally 0..255 before the final vertex clamp.
    pub directed: [f32; 3],
    /// Unit incoming-light direction in renderer coordinates [x,z,-y].
    pub direction: [f32; 3],
    /// Part of `directed` that is the baked map sun, same units. Already
    /// included in `directed` until `relight_sun` moves it out.
    pub baked_sun: [f32; 3],
    /// Second directional light: the runtime sun after `relight_sun`. Zero
    /// (and ignored by the shaders) otherwise.
    pub sun_directed: [f32; 3],
    /// Unit incoming-light direction of `sun_directed`, renderer coordinates.
    pub sun_direction: [f32; 3],
    /// 0..1 agreement between the probes blended into `direction` (length of the
    /// blended direction over the total weight). Low means neighbouring probes
    /// point different ways, so the blended direction is unreliable. It cannot
    /// see lights cancelling inside one probe: q3map2 bakes only the normalized
    /// sum, so that disagreement is gone from the data.
    pub direction_coherence: f32,
}

impl ClassicEntityLight {
    /// Moves the baked sun share out of the lightgrid's directed term and
    /// replaces it with a separate directional light: `baked_sun` scaled by the
    /// per-channel `gain` (runtime sun radiance / map sun radiance), arriving
    /// from `toward_sun` (unit, renderer coordinates).
    ///
    /// The remaining directed light keeps its own direction, recovered by
    /// subtracting the map sun's luminance-weighted direction from the probe's
    /// blended one, so torch and skylight shading does not swing toward the
    /// removed sun.
    pub fn relight_sun(&mut self, map_toward_sun: [f32; 3], toward_sun: [f32; 3], gain: [f32; 3]) {
        let sun_luma = light_luminance(self.baked_sun);
        if sun_luma <= 1e-4 {
            return;
        }
        let total_luma = light_luminance(self.directed).max(sun_luma);
        let mut rest_direction = [0.0_f32; 3];
        for axis in 0..3 {
            rest_direction[axis] =
                self.direction[axis] * total_luma - map_toward_sun[axis] * sun_luma;
        }
        let rest_len_sq: f32 = rest_direction.iter().map(|v| v * v).sum();
        // With nothing left over the remainder direction is irrelevant; keep the
        // probe direction so the shader never sees a zero vector for a live term.
        if rest_len_sq > 1e-8 {
            let inv = rest_len_sq.sqrt().recip();
            self.direction = rest_direction.map(|v| v * inv);
        }
        for channel in 0..3 {
            self.directed[channel] = (self.directed[channel] - self.baked_sun[channel]).max(0.0);
            self.sun_directed[channel] = self.baked_sun[channel] * gain[channel];
        }
        self.sun_direction = toward_sun;
    }
}
