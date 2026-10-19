//! The renderer core's floating-origin re-lay contract (`naval_render::relay`), which the published reference omits.
//! JKA keeps one world origin, so hosts pass [`Identity`]; the trait exists so upstream `relay` methods compile.
pub trait SceneRelay {
    fn point(&self, p: [f64; 3]) -> [f64; 3];
    fn vector(&self, at: [f64; 3], v: [f64; 3]) -> [f64; 3];
    fn heading(&self, at: [f64; 3], heading: f64) -> f64;
    fn point2(&self, p: [f64; 2]) -> [f64; 2] {
        let q = self.point([p[0], 0.0, p[1]]);
        [q[0], q[2]]
    }
    fn direction(&self, at: [f64; 3], d: [f64; 2]) -> [f64; 2] {
        let v = self.vector(at, [d[0], 0.0, d[1]]);
        [v[0], v[2]]
    }
}
pub struct Identity;
impl SceneRelay for Identity {
    fn point(&self, p: [f64; 3]) -> [f64; 3] { p }
    fn vector(&self, _: [f64; 3], v: [f64; 3]) -> [f64; 3] { v }
    fn heading(&self, _: [f64; 3], heading: f64) -> f64 { heading }
}
