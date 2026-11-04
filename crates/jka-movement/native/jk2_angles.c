/*
 * JK2 multiplayer player-angle presentation bridge.
 *
 * Port of Raven's JK2 CODE-mp/cgame/cg_players.c CG_G2PlayerAngles and
 * CG_G2ClientSpineAngles, with their 1.02/1.04 branches kept explicit.
 * Original sources:
 *  jk2-1.02-src/CODE-mp/cgame/cg_players.c:2075,2195
 *  jk2-1.04-src/CODE-mp/cgame/cg_players.c:2311,2413
 *
 * Neither JK2 cgame function is the MVSDK bg_misc.c BG_G2PlayerAngles
 * (which uses a different movement-offset leg-yaw algorithm), nor the
 * JKA/OpenJK BG_G2PlayerAngles. Only Ghoul2 API calls are replaced with
 * bone commands; no JKA renderer or pmove state is used here.
 */
#include "jk2_host_internal.h"

/* Names/flags match the shared PlayerAngleResult Ghoul2 command ABI. */
static void jk2_angle_bone(jka_player_angle_result *out, const char *name, const vec3_t a) {
    jka_bone_angle_command *cmd;
    if (out->command_count >= JKA_MAX_BONE_ANGLE_COMMANDS) return;
    cmd = &out->commands[out->command_count++];
    strncpy(cmd->bone_name, name, JKA_BONE_NAME_CAP - 1);
    VectorCopy(a, cmd->angles);
    cmd->flags = 2; /* BONE_ANGLES_POSTMULT */
    cmd->up = 1; cmd->right = 6; cmd->forward = 5; /* POSITIVE_X, NEGATIVE_Y, NEGATIVE_Z */
}

/* CG_SwingAngles, using the stock frame-time driven swing/angle clamp. */
static void jk2_cg_swing(float destination, float tolerance, float clamp, float speed,
                         float *angle, int32_t *swinging, int frametime) {
    float delta, move, scale;
    if (!*swinging) {
        delta = AngleSubtract(*angle, destination);
        if (delta > tolerance || delta < -tolerance) *swinging = 1;
    }
    if (!*swinging) return;
    delta = AngleSubtract(destination, *angle);
    scale = fabsf(delta);
    if (scale < tolerance * 0.5f) scale = 0.5f;
    else if (scale < tolerance) scale = 1.0f;
    else scale = 2.0f;
    move = frametime * scale * speed;
    if (delta >= 0) {
        if (move >= delta) { move = delta; *swinging = 0; }
        *angle = AngleMod(*angle + move);
    } else {
        move = -move;
        if (move <= delta) { move = delta; *swinging = 0; }
        *angle = AngleMod(*angle + move);
    }
    delta = AngleSubtract(destination, *angle);
    if (delta > clamp) *angle = AngleMod(destination - (clamp - 1));
    else if (delta < -clamp) *angle = AngleMod(destination + (clamp - 1));
}

/* JK2 cgame's CG_InRoll/CG_InRollAnim exclusions are animation/timer
 * checks. The portable presentation entity exposes the animation IDs but
 * not the cgame lerp-frame timer, so conservatively exclude active rolls. */
static int jk2_angle_roll(int anim) {
    switch (anim & ~ANIM_TOGGLEBIT) {
    case BOTH_ROLL_F: case BOTH_ROLL_B: case BOTH_ROLL_L: case BOTH_ROLL_R:
        return 1;
    default:
        return 0;
    }
}

/* Original JK2 1.04 CG_InKnockDown (cg_players.c:2282). */
static int jk2_angle_knockdown(int anim) {
    switch (anim & ~ANIM_TOGGLEBIT) {
    case BOTH_KNOCKDOWN1: case BOTH_KNOCKDOWN2: case BOTH_KNOCKDOWN3:
    case BOTH_KNOCKDOWN4: case BOTH_KNOCKDOWN5:
    case BOTH_GETUP1: case BOTH_GETUP2: case BOTH_GETUP3:
    case BOTH_GETUP4: case BOTH_GETUP5:
    case BOTH_FORCE_GETUP_F1: case BOTH_FORCE_GETUP_F2:
    case BOTH_FORCE_GETUP_B1: case BOTH_FORCE_GETUP_B2:
    case BOTH_FORCE_GETUP_B3: case BOTH_FORCE_GETUP_B4:
    case BOTH_FORCE_GETUP_B5:
        return 1;
    default:
        return 0;
    }
}

