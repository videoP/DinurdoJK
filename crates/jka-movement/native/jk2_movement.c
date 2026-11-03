/* Isolated host for the unmodified pinned MVSDK JK2 shared gameplay. */
#include "jk2_host_internal.h"
#include "cgame/cg_local.h"
#include <stddef.h>
#include <setjmp.h>

typedef struct jk2_field_s { const char *name; size_t offset; int bits; } jk2_field;
#include "jk2_fields.h"
typedef struct jk2_member_s { const char *name; size_t offset, size; } jk2_member;
#include "jk2_state_members.h"
extern animation_t bgGlobalAnimations[MAX_TOTALANIMATIONS];
extern qboolean BGPAFtextLoaded;

static jmp_buf error_target;
static char last_error[1024];
static const unsigned char *animation_text;
static int animation_length;
static jka_trace_fn trace_callback;
static jka_contents_fn contents_callback;
static jk2_model_trace_fn model_trace_callback;
static void *callback_context;
typedef char jk2_trigger_capacity_check[JK2_SNAPSHOT_ENTITIES == MAX_ENTITIES_IN_SNAPSHOT ? 1 : -1];

void QDECL Com_Printf(const char *format, ...) { (void)format; }
void QDECL Com_Error(int level, const char *format, ...) {
    va_list args; (void)level;
    va_start(args, format); vsnprintf(last_error, sizeof(last_error), format, args); va_end(args);
    longjmp(error_target, 1);
}
void trap_Print(const char *text) { (void)text; }
void trap_Error(const char *text) { Com_Error(ERR_DROP, "%s", text); }
/* jk2mv (engine authority for JK2 client and server) sys_win32/sys_unix
 * Sys_SnapVector: cvtps2dq + cvtdq2ps, i.e. round-half-even through an int
 * (-0.4 -> +0.0). OpenJK's JKA version rounds halves away from zero and keeps
 * -0.0, which diverges from JK2 servers after every Pmove velocity snap. */
#if defined(_M_X64) || defined(__x86_64__) || defined(__SSE2__) || (defined(_M_IX86_FP) && _M_IX86_FP >= 2)
#include <emmintrin.h>
void trap_SnapVector(float *v) {
    float out[4];
    _mm_storeu_ps(out, _mm_cvtepi32_ps(_mm_cvtps_epi32(_mm_set_ps(0.0f, v[2], v[1], v[0]))));
    v[0] = out[0]; v[1] = out[1]; v[2] = out[2];
}
#else
void trap_SnapVector(float *v) {
    v[0] = (float)(int)nearbyintf(v[0]); v[1] = (float)(int)nearbyintf(v[1]); v[2] = (float)(int)nearbyintf(v[2]);
}
#endif
void trap_Cvar_VariableStringBuffer(const char *name, char *out, int size) { (void)name; if (size > 0) out[0] = 0; }
int trap_FS_FOpenFile(const char *name, fileHandle_t *handle, fsMode_t mode) {
    if (mode != FS_READ || strcmp(name, "models/players/_humanoid/animation.cfg")) { *handle = 0; return -1; }
    *handle = 1; return animation_length;
}
void trap_FS_Read(void *out, int length, fileHandle_t handle) {
    if (handle != 1 || length < 0 || length > animation_length) Com_Error(ERR_DROP, "Invalid JK2 animation read");
    memcpy(out, animation_text, length);
}
void trap_FS_FCloseFile(fileHandle_t handle) { (void)handle; }

