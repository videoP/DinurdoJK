#pragma once

/* Diagnostic sink is installed only while holding the recursive native lock.
 * No allocation or output in the normal Pmove path. */
typedef struct jka_jump_event_s {
    int stage, time, msec, flags, ground, walking, ground_plane, level, active;
    int entity, bump;
    float origin[3], before[3], after[3], normal[3];
    float jump_start, fraction, overbounce;
} jka_jump_event;
extern int jka_jump_trace_enabled;
extern int jka_force_jump_slope_enabled;
void jka_jump_trace_record(const jka_jump_event *event);
void jka_jump_trace_begin(jka_jump_event *events, int capacity);
int jka_jump_trace_end(void);

#define JKA_JUMP_TRACE(kind, vin, vout, plane, frac, ent, bumpnum, bounce) do { \
    if (jka_jump_trace_enabled) { \
        jka_jump_event e; int jka_i; \
        e.stage = (kind); e.time = pm->cmd.serverTime; e.msec = pml.msec; \
        e.flags = pm->ps->pm_flags; e.ground = pm->ps->groundEntityNum; \
        e.walking = pml.walking; e.ground_plane = pml.groundPlane; \
        e.level = pm->ps->fd.forcePowerLevel[FP_LEVITATION]; \
        e.active = pm->ps->fd.forcePowersActive; e.entity = (ent); e.bump = (bumpnum); \
        e.jump_start = pm->ps->fd.forceJumpZStart; e.fraction = (frac); e.overbounce = (bounce); \
        for (jka_i = 0; jka_i < 3; ++jka_i) { \
            e.origin[jka_i] = pm->ps->origin[jka_i]; \
            e.before[jka_i] = (vin)[jka_i]; e.after[jka_i] = (vout)[jka_i]; \
            e.normal[jka_i] = (plane)[jka_i]; \
        } \
        jka_jump_trace_record(&e); \
    } \
} while (0)
