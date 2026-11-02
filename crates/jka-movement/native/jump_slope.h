/* Included after bg_local.h; helpers are private to each translation unit. */
static void JKA_ForceJumpSlopeCollision(const trace_t *contact, vec3_t endVelocity) {
    /* For an overhead plane, restoring the target Z velocity then clipping
     * contracts horizontal velocity along the normal by a=nz^2 each update.
     * Compose that affine map over elapsed time: sum(a^i)=(1-a^N)/(1-a).
     * N=dt/3ms preserves the established high-rate boost. Only the restored
     * Force Jump contribution is composed; gravity and collision projection
     * still run normally. No impulse is added without an actual contact. */
    float a = contact->plane.normal[2] * contact->plane.normal[2];
    float count = pml.msec / 3.0f;
    float scale;
    float tangentZ = -(pm->ps->velocity[0] * contact->plane.normal[0] +
                        pm->ps->velocity[1] * contact->plane.normal[1]) / contact->plane.normal[2];
    vec3_t before;
    if (!pml.forceJumpSlopeContactKnown) {
        /* A pre-restoration velocity already tangent to this ceiling means
         * continuing contact. On first impact only compose the part of this
         * command spent at the surface. Reuse it for step-slide's retry. */
        pml.forceJumpSlopeContactFraction = DotProduct(pml.previous_velocity, contact->plane.normal) < -5.0f ? contact->fraction : 0.0f;
        pml.forceJumpSlopeContactKnown = qtrue;
    }
    if (count > 1.0f) count = 1.0f + (count - 1.0f) * (1.0f - pml.forceJumpSlopeContactFraction);
    scale = (1.0f - powf(a, count)) / (1.0f - a);
    VectorCopy(pm->ps->velocity, before);
    pm->ps->velocity[2] += (scale - 1.0f) * (pml.forceJumpSlopeTarget - tangentZ);
    endVelocity[2] += (scale - 1.0f) * (pml.forceJumpSlopeTarget - tangentZ);
    JKA_JUMP_TRACE(5, before, pm->ps->velocity, contact->plane.normal,
                   contact->fraction, contact->entityNum, -1, 0);
}