const char *jk2_movement_error(void) { return last_error; }
static const jk2_field *table(int version) { return version == 2 ? fields_102 : fields_104; }
int jk2_ps_field_count(int version) {
    return version == 2 ? sizeof(fields_102)/sizeof(fields_102[0]) : version == 4 ? sizeof(fields_104)/sizeof(fields_104[0]) : 0;
}
const char *jk2_ps_field_name(int version, int index) {
    return index >= 0 && index < jk2_ps_field_count(version) ? table(version)[index].name : "";
}
static const jk2_field *entity_table(int version) { return version == 2 ? entity_fields_102 : entity_fields_104; }
int jk2_entity_field_count(int version) {
    return version == 2 ? sizeof(entity_fields_102)/sizeof(entity_fields_102[0])
        : version == 4 ? sizeof(entity_fields_104)/sizeof(entity_fields_104[0]) : 0;
}
const char *jk2_entity_field_name(int version, int index) {
    return index >= 0 && index < jk2_entity_field_count(version) ? entity_table(version)[index].name : "";
}
int jk2_contract(int index) {
    const int values[] = { sizeof(jka_cmd), sizeof(jka_trace), sizeof(jk2_settings), sizeof(playerState_t),
        MAX_TOTALANIMATIONS, MAX_TOTALANIMATIONS_1_02, WP_SABER, PM_SPECTATOR, PM_NORMAL, ANIM_TOGGLEBIT, MAX_CLIENTS, MASK_PLAYERSOLID,
        WP_NUM_WEAPONS, NUM_FORCE_POWERS, HI_NUM_HOLDABLE, sizeof(jk2_frame) };
    return index >= 0 && index < sizeof(values)/sizeof(values[0]) ? values[index] : -1;
}
int jk2_select_versions(int version, int gameplay) {
    if ((version != 2 && version != 4) || (gameplay != 2 && gameplay != 4)) return 0;
    jk2version = jk2startversion = version == 2 ? VERSION_1_02 : VERSION_1_04;
    MV_SetGamePlay(gameplay == 2 ? VERSION_1_02 : VERSION_1_04);
    return 1;
}
void jk2_bind_services(jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context) {
    trace_callback = trace; contents_callback = contents; model_trace_callback = model_trace; callback_context = context;
}
void jk2_host_trace(trace_t *result, const vec3_t start, const vec3_t mins, const vec3_t maxs, const vec3_t end, int pass, int mask) {
    jka_trace t = {0};
    trace_callback(callback_context, &t, start, mins, maxs, end, pass, mask);
    memset(result, 0, sizeof(*result));
    result->fraction = t.fraction; VectorCopy(t.end, result->endpos); VectorCopy(t.normal, result->plane.normal);
    result->plane.dist = t.distance; SetPlaneSignbits(&result->plane);
    result->plane.type = PlaneTypeForNormal(result->plane.normal);
    result->surfaceFlags = t.surface_flags; result->contents = t.contents; result->entityNum = t.entity;
    result->allsolid = t.all_solid; result->startsolid = t.start_solid;
}
int jk2_host_contents(const vec3_t point, int pass) { return contents_callback(callback_context, point, pass); }
/* Only startsolid is consumed (CG_TouchTriggerPrediction). */
void jk2_host_model_trace(trace_t *result, const vec3_t origin, const vec3_t mins, const vec3_t maxs, int model) {
    jka_trace t = {0};
    model_trace_callback(callback_context, &t, origin, mins, maxs, model);
    memset(result, 0, sizeof(*result));
    result->fraction = t.fraction; VectorCopy(t.end, result->endpos);
    result->allsolid = t.all_solid; result->startsolid = t.start_solid;
}
void jk2_copy_command(usercmd_t *out, const jka_cmd *cmd) {
    memset(out, 0, sizeof(*out));
    out->serverTime = cmd->time; memcpy(out->angles, cmd->angles, sizeof(out->angles));
    out->buttons = cmd->buttons; out->weapon = cmd->weapon; out->forcesel = cmd->force_selection;
    out->invensel = cmd->inventory_selection; out->generic_cmd = cmd->generic_command;
    out->forwardmove = cmd->forward; out->rightmove = cmd->right; out->upmove = cmd->up;
}
void *jk2_player_new(int version, const unsigned char *animations, int length) {
    jk2_player *player;
    last_error[0] = 0;
    if (!jk2_ps_field_count(version) || length < 1 || length >= 39999) return NULL;
    if (setjmp(error_target)) return NULL;
    animation_text = animations; animation_length = length; BGPAFtextLoaded = qfalse;
    memset(bgGlobalAnimations, 0, sizeof(bgGlobalAnimations));
    if (!BG_ParseAnimationFile("models/players/_humanoid/animation.cfg")) return NULL;
    player = calloc(1, sizeof(*player));
    if (!player) return NULL;
    player->version = version;
    memcpy(player->animations, bgGlobalAnimations, sizeof(player->animations));
    animation_text = NULL; animation_length = 0;
    return player;
}
static void free_world(jk2_player *p) {
    int i;
    for (i = 0; i < MAX_GENTITIES; i++) { free(p->world[i]); p->world[i] = NULL; }
    p->trigger_count = 0;
}
void jk2_player_free(void *player) {
    if (player) free_world(player);
    free(player);
}
void *jk2_player_clone(const void *opaque) {
    const jk2_player *source = opaque;
    jk2_player *copy = malloc(sizeof(*copy));
    int i;
    if (!copy) return NULL;
    memcpy(copy, source, sizeof(*copy)); memset(&copy->move, 0, sizeof(copy->move));
    for (i = 0; i < MAX_GENTITIES; i++) {
        if (!source->world[i]) continue;
        copy->world[i] = malloc(sizeof(jk2_centity));
        if (!copy->world[i]) {
            while (i-- > 0) if (source->world[i]) free(copy->world[i]);
            free(copy);
            return NULL;
        }
        *copy->world[i] = *source->world[i];
    }
    return copy;
}
static void save_triggers(const jk2_player *p, int saved[][3]) {
    int i;
    for (i = 0; i < p->trigger_count; i++) {
        const jk2_centity *c = p->world[p->trigger[i]];
        saved[i][0] = c->currentState.eFlags; saved[i][1] = c->miscTime; saved[i][2] = c->predicted_nodraw;
    }
}
static void load_triggers(jk2_player *p, int saved[][3]) {
    int i;
    for (i = 0; i < p->trigger_count; i++) {
        jk2_centity *c = p->world[p->trigger[i]];
        c->currentState.eFlags = saved[i][0]; c->miscTime = saved[i][1]; c->predicted_nodraw = saved[i][2];
    }
}
static int map104(int anim) {
    const int index = anim & ~ANIM_TOGGLEBIT;
    return index >= 0 && index < MAX_TOTALANIMATIONS_1_02 ? MV_MapAnimation104(anim) : -1;
}
static int valid_state(const playerState_t *ps, int version) {
    const int count = version == 2 ? MAX_TOTALANIMATIONS_1_02 : MAX_TOTALANIMATIONS;
    return ps->clientNum >= 0 && ps->clientNum < MAX_CLIENTS && ps->weapon >= 0 && ps->weapon < WP_NUM_WEAPONS
        && ps->pm_type >= PM_NORMAL && ps->pm_type <= PM_SPINTERMISSION
        && ps->saberMove >= LS_INVALID && ps->saberMove < LS_MOVE_MAX
        && (ps->legsAnim & ~ANIM_TOGGLEBIT) >= 0 && (ps->legsAnim & ~ANIM_TOGGLEBIT) < count
        && (ps->torsoAnim & ~ANIM_TOGGLEBIT) >= 0 && (ps->torsoAnim & ~ANIM_TOGGLEBIT) < count
        && ps->fd.forcePowerLevel[FP_LEVITATION] >= 0 && ps->fd.forcePowerLevel[FP_LEVITATION] <= 3
        && ps->fd.forcePowerLevel[FP_SEE] >= 0 && ps->fd.forcePowerLevel[FP_SEE] <= 3;
}
static int import_state(int version, playerState_t *out, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups) {
    playerState_t ps = {0};
    const jk2_field *layout = table(version);
    int i;
    if (count != jk2_ps_field_count(version)) return 0;
    for (i = 0; i < count; i++) {
        if (!layout[i].bits) { float value; memcpy(&value, &fields[i], 4); if (!isfinite(value)) return 0; }
        memcpy((unsigned char *)&ps + layout[i].offset, &fields[i], 4);
    }
    memcpy(ps.stats, stats, 16*4); memcpy(ps.persistant, persistant, 16*4);
    memcpy(ps.ammo, ammo, 16*4); memcpy(ps.powerups, powerups, 16*4);
    if (!valid_state(&ps, version)) return 0;
    if (version == 2) {
        /* Same semantic conversions as pinned CG_GetSnapshot, after importing
         * each field into this SDK's own native playerState_t layout. */
        ps.legsAnim = map104(ps.legsAnim); ps.torsoAnim = map104(ps.torsoAnim);
        ps.legsAnimExecute = map104(ps.legsAnimExecute); ps.torsoAnimExecute = map104(ps.torsoAnimExecute);
        ps.fullAnimExecute = map104(ps.fullAnimExecute); ps.saberAttackSequence = map104(ps.saberAttackSequence);
        if (ps.forceHandExtend == HANDEXTEND_TAUNT || ps.forceHandExtend == HANDEXTEND_DODGE) {
            ps.forceDodgeAnim = map104(ps.forceDodgeAnim); if (ps.forceDodgeAnim < 0) return 0;
        }
        if (ps.saberBlocked > BLOCKED_NONE) ps.saberBlocked++;
    }
    if ((ps.forceHandExtend == HANDEXTEND_TAUNT || ps.forceHandExtend == HANDEXTEND_DODGE)
        && ((ps.forceDodgeAnim & ~ANIM_TOGGLEBIT) < 0 || (ps.forceDodgeAnim & ~ANIM_TOGGLEBIT) >= MAX_TOTALANIMATIONS)) return 0;
    *out = ps;
    return 1;
}
int jk2_import_wire(int version, const jk2_wire *wire, playerState_t *out) {
    return wire && import_state(version, out, wire->fields, wire->count,
        wire->stats, wire->persistant, wire->ammo, wire->powerups);
}
int jk2_player_set_network(void *opaque, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups) {
    jk2_player *p = opaque;
    if (!import_state(p->version, &p->ps, fields, count, stats, persistant, ammo, powerups)) return 0;
    memset(&p->move, 0, sizeof(p->move));
    return 1;
}
int jk2_player_get_network(const void *opaque, int normalized, uint32_t *fields, int count,
    int32_t *stats, int32_t *persistant, int32_t *ammo, int32_t *powerups) {
    const jk2_player *p = opaque;
    playerState_t ps = p->ps;
    const int version = normalized ? 4 : p->version;
    const jk2_field *layout = table(version);
    int i;
    if (count != jk2_ps_field_count(version)) return 0;
    if (!normalized && version == 2) {
        ps.legsAnim = MV_MapAnimation102(ps.legsAnim); ps.torsoAnim = MV_MapAnimation102(ps.torsoAnim);
        ps.legsAnimExecute = MV_MapAnimation102(ps.legsAnimExecute); ps.torsoAnimExecute = MV_MapAnimation102(ps.torsoAnimExecute);
        ps.fullAnimExecute = MV_MapAnimation102(ps.fullAnimExecute); ps.saberAttackSequence = MV_MapAnimation102(ps.saberAttackSequence);
        if (ps.forceHandExtend == HANDEXTEND_TAUNT || ps.forceHandExtend == HANDEXTEND_DODGE) ps.forceDodgeAnim = MV_MapAnimation102(ps.forceDodgeAnim);
        if (ps.saberBlocked > BLOCKED_NONE) ps.saberBlocked--;
    }
    for (i = 0; i < count; i++) memcpy(&fields[i], (const unsigned char *)&ps + layout[i].offset, 4);
    memcpy(stats, ps.stats, 16*4); memcpy(persistant, ps.persistant, 16*4);
    memcpy(ammo, ps.ammo, 16*4); memcpy(powerups, ps.powerups, 16*4);
    return 1;
}
int jk2_player_predict(void *opaque, const jka_cmd *cmd, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context) {
    jk2_player *p = opaque;
    jk2_centity *list[JK2_SNAPSHOT_ENTITIES];
    int i;
    last_error[0] = 0;
    if (settings->spectator_server_timing && p->ps.pm_type != PM_SPECTATOR) {
        snprintf(last_error, sizeof(last_error), "JK2 server spectator timing requires PM_SPECTATOR");
        return 0;
    }
    if (!jk2_select_versions(p->version, settings->gameplay_version) || settings->pmove_msec < 8 || settings->pmove_msec > 33
        || !trace || !contents || !model_trace || cmd->weapon >= WP_NUM_WEAPONS || (cmd->force_selection != 255 && cmd->force_selection >= NUM_FORCE_POWERS)
        || (cmd->inventory_selection != 255 && cmd->inventory_selection >= HI_NUM_HOLDABLE)) return 0;
    p->checkpoint = p->ps;
    memcpy(p->remote_checkpoint, p->remote, sizeof(p->remote));
    save_triggers(p, p->trigger_checkpoint);
    p->hyperspace_checkpoint = p->hyperspace;
    if (setjmp(error_target)) { jk2_player_rollback(p); return 0; }
    jk2_bind_services(trace, contents, model_trace, context);
    memset(&p->move, 0, sizeof(p->move)); p->move.ps = &p->ps;
    p->move.animations = p->animations; p->move.trace = jk2_host_trace; p->move.pointcontents = jk2_host_contents;
    p->move.gametype = settings->gametype; p->move.tracemask = settings->tracemask;
    /* ClientThink_real rounds ucmd time before SpectatorThink. The latter
     * zeroes pmove_t and never installs fixed subdivision (g_active.c).
     * Keep the CGame/default mode separately available for replay diagnostics. */
    p->move.pmove_fixed = settings->spectator_server_timing ? 0 : settings->pmove_fixed;
    p->move.pmove_msec = settings->pmove_msec; p->move.noFootsteps = settings->no_footsteps;
    jk2_copy_command(&p->move.cmd, cmd);
    if (settings->pmove_fixed) {
        PM_UpdateViewAngles(&p->ps, &p->move.cmd);
    }
    /* CG_PredictPlayerState skips already acknowledged commands before fixed
     * timestamp rounding. Calling Pmove at equal times changes framecount. */
    if (p->move.cmd.serverTime <= p->ps.commandTime) return 1;
    if (settings->pmove_fixed) {
        p->move.cmd.serverTime = ((p->move.cmd.serverTime + settings->pmove_msec-1)/settings->pmove_msec)*settings->pmove_msec;
    }
    jk2_context_pmove(&p->move, p->remote, p->send, &p->frame, p->context_ready);
    // add push trigger movement effects
    for (i = 0; i < p->trigger_count; i++) list[i] = p->world[p->trigger[i]];
    if (jk2_touch_triggers(&p->ps, &p->move, list, p->trigger_count, settings, p->frame.cg_time)) p->hyperspace = 1;
    return 1;
}
void jk2_player_rollback(void *opaque) {
    jk2_player *p = opaque;
    p->ps = p->checkpoint;
    memcpy(p->remote, p->remote_checkpoint, sizeof(p->remote));
    load_triggers(p, p->trigger_checkpoint);
    p->hyperspace = p->hyperspace_checkpoint;
}

