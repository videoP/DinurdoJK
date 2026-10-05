# Local rapier-cloth-core patch

Based on rapier-cloth-core 0.1.0, from the MIT-licensed upstream package.
The upstream README and license are retained beside this file.

Change: bound each accepted dihedral XPBD correction by 20% of the shortest
shared-edge/face-altitude scale of that hinge. Update the scalar multiplier by
the same accepted amount. Ordinary small corrections are unchanged.

Reason: replay of the actual jawa_jazzy GLM and humanoid GLA identified the
bending projection proposing 8–10 metre vertex movements on folded/sliver
triangles. Contact-only corrections did not resolve this failure.

The bound depends on geometry and applies to every hinge. It has no character,
surface-name, animation, or jump-specific behavior. StepReport counts limited
bend corrections separately from fallback/rejected simulation steps.

Regression tests cover ordinary steps, thin folds, sign, and geometry scaling.
The tools/cloth-replay diagnostic exercises the shared library and production
body proxies with real assets.
