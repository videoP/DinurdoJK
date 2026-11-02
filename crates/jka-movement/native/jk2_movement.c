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
extern void Sys_SnapVector(float *);

static jmp_buf error_target;
static char last_error[1024];
static const unsigned char *animation_text;
static int animation_length;
static jka_trace_fn trace_callback;
static jka_contents_fn contents_callback;
static void *callback_context;

void QDECL Com_Printf(const char *format, ...) { (void)format; }
void QDECL Com_Error(int level, const char *format, ...) {
    va_list args; (void)level;
    va_start(args, format); vsnprintf(last_error, sizeof(last_error), format, args); va_end(args);
    longjmp(error_target, 1);
}
void trap_Print(const char *text) { (void)text; }
void trap_Error(const char *text) { Com_Error(ERR_DROP, "%s", text); }
void trap_SnapVector(float *v) { Sys_SnapVector(v); }
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
int jk2_contract(int index) {
    const int values[] = { sizeof(jka_cmd), sizeof(jka_trace), sizeof(jk2_settings), sizeof(playerState_t),
        MAX_TOTALANIMATIONS, MAX_TOTALANIMATIONS_1_02, WP_SABER, PM_SPECTATOR, PM_NORMAL, ANIM_TOGGLEBIT, MAX_CLIENTS, MASK_PLAYERSOLID,
        WP_NUM_WEAPONS, NUM_FORCE_POWERS, HI_NUM_HOLDABLE };
    return index >= 0 && index < sizeof(values)/sizeof(values[0]) ? values[index] : -1;
}
int jk2_select_versions(int version, int gameplay) {
    if ((version != 2 && version != 4) || (gameplay != 2 && gameplay != 4)) return 0;
    jk2version = jk2startversion = version == 2 ? VERSION_1_02 : VERSION_1_04;
    MV_SetGamePlay(gameplay == 2 ? VERSION_1_02 : VERSION_1_04);
    return 1;
}
void jk2_bind_services(jka_trace_fn trace, jka_contents_fn contents, void *context) {
    trace_callback = trace; contents_callback = contents; callback_context = context;
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
void jk2_copy_command(usercmd_t *out, const jka_cmd *cmd) {
    memset(out, 0, sizeof(*out));
    out->serverTime = cmd->time; memcpy(out->angles, cmd->angles, sizeof(out->angles));
    out->buttons = cmd->buttons; out->weapon = cmd->weapon; out->forcesel = cmd->force_selection;
    out->invensel = cmd->inventory_selection; out->generic_cmd = cmd->generic_command;
    out->forwardmove = cmd->forward; out->rightmove = cmd->right; out->upmove = cmd->up;
}
void jk2_require_context(const playerState_t *ps, int time) {
    int anim = ps->torsoAnim & ~ANIM_TOGGLEBIT;
    if (ps->saberLockTime > time || ps->saberLockFrame || anim == BOTH_BF2LOCK || anim == BOTH_BF1LOCK
        || anim == BOTH_CWCIRCLELOCK || anim == BOTH_CCWCIRCLELOCK)
        Com_Error(ERR_DROP, "JK2 saber-lock prediction requires remote client state");
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
void jk2_player_free(void *player) { free(player); }
void *jk2_player_clone(const void *opaque) {
    jk2_player *copy = malloc(sizeof(*copy));
    if (copy) { memcpy(copy, opaque, sizeof(*copy)); memset(&copy->move, 0, sizeof(copy->move)); }
    return copy;
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
int jk2_player_set_network(void *opaque, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups) {
    jk2_player *p = opaque;
    playerState_t ps = {0};
    const jk2_field *layout = table(p->version);
    int i;
    if (count != jk2_ps_field_count(p->version)) return 0;
    for (i = 0; i < count; i++) {
        if (!layout[i].bits) { float value; memcpy(&value, &fields[i], 4); if (!isfinite(value)) return 0; }
        memcpy((unsigned char *)&ps + layout[i].offset, &fields[i], 4);
    }
    memcpy(ps.stats, stats, 16*4); memcpy(ps.persistant, persistant, 16*4);
    memcpy(ps.ammo, ammo, 16*4); memcpy(ps.powerups, powerups, 16*4);
    if (!valid_state(&ps, p->version)) return 0;
    if (p->version == 2) {
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
    p->ps = ps; memset(&p->move, 0, sizeof(p->move));
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
    jka_trace_fn trace, jka_contents_fn contents, void *context) {
    jk2_player *p = opaque; int i;
    last_error[0] = 0;
    if (!jk2_select_versions(p->version, settings->gameplay_version) || settings->pmove_msec < 8 || settings->pmove_msec > 33
        || !trace || !contents || cmd->weapon >= WP_NUM_WEAPONS || (cmd->force_selection != 255 && cmd->force_selection >= NUM_FORCE_POWERS)
        || (cmd->inventory_selection != 255 && cmd->inventory_selection >= HI_NUM_HOLDABLE)) return 0;
    p->checkpoint = p->ps;
    if (setjmp(error_target)) { p->ps = p->checkpoint; return 0; }
    jk2_bind_services(trace, contents, context);
    memset(&p->move, 0, sizeof(p->move)); p->move.ps = &p->ps;
    p->move.animations = p->animations; p->move.trace = jk2_host_trace; p->move.pointcontents = jk2_host_contents;
    p->move.gametype = settings->gametype; p->move.tracemask = settings->tracemask;
    p->move.pmove_fixed = settings->pmove_fixed; p->move.pmove_msec = settings->pmove_msec; p->move.noFootsteps = settings->no_footsteps;
    for (i = 0; i < MAX_CLIENTS; i++) p->move.bgClients[i] = &p->clients[i];
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
    jk2_require_context(&p->ps, p->move.cmd.serverTime);
    Pmove(&p->move);
    return 1;
}
void jk2_player_rollback(void *opaque) { jk2_player *p = opaque; p->ps = p->checkpoint; }
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

extern int jk2_oracle_replay_impl(const void *, const jka_cmd *, int, const jk2_settings *, jka_trace_fn, jka_contents_fn, void *, unsigned char *, int);
int jk2_oracle_replay(const void *seed, const jka_cmd *commands, int count, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, void *context, unsigned char *states, int state_size) {
    if (setjmp(error_target)) return 0;
    return jk2_oracle_replay_impl(seed, commands, count, settings, trace, contents, context, states, state_size);
}