/* CG_Init zeroes cg_entities; snapshot transitions then overwrite the slots
 * present in each snapshot and leave absent clients' last state in place. */
void jk2_context_reset(void *opaque) {
    jk2_player *p = opaque;
    memset(p->remote, 0, sizeof(p->remote));
    memset(&p->frame, 0, sizeof(p->frame));
    p->context_ready = 0;
    free_world(p);
    p->hyperspace = 0;
}
/* CG_TransitionSnapshot: BG_PlayerStateToEntityState(&cg.snap->ps, ...),
 * with the playerstate already normalized as CG_GetSnapshot does. */
int jk2_context_set_self(void *opaque, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups) {
    jk2_player *p = opaque;
    playerState_t ps;
    if (!import_state(p->version, &ps, fields, count, stats, persistant, ammo, powerups)) return 0;
    BG_PlayerStateToEntityState(&ps, &p->remote[ps.clientNum], qfalse);
    p->context_ready = 1;
    return 1;
}
static int valid_anim(int anim, int version) {
    const int index = anim & ~ANIM_TOGGLEBIT;
    return index >= 0 && index < (version == 2 ? MAX_TOTALANIMATIONS_1_02 : MAX_TOTALANIMATIONS);
}
/* CG_TransitionEntity for a client slot: the whole wire entityState_t, then
 * MVSDK's 1.02 torso/legs mapping. */
