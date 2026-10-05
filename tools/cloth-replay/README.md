# Cloth replay

Runs the **production cloth solver and body-proxy helpers** using locally installed GLM/GLA/skin assets. No game window or asset changes are required.

From the repository root:

~~~powershell
cargo run --manifest-path tools/cloth-replay/Cargo.toml --target-dir target/cloth-replay --release --offline -- --assert-stable
~~~

Add --quick for a 3-second sweep at 60 Hz and clearance 0/0.25/1/4.

Run the source's regression tests:

~~~powershell
cargo test --manifest-path tools/cloth-replay/Cargo.toml --target-dir target/cloth-replay --release --offline
~~~

Options:

- --base path: GameData/base directory; default target/release/base.
- --model qpath: GLM; default models/players/jawa_jazzy/model.glm.
- --skin qpath: skin; default models/players/jawa_jazzy/model_jazzy_hood.skin.
- --out path: results; default target/cloth-replay/results.
- --quick: short clearance sweep.
- --assert-stable: nonzero exit on solver errors, runaway resets, rejected native steps, or rendered offsets over 100 JKA units.

The full matrix covers 5 seconds per case, clearance 0/0.1/0.25/0.5/1/2/4, 15/60/240 Hz physics, and 16/2 ms presentation. Scenarios include standing in wind, running/stopping/turning, jumping, and deliberately moving arms into the torso. Bone-angle errors are fatal so that stress case cannot be silently skipped.

Outputs:

- metrics.csv / summary.json: physical secondary offsets, rendered deviation from animation, resets, errors, rejected steps, proxy penetration, and wall time.
- viewer.html: rotate/zoom the worst frame for each case; show authored cloth or collision proxies.
- timeline.html: recorded motion for the 60 Hz/default-clearance cases.
- snapshots.json: inspectable body, cloth, and collider geometry.

Open HTML directly in a browser; no network resources are used. Raw proxy penetration includes overlap present in skeletal animation; new penetration subtracts that authored overlap. Proxies remain approximate. Textures, the game camera, gameplay prediction, and the exact player torso/leg animation split are not reproduced. These limits distinguish solver regression checks from complete in-game validation.