/* CG_G2ClientSpineAngles: matching both original client versions. The
 * animations are the current entityState and *previously installed* ci IDs. */
static void jk2_cg_spine(int version, const jka_player_angle_entity *es,
                         int ci_legs, int ci_torso, const float *lerp_angles,
                         const float *motion_matrix, vec3_t view,
                         const vec3_t root, vec3_t thoracic, vec3_t upper, vec3_t lower) {
    const int la = es->legs_anim & ~ANIM_TOGGLEBIT;
    const int ta = es->torso_anim & ~ANIM_TOGGLEBIT;
    const int cl = ci_legs & ~ANIM_TOGGLEBIT;
    const int ct = ci_torso & ~ANIM_TOGGLEBIT;
    int eligible;
    int i;

    view[YAW] = AngleDelta(lerp_angles[YAW], root[YAW]);
    eligible = !BG_FlippingAnim(la) && !BG_SpinningSaberAnim(la)
        && !BG_SpinningSaberAnim(ta) && !BG_InSpecialJump(la)
        && !BG_InSpecialJump(ta) && !BG_InDeathAnim(la)
        && !BG_InDeathAnim(ta) && !BG_SaberInSpecial(es->saber_move)
        && !BG_SaberInSpecialAttack(ta) && !BG_SaberInSpecialAttack(la)
        && !BG_FlippingAnim(cl) && !BG_SpinningSaberAnim(cl)
        && !BG_SpinningSaberAnim(ct) && !BG_InSpecialJump(cl)
        && !BG_InSpecialJump(ct) && !BG_InDeathAnim(cl)
        && !BG_InDeathAnim(ct) && !BG_SaberInSpecialAttack(ct)
        && !BG_SaberInSpecialAttack(cl) && !(es->e_flags & EF_DEAD)
        && !jk2_angle_roll(la);
    /* 1.04 added knockdown exclusion and the split legs/torso requirement;
     * 1.02 evaluates motion correction even in a shared full-body anim. */
    if (version == 4) {
        eligible = eligible && la != ta && cl != ct;
        eligible = eligible && !jk2_angle_knockdown(la)
            && !jk2_angle_knockdown(ta)
            && !jk2_angle_knockdown(cl)
            && !jk2_angle_knockdown(ct);
    }
    if (eligible && motion_matrix) {
        vec3_t forward, motion_angles;
        /* mdxaBone_t is a row-major 3x4 matrix, exactly the existing host
         * flatten_matrix3x4 contract, which equals 1.04's
         * GetBoltMatrix_NoRecNoRot. 1.02 calls GetBoltMatrix_NoReconstruct,
         * which post-multiplies the bolt by Create_Matrix({0,270,0})
         * (jk2mv G2_API.cpp G2API_GetBoltMatrix); its NEGATIVE_Y is the
         * NEGATIVE_Y of that product. */
        if (version == 2) {
            static const vec3_t rot_angles = {0.0f, 270.0f, 0.0f};
            vec3_t rot[3];
            AnglesToAxis(rot_angles, rot);
            /* Create_Matrix column 1 is axis[1]. */
            for (i = 0; i < 3; ++i)
                forward[i] = -(motion_matrix[i * 4] * rot[1][0]
                    + motion_matrix[i * 4 + 1] * rot[1][1]
                    + motion_matrix[i * 4 + 2] * rot[1][2]);
        } else {
            for (i = 0; i < 3; ++i) forward[i] = -motion_matrix[i * 4 + 1];
        }
        vectoangles(forward, motion_angles);
        if (version == 4) {
            vec3_t right, temp;
            for (i = 0; i < 3; ++i) right[i] = -motion_matrix[i * 4];
            vectoangles(right, temp);
            motion_angles[ROLL] = -temp[PITCH];
        }
        for (i = 0; i < 3; ++i)
            view[i] = AngleNormalize180(view[i] - AngleNormalize180(motion_angles[i]));
        if (version == 2) {
            /* 1.02 cancels the quarter-turn GetBoltMatrix added above;
             * 1.04's NoRecNoRot call needs no correction. */
            if (view[YAW] < -90.0f) view[YAW] += 360.0f;
            view[YAW] -= 90.0f;
        }
    }

    if (version == 2) {
        /* 1.02 cgame: zero spine pitch, yaw coupled to view pitch. */
        thoracic[PITCH] = lower[PITCH] = upper[PITCH] = 0.0f;
        thoracic[YAW] = view[YAW]*0.20f - view[PITCH]*(view[YAW]*0.020f);
        upper[YAW] = view[YAW]*0.25f - view[PITCH]*(view[YAW]*0.0005f);
        lower[YAW] = view[YAW]*0.25f - view[PITCH]*(view[YAW]*0.0005f);
        if (thoracic[YAW] > 20.0f) thoracic[YAW] = 20.0f;
        if (upper[YAW] > 20.0f) upper[YAW] = 20.0f;
        if (lower[YAW] > 20.0f) lower[YAW] = 20.0f;
        thoracic[ROLL] = view[ROLL]*0.20f;
        upper[ROLL] = view[ROLL]*0.35f;
        lower[ROLL] = view[ROLL]*0.45f;
        for (i = 0; i < 3; ++i) if (upper[i] < 0.0f) upper[i] += 360.0f;
    } else {
        /* 1.04 cgame: distributed pitch/yaw/roll, no 1.02 cross-coupling. */
        thoracic[PITCH] = view[PITCH]*0.20f;
        upper[PITCH] = lower[PITCH] = view[PITCH]*0.40f;
        thoracic[YAW] = view[YAW]*0.20f;
        upper[YAW] = view[YAW]*0.35f;
        lower[YAW] = view[YAW]*0.45f;
        thoracic[ROLL] = view[ROLL]*0.20f;
        upper[ROLL] = view[ROLL]*0.35f;
        lower[ROLL] = view[ROLL]*0.45f;
    }
}