int jk2_context_set_entity(void *opaque, int number, const uint32_t *fields, int count) {
    jk2_player *p = opaque;
    entityState_t s = {0};
    const jk2_field *layout = entity_table(p->version);
    int i;
    if (number < 0 || number >= MAX_CLIENTS || count != jk2_entity_field_count(p->version)) return 0;
    for (i = 0; i < count; i++) {
        if (!layout[i].bits) { float value; memcpy(&value, &fields[i], 4); if (!isfinite(value)) return 0; }
        memcpy((unsigned char *)&s + layout[i].offset, &fields[i], 4);
    }
    s.number = number;
    if (!valid_anim(s.torsoAnim, p->version) || !valid_anim(s.legsAnim, p->version)) return 0;
    if (p->version == 2) { s.torsoAnim = map104(s.torsoAnim); s.legsAnim = map104(s.legsAnim); }
    p->remote[number] = s;
    return 1;
}
/* Native (1.04-identifier) slot contents, for diagnostics and tests. */
int jk2_context_get_entity(const void *opaque, int number, uint32_t *fields, int count) {
    const jk2_player *p = opaque;
    const jk2_field *layout = entity_table(4);
    int i;
    if (number < 0 || number >= MAX_CLIENTS || count != jk2_entity_field_count(4)) return 0;
    for (i = 0; i < count; i++) memcpy(&fields[i], (const unsigned char *)&p->remote[number] + layout[i].offset, 4);
    return 1;
}
/* Also CG_PredictPlayerState's `cg.hyperspace = qfalse`. */
void jk2_context_set_frame(void *opaque, const jk2_frame *frame) {
    jk2_player *p = opaque;
    p->frame = *frame;
    p->hyperspace = 0;
}
void jk2_context_mark(void *opaque) {
    jk2_player *p = opaque;
    memcpy(p->remote_mark, p->remote, sizeof(p->remote));
    p->context_ready_mark = p->context_ready;
    save_triggers(p, p->trigger_mark);
    p->hyperspace_mark = p->hyperspace;
}
void jk2_context_restore_mark(void *opaque) {
    jk2_player *p = opaque;
    memcpy(p->remote, p->remote_mark, sizeof(p->remote));
    p->context_ready = p->context_ready_mark;
    load_triggers(p, p->trigger_mark);
    p->hyperspace = p->hyperspace_mark;
}
/* CG_TransitionEntity / CG_SetInitialSnapshot for a trigger-type slot: the
 * whole wire entityState_t, MVSDK's 1.02 torso/legs mapping, then the
 * mvsdk_svFlags submodel index fixups. A transition replaces currentState,
 * which drops any predicted EF_NODRAW; miscTime belongs to the centity. */
