# rapier-cloth-core

An engine-independent CPU cloth solver using extended position-based dynamics
(XPBD). It provides triangle meshes, area-based masses, edge-length and dihedral
bending constraints, fixed vertices, external forces and collision-source interfaces.
The only normal dependency is glam.

For Rapier collision queries, attachments and time synchronization, use
[rapier-cloth](https://github.com/neka-nat/rapier-cloth).

## Use from a checkout

```toml
[dependencies]
rapier-cloth-core = { path = "../rapier-cloth/crates/rapier-cloth-core" }
```

Requires Rust 1.90 or newer. Select exactly one precision: `f32` is the default;
for f64, add `default-features = false, features = ["f64"]`. Do not use
`--all-features`.

```rust
use rapier_cloth_core::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mesh = GridBuilder::new(32, 32).origin(Vec3::Y).build()?;
    let mut cloth = Cloth::new(mesh, ClothMaterial::default())?;
    for i in 0..32 {
        cloth.pin(i, cloth.positions()[i as usize])?;
    }
    let mut solver = Solver::new();
    solver.step(
        &mut cloth,
        1.0 / 240.0,
        -Vec3::Y * 9.81,
        &SolverSettings::default(),
    )?;
    assert_eq!(cloth.surface().positions.len(), 1024);
    Ok(())
}
```

This example has gravity and pins but no external collision source. The caller owns
the substep loop and rendering. Mesh topology is fixed; self-collision and tearing
are not implemented. Material compliance describes discrete constraints and requires
tuning with the chosen grid, time step and iteration count.

See the repository's [documentation](https://github.com/neka-nat/rapier-cloth/tree/main/docs)
and [examples](https://github.com/neka-nat/rapier-cloth/tree/main/examples).

Licensed under the [MIT License](LICENSE-MIT).