int jk2_cg_g2_player_angles(int version, const jka_player_angle_entity *es, int time,
                            const float *lerp_origin, const float *lerp_angles,
                            int frametime, int ci_legs, int ci_torso,
                            const float *motion_matrix, jka_player_angle_state *state,
                            jka_player_angle_result *out) {
    static const int movement_offsets[8] = {0, 22, 45, -22, 0, 22, -45, -22};
    vec3_t head, torso = {0,0,0}, legs = {0,0,0}, velocity;
    vec3_t angles, view, upper = {0,0,0}, lower = {0,0,0}, thoracic = {0,0,0};
    vec3_t vel_ang, axis[3], cervical = {0,0,0};
    float speed, desired_pitch;
    int direction;

    (void)lerp_origin; /* only the velocity difference survives CG's subtract */
    (void)time;
    if ((version != 2 && version != 4) || !es || !lerp_angles || !state || !out) return 0;
    memset(out, 0, sizeof(*out));
    /* Source predicates switch on jk2gameplay in MVSDK; both versions must
     * be selected under the shared NativeGuard, as JK2 prediction already is. */
    if (!jk2_select_versions(version, version)) return 0;
    VectorCopy(lerp_angles, head);
    head[YAW] = AngleMod(head[YAW]);
    direction = es->e_flags & EF_DEAD ? 0 : es->movement_dir;
    if (direction < 0 || direction > 7) return 0;
    if ((es->legs_anim & ~ANIM_TOGGLEBIT) != BOTH_STAND1
        || (es->torso_anim & ~ANIM_TOGGLEBIT) != WeaponReadyAnim[es->weapon >= 0 && es->weapon < WP_NUM_WEAPONS ? es->weapon : 0]) {
        state->torso_yawing = state->torso_pitching = state->legs_yawing = 1;
    }
    torso[YAW] = head[YAW] + 0.25f * movement_offsets[direction];
    state->torso_yaw_angle = torso[YAW];

    VectorCopy(es->velocity, velocity);
    speed = VectorNormalize(velocity);
    if (speed == 0.0f) torso[YAW] = head[YAW];
    desired_pitch = head[PITCH] > 180.0f ? (head[PITCH] - 360.0f)*0.75f : head[PITCH]*0.75f;
    jk2_cg_swing(desired_pitch, 15.0f, 30.0f, 0.1f, &state->torso_pitch_angle,
                 &state->torso_pitching, frametime);
    torso[PITCH] = state->torso_pitch_angle;

    if (speed != 0.0f) {
        float side;
        speed *= 0.05f;
        AnglesToAxis(legs, axis);
        side = speed * DotProduct(velocity, axis[1]);
        legs[ROLL] -= side;
        side = speed * DotProduct(velocity, axis[0]);
        legs[PITCH] += side;
    }
    legs[YAW] = head[YAW];
    /* Original: velPos = lerpOrigin + (velocity.x, velocity.y, 0);
     * velAng = lerpOrigin - velPos. Aerial players skip this correction. */
    vel_ang[0] = -velocity[0]; vel_ang[1] = -velocity[1]; vel_ang[2] = 0.0f;
    if (es->ground_entity_num != ENTITYNUM_NONE
        && (vel_ang[0] != 0.0f || vel_ang[1] != 0.0f)) {
        float negative, positive, diff;
        int add_direction;
        vectoangles(vel_ang, vel_ang);
        if (vel_ang[YAW] <= legs[YAW]) {
            negative = legs[YAW] - vel_ang[YAW];
            positive = (360.0f - legs[YAW]) + vel_ang[YAW];
        } else {
            negative = legs[YAW] + (360.0f - vel_ang[YAW]);
            positive = vel_ang[YAW] - legs[YAW];
        }
        add_direction = negative >= positive;
        diff = add_direction ? positive : negative;
        if (diff > 90.0f) diff = 180.0f - diff;
        if (diff > 60.0f) diff = 60.0f;
        if (direction == 3 || direction == 5) diff = -diff;
        legs[YAW] += add_direction ? -diff : diff;
    }
    /* CG_SwingAngles(legs[YAW],40,90,0.3) with stock cg_swingAngles enabled. */
    jk2_cg_swing(legs[YAW], 40.0f, 90.0f, 0.3f,
                 &state->legs_yaw_angle, &state->legs_yawing, frametime);
    legs[YAW] = state->legs_yaw_angle;
    legs[ROLL] = torso[ROLL] = 0.0f;
    VectorCopy(legs, out->tur_angles);
    /* CG_G2PlayerAngles: subtract head/torso hierarchy. */
    AnglesSubtract(head, torso, head);
    AnglesSubtract(torso, legs, torso);
    /* 1.04 clears root pitch after saving turAngles; 1.02 retains it. */
    if (version == 4) legs[PITCH] = 0.0f;
    AnglesToAxis(legs, axis);
    memcpy(out->legs_axis, axis, sizeof(out->legs_axis));
    VectorCopy(legs, out->legs_angles);

    VectorCopy(lerp_angles, view);
    view[YAW] = view[ROLL] = 0.0f;
    view[PITCH] *= 0.5f;
    if (version == 2) {
        VectorCopy(lerp_angles, angles);
        angles[PITCH] = 0.0f;
    } else {
        VectorSet(angles, 0.0f, legs[YAW], 0.0f);
        angles[PITCH] = legs[PITCH];
        if (angles[PITCH] > 30.0f) angles[PITCH] = 30.0f;
        else if (angles[PITCH] < -30.0f) angles[PITCH] = -30.0f;
    }
    jk2_cg_spine(version, es, ci_legs, ci_torso, lerp_angles,
                 motion_matrix, view, angles, thoracic, upper, lower);
    if (version == 2) {
        /* 1.02 adds the torso's residual angles after motion compensation. */
        upper[YAW] += torso[YAW]*0.3f;
        lower[YAW] += torso[YAW]*0.3f;
        thoracic[YAW] += torso[YAW]*0.4f;
        upper[PITCH] = torso[PITCH]*0.3f;
        lower[PITCH] = torso[PITCH]*0.3f;
        thoracic[PITCH] = torso[PITCH]*0.4f;
        upper[ROLL] += torso[ROLL]*0.3f;
        lower[ROLL] += torso[ROLL]*0.3f;
        thoracic[ROLL] += torso[ROLL]*0.4f;
    }
    /* Both CG variants zero the spine when riding an emplaced gun. Other
     * special emplaced bone/cervical angles require the gun entity's origin. */
    if (es->other_entity_num2 && !(es->e_flags & EF_DEAD)) {
        VectorClear(upper); VectorClear(lower); VectorClear(thoracic);
    }
    jk2_angle_bone(out, "upper_lumbar", upper);
    jk2_angle_bone(out, "lower_lumbar", lower);
    jk2_angle_bone(out, "thoracic", thoracic);
    jk2_angle_bone(out, "cervical", cervical);
    return 1;
}