int jk2_context_set_world_entity(void *opaque, int number, const uint32_t *fields, int count, int svflags) {
    jk2_player *p = opaque;
    entityState_t s = {0};
    const jk2_field *layout = entity_table(p->version);
    int i;
    if (number < MAX_CLIENTS || number >= MAX_GENTITIES || count != jk2_entity_field_count(p->version)) return 0;
    for (i = 0; i < count; i++) {
        if (!layout[i].bits) { float value; memcpy(&value, &fields[i], 4); if (!isfinite(value)) return 0; }
        memcpy((unsigned char *)&s + layout[i].offset, &fields[i], 4);
    }
    s.number = number;
    if (s.eType != ET_ITEM && s.eType != ET_PUSH_TRIGGER && s.eType != ET_TELEPORT_TRIGGER) return 0;
    if (!valid_anim(s.torsoAnim, p->version) || !valid_anim(s.legsAnim, p->version)) return 0;
    if (p->version == 2) { s.torsoAnim = map104(s.torsoAnim); s.legsAnim = map104(s.legsAnim); }
    if ( s.solid == SOLID_BMODEL && s.time2 && svflags & MVSDK_SVFLAG_SUBMODEL_TIME2 )
    {
        s.modelindex = s.time2;
    }
    else if ( s.solid == SOLID_BMODEL && s.modelindex < 0 && svflags & MVSDK_SVFLAG_SUBMODEL_WORKAROUND )
    {
        s.modelindex += 256;
    }
    if (!p->world[number]) {
        p->world[number] = calloc(1, sizeof(jk2_centity));
        if (!p->world[number]) return 0;
    }
    p->world[number]->currentState = s;
    p->world[number]->predicted_nodraw = 0;
    return 1;
}
/* Slots whose transitioned currentState is not a trigger type stop being
 * candidates (CG_BuildSolidList filters by currentState.eType). The list
 * itself is rebuilt by the next jk2_context_set_triggers. */
