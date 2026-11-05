//! Coordinates.

pub fn render_position([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, z, -y]
}

pub fn jka_position([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, -z, y]
}