void jk2_context_clear_world_entities(void *opaque, const int32_t *numbers, int count) {
    jk2_player *p = opaque;
    int i;
    for (i = 0; i < count; i++) {
        if (numbers[i] < MAX_CLIENTS || numbers[i] >= MAX_GENTITIES) continue;
        free(p->world[numbers[i]]); p->world[numbers[i]] = NULL;
    }
    p->trigger_count = 0;
}
void jk2_context_set_triggers(void *opaque, const int32_t *numbers, int count) {
    jk2_player *p = opaque;
    int i;
    p->trigger_count = 0;
    for (i = 0; i < count && p->trigger_count < JK2_SNAPSHOT_ENTITIES; i++) {
        if (numbers[i] < MAX_CLIENTS || numbers[i] >= MAX_GENTITIES || !p->world[numbers[i]]) continue;
        p->trigger[p->trigger_count++] = numbers[i];
    }
}
int jk2_context_hyperspace(const void *opaque) { return ((const jk2_player *)opaque)->hyperspace; }
int jk2_context_predicted_nodraw(const void *opaque, int32_t *numbers, int capacity) {
    const jk2_player *p = opaque;
    int i, n = 0;
    for (i = MAX_CLIENTS; i < MAX_GENTITIES; i++) {
        if (!p->world[i] || !p->world[i]->predicted_nodraw) continue;
        if (n < capacity) numbers[n] = i;
        n++;
    }
    return n;
}
int jk2_native_state(const void *opaque, unsigned char *out, int size) {
    if (size != sizeof(playerState_t)) return 0;
    memcpy(out, &((const jk2_player *)opaque)->ps, size); return 1;
}
const char *jk2_native_word_name(int offset) {
    int i;
    static char indexed_name[128]; /* caller holds the shared native lock */
    for (i = 0; i < jk2_ps_field_count(4); i++) if (fields_104[i].offset == offset) return fields_104[i].name;
    for (i = 0; i < sizeof(native_members)/sizeof(native_members[0]); i++) {
        const jk2_member *m = &native_members[i];
        if (offset >= m->offset && offset < m->offset+m->size) {
            if (m->size == 4) return m->name;
            snprintf(indexed_name, sizeof(indexed_name), "%s[%u]", m->name, (unsigned)(offset-m->offset)/4);
            return indexed_name;
        }
    }
    return "native padding";
}

/* Read-only view of the pinned MVSDK bg_itemlist (immutable static data).
 * strings[]: classname, world_model[0], world_model[1], view_model, icon,
 * pickup_sound; "" for none. Returns 0 for an out-of-range index. */
int jk2_item_info(int index, const char **strings, int *type, int *tag, int *quantity) {
    const gitem_t *item;
    if (index < 0 || index >= bg_numItems) return 0;
    item = &bg_itemlist[index];
    strings[0] = item->classname ? item->classname : "";
    strings[1] = item->world_model[0] ? item->world_model[0] : "";
    strings[2] = item->world_model[1] ? item->world_model[1] : "";
    strings[3] = item->view_model ? item->view_model : "";
    strings[4] = item->icon ? item->icon : "";
    strings[5] = item->pickup_sound ? item->pickup_sound : "";
    *type = (int)item->giType; *tag = item->giTag; *quantity = item->quantity;
    return 1;
}

/* Pinned BG_LegalizedForcePowers, as the JK2 UI applies to its force
 * configuration after `nfr` (ui_rankChange). Returns 1 if already legal,
 * 0 if rewritten, -1 on invalid input. */
int jk2_legalize_force_powers(int version, int gameplay, char *powers, int size,
    int max_rank, int free_saber, int team_force, int gametype, int fp_disabled) {
    char buffer[128];
    int legal;
    last_error[0] = 0;
    if (!powers || size < 2 || size > (int)sizeof(buffer) || !memchr(powers, 0, size)
        || strlen(powers) >= sizeof(buffer) || !jk2_select_versions(version, gameplay)) return -1;
    Q_strncpyz(buffer, powers, sizeof(buffer));
    if (setjmp(error_target)) return -1;
    legal = BG_LegalizedForcePowers(buffer, max_rank, free_saber, team_force, gametype, fp_disabled);
    if ((int)strlen(buffer) >= size) return -1;
    Q_strncpyz(powers, buffer, size);
    return legal ? 1 : 0;
}

extern int jk2_oracle_replay_impl(const void *, const jka_cmd *, int, const jk2_settings *, jka_trace_fn, jka_contents_fn,
    jk2_model_trace_fn, void *, unsigned char *, int);
int jk2_oracle_replay(const void *seed, const jka_cmd *commands, int count, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context,
    unsigned char *states, int state_size) {
    if (setjmp(error_target)) return 0;
    return jk2_oracle_replay_impl(seed, commands, count, settings, trace, contents, model_trace, context, states, state_size);
}
