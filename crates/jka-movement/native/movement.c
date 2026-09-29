/* The host owns state and services. Build this once for stock OpenJK and once
 * for the pinned TaystJK/JAPRO sources, with isolated native symbols. */
#include "qcommon/q_shared.h"
#include "cgame/cg_local.h"
#include "game/bg_local.h"
#include "bridge.h"
#include <setjmp.h>

cgs_t cgs;
centity_t cg_entities[MAX_GENTITIES];
vmCvar_t bg_fighterAltControl;
vehWeaponInfo_t g_vehWeaponInfo[MAX_VEH_WEAPONS];
static cgameImport_t imports;
cgameImport_t *trap = &imports;
#ifdef JKA_JAPRO
cg_t cg;
vmCvar_t cp_pluginDisable, cg_jumpHeight, cg_legstuck, pmove_fixed;
int cg_dueltypes[MAX_CLIENTS];
static snapshot_t prediction_snapshot;
static playerState_t remote_players[MAX_GENTITIES];
static char legacy_fixes_string[32] = "0";
static uint32_t previous_legacy_fixes;
static int legacy_fixes_initialized;
static uint64_t next_entities_revision = 1;
void japro_configure(const jka_predict_settings *settings) {
    cgs.serverMod = SVMOD_JAPRO;
    cgs.gametype = settings->gametype;
    cgs.jcinfo = cgs.cinfo = settings->jcinfo;
    cgs.jcinfo2 = settings->jcinfo2;
    cgs.taystJKinfo = settings->taystjk_info;
    cgs.dmflags = settings->dmflags;
    cgs.hookpull = settings->hook_pull;
    cgs.restricts = settings->restricts;
    cgs.legacyProtocol = qfalse;
    cp_pluginDisable.integer = settings->plugin_disable;
    pmove_fixed.integer = settings->pmove_fixed;
    cg.snap = &prediction_snapshot;
    if (!legacy_fixes_initialized || previous_legacy_fixes != settings->legacy_fixes) {
        snprintf(legacy_fixes_string, sizeof(legacy_fixes_string), "%u", settings->legacy_fixes);
        BG_FixSaberMoveData();
        BG_FixWeaponAttackAnim();
        previous_legacy_fixes = settings->legacy_fixes;
        legacy_fixes_initialized = 1;
    }
}
#endif
static jmp_buf error_target;
static char last_error[1024];
static const unsigned char *animation_file;
static int animation_length;
static jka_trace_fn trace_callback;
static jka_contents_fn contents_callback;
static void *callback_context;
int jka_contract(int index) {
    const int values[] = { sizeof(jka_cmd), sizeof(jka_trace), sizeof(jka_view), WP_SABER, PM_NOCLIP, PM_SPECTATOR,
        BUTTON_ATTACK, BUTTON_WALKING, BUTTON_ALT_ATTACK, ENTITYNUM_WORLD, ENTITYNUM_NONE, PMF_DUCKED, PMF_ROLLING, PMF_STUCK_TO_WALL,
        FP_SPEED, FP_RAGE, sizeof(jka_entity_view), sizeof(jka_player_angle_entity),
        sizeof(jka_player_angle_state), sizeof(jka_bone_angle_command), sizeof(jka_player_angle_result),
        sizeof(jka_saber_movement_info), sizeof(jka_predict_settings), sizeof(jka_prediction_entity) };
    return index >= 0 && index < sizeof(values)/sizeof(values[0]) ? values[index] : -1;
}

static NORETURN void host_error(int level, const char *format, ...) {
    va_list args;
    va_start(args, format);
    vsnprintf(last_error, sizeof(last_error), format, args);
    va_end(args);
    longjmp(error_target, 1);
}
static void host_print(const char *format, ...) { (void)format; }
NORETURN_PTR void (*Com_Error)(int, const char *, ...) = host_error;
void (*Com_Printf)(const char *, ...) = host_print;

const char *CG_ConfigString(int index) {
#ifdef JKA_JAPRO
    if (index == CS_LEGACY_FIXES) return legacy_fixes_string;
#endif
    return "0";
}
void CG_GetVehicleCamPos(vec3_t position) { host_error(ERR_DROP, "Vehicle camera is outside the offline player host"); }
qboolean BG_FighterUpdate(Vehicle_t *vehicle, const usercmd_t *cmd, vec3_t mins, vec3_t maxs,
                         float gravity, void (*trace)(trace_t *, const vec3_t, const vec3_t, const vec3_t, const vec3_t, int, int)) {
    host_error(ERR_DROP, "Vehicle simulation requires a game entity host");
    return qfalse;
}
static int animation_open(const char *name, fileHandle_t *handle, fsMode_t mode) {
    if (strcmp(name, "models/players/_humanoid/animation.cfg") || mode != FS_READ) { *handle = 0; return -1; }
    *handle = 1;
    return animation_length;
}
static void animation_read(void *buffer, int length, fileHandle_t handle) {
    if (handle != 1 || length < 0 || length > animation_length) host_error(ERR_DROP, "Invalid animation read");
    memcpy(buffer, animation_file, length);
}
static void animation_close(fileHandle_t handle) { (void)handle; }
static void host_malloc(void **p, int size) { *p = calloc(1, size); if (!*p) host_error(ERR_DROP, "Allocation failed"); }
static void host_free(void **p) { free(*p); *p = NULL; }
extern void Sys_SnapVector(float *v);

int jka_load_animations(const unsigned char *data, int length) {
    if (setjmp(error_target)) return 0;
    if (length <= 0 || length >= 59999) return 0;
    animation_file = data;
    animation_length = length;
    imports.FS_Open = animation_open;
    imports.FS_Read = animation_read;
    imports.FS_Close = animation_close;
    imports.TrueMalloc = host_malloc;
    imports.TrueFree = host_free;
    imports.SnapVector = Sys_SnapVector;
    BG_InitAnimsets();
    BGPAFtextLoaded = qfalse;
    return BG_ParseAnimationFile("models/players/_humanoid/animation.cfg", bgHumanoidAnimations, qtrue) == 0;
}
const char *jka_movement_error(void) { return last_error; }
extern qboolean BG_SuperBreakWinAnim(int anim);

/* Read-only presentation queries from the exact vendored OpenJK gameplay data.
 * Keep saber trail timing/animation classification owned by the stock C tables
 * instead of copying those tables into the Rust renderer. */
int jka_saber_move_trail_length(int move) {
    if (move < LS_NONE || move >= LS_MOVE_MAX) return 0;
    return (int)saberMoveData[move].trailLength;
}
int jka_super_break_win_anim(int anim) {
    return BG_SuperBreakWinAnim(anim) ? 1 : 0;
}

typedef struct jka_player_s {
    playerState_t ps;
    pmove_t move;
    /* gclient_t state needed by the lightweight local-server generic-command
     * path. These are intentionally server-side and are not transmitted. */
    int last_generic_cmd;
    int last_generic_cmd_time;
    int saber_cycle_queue;
    /* Per-player saberInfo_t state. Keep this on the player object rather than
     * permanently in global cgs.clientinfo so local and remote prediction
     * cannot contaminate each other through the _CGAME OpenJK host. */
    saberInfo_t saber[2];
    qboolean saber_present[2];
    /* Host-only presentation metadata. This deliberately does not live in
     * playerState_t because OpenJK does not network this fact. */
    qboolean view_forced;
#ifdef JKA_JAPRO
    jka_prediction_entity entities[MAX_GENTITIES];
    int entity_count;
    uint64_t entities_revision;
#endif
} jka_player;

/* PM_SetPMViewAngle is OpenJK's authoritative "pmove owns the view" path.
 * Pmove may split one command into several PmoveSingle calls, so reset at the
 * start of every single step. The value left after Pmove returns therefore
 * describes the final step rather than OR-ing stale state from an earlier one. */
static playerState_t *view_tracking_ps;
static qboolean view_forced_for_current_single;

void jka_pmove_begin_view_tracking(playerState_t *ps) {
    if (ps == view_tracking_ps)
        view_forced_for_current_single = qfalse;
}

void jka_pmove_note_forced_view(playerState_t *ps) {
    if (ps == view_tracking_ps)
        view_forced_for_current_single = qtrue;
}

static void begin_pmove_view_tracking(playerState_t *ps) {
    view_tracking_ps = ps;
    view_forced_for_current_single = qfalse;
}

static qboolean end_pmove_view_tracking(void) {
    qboolean forced = view_forced_for_current_single;
    view_tracking_ps = NULL;
    view_forced_for_current_single = qfalse;
    return forced;
}

#ifdef JKA_JAPRO
int japro_set_entities(void *player, const jka_prediction_entity *entities, int count) {
    jka_player *p = player;
    int i;
    if (count < 0 || count > MAX_GENTITIES) return 0;
    for (i = 0; i < count; ++i)
        if (entities[i].number < 0 || entities[i].number >= MAX_GENTITIES) return 0;
    memcpy(p->entities, entities, count * sizeof(*entities));
    p->entity_count = count;
    p->entities_revision = next_entities_revision++;
    return 1;
}

/* CG_ClipMoveToEntities' JAPRO prediction rules. PointContents deliberately
 * retains all inline models, as in the reference. */
int japro_clip_entity(const void *player, const jka_prediction_entity *ent) {
    const playerState_t *ps = &((const jka_player *)player)->ps;
    if (ent->entity_type == ET_SPECIAL && ent->model_index == HI_SHIELD) return 0;
    if (ps->duelInProgress) {
        if (ent->number != ps->duelIndex && ent->entity_type != ET_MOVER) return 0;
    } else {
        if (ent->bolt1 && ent->entity_type == ET_PLAYER) return 0;
        if (ps->stats[STAT_RACEMODE]) {
            if (ent->entity_type != ET_MOVER) return 0;
            if (ent->trajectory_type != TR_SINE &&
                (VectorLengthSquared(ent->velocity) || VectorLengthSquared(ent->angular_velocity))) return 0;
        }
    }
    return 1;
}

static void install_prediction_entities(const jka_player *p) {
    static const jka_player *installed_player;
    static uint64_t installed_revision;
    static int installed_numbers[MAX_GENTITIES + 1];
    static int installed_count;
    int i;
    if (installed_player == p && installed_revision == p->entities_revision) return;
    /* Reference CG_PredictPlayerState installs remote state once per replay,
     * not once per command. Clear only entries touched by the previous host. */
    for (i = 0; i < installed_count; ++i)
        memset(&cg_entities[installed_numbers[i]], 0, sizeof(cg_entities[0]));
    installed_count = 0;
    memset(cg_dueltypes, 0, sizeof(cg_dueltypes));
    for (i = 0; i < p->entity_count; ++i) {
        const jka_prediction_entity *ent = &p->entities[i];
        playerState_t *ps = &remote_players[ent->number];
        centity_t *cent = &cg_entities[ent->number];
        installed_numbers[installed_count++] = ent->number;
        memset(ps, 0, sizeof(*ps));
        cent->currentState.number = ent->number;
        cent->currentState.eType = ent->entity_type;
        cent->currentState.bolt1 = ent->bolt1;
        if (ent->entity_type != ET_PLAYER && ent->entity_type != ET_NPC) continue;
        VectorCopy(ent->origin, ps->origin);
        VectorCopy(ent->velocity, ps->velocity);
        ps->clientNum = ent->number;
        ps->legsAnim = ent->legs_anim;
        ps->torsoAnim = ent->torso_anim;
        ps->saberMove = ent->saber_move;
        cent->playerState = ps;
    }
    if (p->ps.clientNum >= 0 && p->ps.clientNum < MAX_CLIENTS)
        installed_numbers[installed_count++] = p->ps.clientNum;
    installed_player = p;
    installed_revision = p->entities_revision;
}
#endif
extern stringID_table_t animTable[MAX_ANIMATIONS+1];
const char *jka_animation_name(int index) { return GetStringForID(animTable, index); }
void jka_player_jump_level(void *player, int level) {
    ((jka_player *)player)->ps.fd.forcePowerLevel[FP_LEVITATION] = level;
}
void jka_player_knockback(void *player, const float *velocity, int duration) {
    playerState_t *ps = &((jka_player *)player)->ps;
    VectorCopy(velocity, ps->velocity);
    ps->pm_time = duration;
    ps->pm_flags |= PMF_TIME_KNOCKBACK;
}
/* OpenJK codemp/game/g_misc.c TeleportPlayer(player, origin, angles, speed)
 * for the playerState_t owned by this host. The caller supplies the full
 * view angles (setviewpos passes pitch 0 and roll 0). SetClientViewAngle
 * stores delta_angles relative to the client's usercmd angles; the Rust host
 * rebases its usercmd accumulator on the new viewangles afterwards, so a
 * zero delta is the same final state. Temp events and G_KillBox belong to
 * the game module and have no equivalent in this single-client host. */
void jka_player_teleport(void *player, const float *origin, const float *angles, int speed) {
    playerState_t *ps = &((jka_player *)player)->ps;
    VectorCopy(origin, ps->origin);
    ps->origin[2] += 1;
    AngleVectors(angles, ps->velocity, NULL, NULL);
    VectorScale(ps->velocity, speed ? speed : 400, ps->velocity);
    ps->pm_time = 160;
    ps->pm_flags |= PMF_TIME_KNOCKBACK;
    VectorClear(ps->delta_angles);
    VectorCopy(angles, ps->viewangles);
    ps->eFlags ^= EF_TELEPORT_BIT;
}
void jka_player_set_noclip(void *player, int enabled) {
    playerState_t *ps = &((jka_player *)player)->ps;
    if (enabled) {
        if (ps->pm_type == PM_NORMAL || ps->pm_type == PM_NOCLIP)
            ps->pm_type = PM_NOCLIP;
    } else if (ps->pm_type == PM_NOCLIP) {
        ps->pm_type = PM_NORMAL;
    }
}

/* Effective playerState result of OpenJK codemp/game/g_cmds.c G_Give(..., "all", ...).
 * The real game module also owns gentity_t::health; this lightweight host has
 * only playerState_t, so STAT_HEALTH is updated directly to the value that
 * ClientThink would publish back to ps after G_Give changes ent->health. */
void jka_player_give_all(void *player) {
    playerState_t *ps = &((jka_player *)player)->ps;
    int i;
    for (i = 0; i < HI_NUM_HOLDABLE; i++)
        ps->stats[STAT_HOLDABLE_ITEMS] |= (1 << i);
    ps->stats[STAT_HEALTH] = ps->stats[STAT_MAX_HEALTH];
    ps->stats[STAT_ARMOR] = ps->stats[STAT_MAX_HEALTH];
    ps->fd.forcePower = ps->fd.forcePowerMax;
    ps->stats[STAT_WEAPONS] = (1 << (LAST_USEABLE_WEAPON + 1)) - (1 << WP_NONE);
    for (i = AMMO_BLASTER; i < AMMO_MAX; i++)
        ps->ammo[i] = 999;
}

static void copy_saber_movement_info(saberInfo_t *out, const jka_saber_movement_info *info) {
    memset(out, 0, sizeof(*out));
    if (!info || !info->present)
        return;

    /* BG_MySaber treats a non-empty model as ownership. The actual hilt model
     * remains renderer-owned; the native movement host only needs an equipped
     * saberInfo_t with the gameplay fields below. */
    Q_strncpyz(out->model, DEFAULT_SABER_MODEL, sizeof(out->model));
    out->numBlades = info->num_blades;
    out->stylesLearned = info->styles_learned;
    out->stylesForbidden = info->styles_forbidden;
    out->saberFlags = info->saber_flags;
    out->moveSpeedScale = info->move_speed_scale;
    out->animSpeedScale = info->anim_speed_scale;
    out->readyAnim = info->ready_anim;
    out->drawAnim = info->draw_anim;
    out->putawayAnim = info->putaway_anim;
    out->kataMove = LS_INVALID;
    out->lungeAtkMove = LS_INVALID;
    out->jumpAtkUpMove = LS_INVALID;
    out->jumpAtkFwdMove = LS_INVALID;
    out->jumpAtkBackMove = LS_INVALID;
    out->jumpAtkRightMove = LS_INVALID;
    out->jumpAtkLeftMove = LS_INVALID;
}

int jka_player_set_saber_movement_info(void *player, int saber_num,
                                       const jka_saber_movement_info *info) {
    jka_player *p = player;
    if (!p || !info || saber_num < 0 || saber_num >= 2 ||
        info->num_blades < 1 || info->num_blades > MAX_BLADES ||
        !isfinite(info->move_speed_scale) || !isfinite(info->anim_speed_scale))
        return 0;

    p->saber_present[saber_num] = info->present ? qtrue : qfalse;
    copy_saber_movement_info(&p->saber[saber_num], info);

    /* A real server keeps an attached saber as a separate non-zero entity.
     * The lightweight host reserves the first non-client slot as the ownership
     * token; attached sabers do not need a packet entity until thrown. */
    p->ps.saberEntityNum = p->saber_present[0] ? MAX_CLIENTS : 0;
    return 1;
}

static int install_player_saber_info(jka_player *p) {
    const int client_num = p->ps.clientNum;
    clientInfo_t *ci;
    int saber_num;
    if (client_num < 0 || client_num >= MAX_CLIENTS)
        return -1;
    ci = &cgs.clientinfo[client_num];
    memset(ci, 0, sizeof(*ci));
    ci->infoValid = qtrue;
    for (saber_num = 0; saber_num < 2; ++saber_num) {
        if (p->saber_present[saber_num])
            memcpy(&ci->saber[saber_num], &p->saber[saber_num], sizeof(saberInfo_t));
    }
    return client_num;
}

static void clear_player_saber_info(int client_num) {
    if (client_num >= 0 && client_num < MAX_CLIENTS)
        memset(&cgs.clientinfo[client_num], 0, sizeof(cgs.clientinfo[client_num]));
}

/* Offline FFA host subset of WP_ForcePowerRun/Update/Regenerate in w_force.c.
 * This runs separately from Pmove: prediction must not regenerate server power. */
static void stop_offline_power(playerState_t *ps, int power, int time) {
    ps->fd.forcePowersActive &= ~(1 << power);
    if (power == FP_RAGE) ps->fd.forceRageRecoveryTime = time + 10000;
}
void jka_player_offline_force_tick(void *player, int time, int requested_power) {
    playerState_t *ps = &((jka_player *)player)->ps;
    if (ps->pm_type != PM_NORMAL) return;
    // ForceSpeed / ForceRage, stock level-three FFA setup. Sound/entity services
    // do not alter these state transitions and are outside the offline host.
    if (requested_power == FP_SPEED || requested_power == FP_RAGE) {
        int power = requested_power;
        if ((ps->fd.forcePowersActive & (1 << power)) && ps->forceAllowDeactivateTime < time) {
            stop_offline_power(ps, power, time);
        } else if (!(ps->fd.forcePowersActive & (1 << power)) && ps->stats[STAT_HEALTH] > 0 &&
                   BG_CanUseFPNow(GT_FFA, ps, time, power) &&
                   ps->fd.forcePower >= forcePowerNeeded[ps->fd.forcePowerLevel[power]][power] &&
                   (power != FP_RAGE || (ps->stats[STAT_HEALTH] >= 10 && ps->fd.forceRageRecoveryTime < time))) {
            ps->forceAllowDeactivateTime = time + 1500;
            ps->fd.forcePowersActive |= (1 << power);
            ps->fd.forcePowerDuration[power] = time + 20000;
            ps->fd.forcePowerDebounce[power] = 0;
            BG_ForcePowerDrain(ps, power, 0);
        }
    }
    for (int power = FP_SPEED; power <= FP_RAGE; ++power) {
        if (ps->fd.forcePowerDuration[power] && ps->fd.forcePowerDuration[power] < time) {
            if (ps->fd.forcePowersActive & (1 << power)) stop_offline_power(ps, power, time);
            ps->fd.forcePowerDuration[power] = 0;
        }
    }
    if ((ps->fd.forcePowersActive & (1 << FP_RAGE)) && ps->forceRageDrainTime < time) {
        ps->stats[STAT_HEALTH] -= 2;
        ps->forceRageDrainTime = time + 450;
        if (ps->stats[STAT_HEALTH] < 1) { ps->stats[STAT_HEALTH] = 1; stop_offline_power(ps, FP_RAGE, time); }
    }
    if (ps->groundEntityNum != ENTITYNUM_NONE && !ps->fd.forceJumpZStart)
        ps->fd.forcePowersActive &= ~(1 << FP_LEVITATION);
    if ((!ps->fd.forcePowersActive || ps->fd.forcePowersActive == (1 << FP_DRAIN)) && !ps->saberInFlight &&
        (ps->weapon != WP_SABER || !BG_SaberInSpecial(ps->saberMove))) {
        while (ps->fd.forcePowerRegenDebounceTime < time) {
            if (ps->fd.forcePower < ps->fd.forcePowerMax) ps->fd.forcePower++;
            ps->fd.forcePowerRegenDebounceTime += 200; // stock g_forceRegenTime
        }
    } else ps->fd.forcePowerRegenDebounceTime = time;
}

void *jka_player_new(const float *origin, float yaw, int spectator) {
    jka_player *p = calloc(1, sizeof(*p));
    if (!p) return NULL;
#ifdef JKA_JAPRO
    p->entities_revision = next_entities_revision++;
#endif
    playerState_t *ps = &p->ps;
    VectorCopy(origin, ps->origin);
    ps->viewangles[YAW] = yaw;
    ps->pm_type = spectator ? PM_SPECTATOR : PM_NORMAL;
    /* OpenJK ClientSpawn mirrors sess.sessionTeam into PERS_TEAM.  Free
     * spectators are a real TEAM_SPECTATOR playerState, not a normal player
     * whose model is merely hidden by the renderer. */
    ps->persistant[PERS_TEAM] = spectator ? TEAM_SPECTATOR : TEAM_FREE;
    ps->gravity = DEFAULT_GRAVITY;
    ps->speed = ps->basespeed = spectator ? 400 : 250;
    ps->groundEntityNum = ENTITYNUM_NONE;
    ps->stats[STAT_HEALTH] = ps->stats[STAT_MAX_HEALTH] = 100;
    ps->stats[STAT_WEAPONS] = spectator ? 0 : (1 << WP_SABER);
    /* Stock local FFA starts with a saber; ownership metadata is attached by
     * jka_player_set_saber_movement_info after the player is created. */
    ps->weapon = WP_SABER;
    ps->saberMove = LS_READY;
    ps->fd.saberAnimLevel = SS_MEDIUM;
    ps->fd.saberAnimLevelBase = SS_MEDIUM;
    ps->fd.forcePower = ps->fd.forcePowerMax = 100;
    ps->fd.forcePowersKnown = (1 << FP_LEVITATION) | (1 << FP_SABER_OFFENSE) | (1 << FP_SPEED) | (1 << FP_RAGE);
    ps->fd.forcePowerLevel[FP_LEVITATION] = FORCE_LEVEL_3;
    ps->fd.forcePowerLevel[FP_SABER_OFFENSE] = FORCE_LEVEL_3;
    ps->fd.forcePowerLevel[FP_SPEED] = FORCE_LEVEL_3;
    ps->fd.forcePowerLevel[FP_RAGE] = FORCE_LEVEL_3;
    ps->viewheight = DEFAULT_VIEWHEIGHT;
    ps->standheight = DEFAULT_MAXS_2;
    ps->crouchheight = CROUCH_MAXS_2;
    ps->legsAnim = BOTH_STAND1;
    ps->torsoAnim = BOTH_STAND1;
    VectorSet(p->move.mins, -15, -15, DEFAULT_MINS_2);
    VectorSet(p->move.maxs, 15, 15, DEFAULT_MAXS_2);
    return p;
}
void jka_player_free(void *player) { free(player); }
void *jka_player_clone(const void *player) {
    jka_player *p = malloc(sizeof(*p));
    if (p) memcpy(p, player, sizeof(*p));
#ifdef JKA_JAPRO
    if (p) p->entities_revision = next_entities_revision++;
#endif
    return p;
}
static void host_trace(trace_t *result, const vec3_t start, const vec3_t mins, const vec3_t maxs,
                       const vec3_t end, int pass, int mask) {
    jka_trace t = {0};
    trace_callback(callback_context, &t, start, mins, maxs, end, pass, mask);
    memset(result, 0, sizeof(*result));
    result->fraction = t.fraction;
    VectorCopy(t.end, result->endpos);
    VectorCopy(t.normal, result->plane.normal);
    result->plane.dist = t.distance;
    SetPlaneSignbits(&result->plane);
    result->plane.type = PlaneTypeForNormal(result->plane.normal);
    result->surfaceFlags = t.surface_flags;
    result->contents = t.contents;
    result->entityNum = t.entity;
    result->allsolid = t.all_solid;
    result->startsolid = t.start_solid;
}
static int host_contents(const vec3_t point, int pass) { return contents_callback(callback_context, point, pass); }

/* Lightweight counterpart to the generic_cmd switch in OpenJK g_active.c.
 * Pmove intentionally does not execute these: on a real server they are
 * authoritative game-side commands processed after Pmove. Keep that same
 * ordering here and add cases as the local server grows. */
static void local_apply_queued_saber_style(jka_player *p) {
    if (p->saber_cycle_queue && p->ps.weaponTime <= 0) {
        p->ps.fd.saberAnimLevel = p->saber_cycle_queue;
        p->saber_cycle_queue = 0;
    }
}

static void local_saber_attack_cycle(jka_player *p) {
    playerState_t *ps = &p->ps;
    int select_level;

    /* Cmd_SaberAttackCycle_f guards that matter to the one-client FFA host.
     * The current local authority models the stock single-saber case; dual/
     * staff saberInfo_t ownership belongs in the later full game-entity shim. */
    if (ps->stats[STAT_HEALTH] <= 0 || ps->pm_type != PM_NORMAL || ps->weapon != WP_SABER)
        return;

    select_level = p->saber_cycle_queue ? p->saber_cycle_queue : ps->fd.saberAnimLevel;
    select_level++;
    if (select_level > ps->fd.forcePowerLevel[FP_SABER_OFFENSE])
        select_level = FORCE_LEVEL_1;
    if (select_level < FORCE_LEVEL_1)
        select_level = FORCE_LEVEL_1;

    /* OpenJK queues a stance switch while a saber move is busy so chaining is
     * not reinterpreted halfway through the move. */
    ps->fd.saberAnimLevelBase = select_level;
    if (ps->weaponTime <= 0) {
        ps->fd.saberAnimLevel = select_level;
        p->saber_cycle_queue = 0;
    } else {
        p->saber_cycle_queue = select_level;
    }
}

static void local_process_generic_cmd(jka_player *p, int generic_cmd) {
    const int now = p->ps.commandTime;
    if (!generic_cmd)
        return;

    /* g_active.c: allow a changed command immediately, otherwise apply the
     * stock 300 ms repeat debounce (push/pull are the exceptions there). */
    if (generic_cmd == p->last_generic_cmd && p->last_generic_cmd_time >= now)
        return;
    p->last_generic_cmd = generic_cmd;
    if (generic_cmd != GENCMD_FORCE_THROW && generic_cmd != GENCMD_FORCE_PULL)
        p->last_generic_cmd_time = now + 300;

    switch (generic_cmd) {
    case GENCMD_SABERATTACKCYCLE:
        local_saber_attack_cycle(p);
        break;
    default:
        /* Other generic commands still require their corresponding game-side
         * authority (entities, Force targeting, holdables, taunts, etc.). */
        break;
    }
}

int jka_player_step(void *player, const jka_cmd *input, int tick, jka_trace_fn trace,
                    jka_contents_fn contents, void *context) {
    if (setjmp(error_target)) return 0;
    jka_player *p = player;
    trace_callback = trace; contents_callback = contents; callback_context = context;
    memset(&cg_entities[0], 0, sizeof(cg_entities[0]));
    cg_entities[0].playerState = &p->ps;
    p->move.ps = &p->ps;
    p->move.baseEnt = (bgEntity_t *)cg_entities;
    p->move.entSize = sizeof(cg_entities[0]);
    p->move.animations = bgHumanoidAnimations;
    p->move.tracemask = MASK_PLAYERSOLID;
    p->move.trace = host_trace;
    p->move.pointcontents = host_contents;
    p->move.pmove_fixed = 1;
    p->move.pmove_msec = tick;
    p->move.pmove_float = 0;
    p->move.stepSlideFix = 1;
    p->move.gametype = GT_FFA;
    memset(&p->move.cmd, 0, sizeof(p->move.cmd));
    p->move.cmd.serverTime = input->time;
    memcpy(p->move.cmd.angles, input->angles, sizeof(input->angles));
    p->move.cmd.buttons = input->buttons;
    p->move.cmd.weapon = input->weapon;
    p->move.cmd.forcesel = input->force_selection;
    p->move.cmd.invensel = input->inventory_selection;
    p->move.cmd.generic_cmd = input->generic_command;
    p->move.cmd.forwardmove = input->forward;
    p->move.cmd.rightmove = input->right;
    p->move.cmd.upmove = input->up;
    const int saber_client_num = install_player_saber_info(p);
    begin_pmove_view_tracking(p->move.ps);
    Pmove(&p->move);
    p->view_forced = end_pmove_view_tracking();
    clear_player_saber_info(saber_client_num);
    local_apply_queued_saber_style(p);
    local_process_generic_cmd(p, input->generic_command);
    return 1;
}
void jka_player_view(const void *player, jka_view *view) {
    const jka_player *p = player;
    const playerState_t *ps = &p->ps;
    memset(view, 0, sizeof(*view));
    VectorCopy(ps->origin, view->origin); VectorCopy(ps->velocity, view->velocity);
    VectorCopy(ps->viewangles, view->angles); VectorCopy(p->move.mins, view->mins); VectorCopy(p->move.maxs, view->maxs);
    view->time = ps->commandTime; view->pm_type = ps->pm_type; view->flags = ps->pm_flags;
    view->timer = ps->pm_time; view->ground_entity = ps->groundEntityNum; view->view_height = ps->viewheight; view->bob_cycle = ps->bobCycle;
    view->legs_anim = ps->legsAnim; view->legs_timer = ps->legsTimer;
    view->torso_anim = ps->torsoAnim; view->torso_timer = ps->torsoTimer;
    view->water_level = p->move.waterlevel; view->water_type = p->move.watertype;
    view->force_power = ps->fd.forcePower; view->force_jump_level = ps->fd.forcePowerLevel[FP_LEVITATION];
    view->event_sequence = ps->eventSequence;
    memcpy(view->events, ps->events, sizeof(view->events)); memcpy(view->event_parms, ps->eventParms, sizeof(view->event_parms));
    view->touch_count = p->move.numtouch; memcpy(view->touches, p->move.touchents, sizeof(view->touches));
    memcpy(view->delta_angles, ps->delta_angles, sizeof(view->delta_angles));
    view->health = ps->stats[STAT_HEALTH]; view->active_powers = ps->fd.forcePowersActive;
    view->rage_recovery = ps->fd.forceRageRecoveryTime;
    view->armor = ps->stats[STAT_ARMOR];
    view->max_health = ps->stats[STAT_MAX_HEALTH];
    view->force_power_max = ps->fd.forcePowerMax;
    view->weapon = ps->weapon;
    view->view_forced = p->view_forced ? 1 : 0;
    view->ammo = -1;
    if (ps->weapon >= 0 && ps->weapon < WP_NUM_WEAPONS) {
        const int ammo_index = weaponData[ps->weapon].ammoIndex;
        if (ammo_index > AMMO_NONE && ammo_index < MAX_AMMO) view->ammo = ps->ammo[ammo_index];
    }
}

void jka_player_entity_view(const void *player, jka_entity_view *view) {
    const jka_player *p = player;
    playerState_t ps = p->ps;
    entityState_t es;
    memset(&es, 0, sizeof(es));
    memset(view, 0, sizeof(*view));

    /* This is the actual OpenJK bridge used by client/server presentation.
     * Use a playerState copy because BG_PlayerStateToEntityState advances the
     * entity-event sequence; presentation must not mutate the offline host. */
    BG_PlayerStateToEntityState(&ps, &es, qfalse);

    view->number = es.number; view->e_type = es.eType; view->client_num = es.clientNum;
    VectorCopy(es.pos.trBase, view->origin); VectorCopy(es.pos.trDelta, view->velocity);
    VectorCopy(es.apos.trBase, view->angles); view->speed = es.speed; VectorCopy(es.origin2, view->origin2);
    view->trickedentindex = es.trickedentindex; view->trickedentindex2 = es.trickedentindex2;
    view->trickedentindex3 = es.trickedentindex3; view->trickedentindex4 = es.trickedentindex4;
    view->force_frame = es.forceFrame; view->emplaced_owner = es.emplacedOwner;
    view->generic_enemy_index = es.genericenemyindex; view->active_force_pass = es.activeForcePass;
    view->movement_dir = (int32_t)es.angles2[YAW]; view->legs_anim = es.legsAnim; view->torso_anim = es.torsoAnim;
    view->legs_flip = es.legsFlip; view->torso_flip = es.torsoFlip; view->e_flags = es.eFlags; view->e_flags2 = es.eFlags2;
    view->saber_in_flight = es.saberInFlight; view->saber_entity_num = es.saberEntityNum; view->saber_move = es.saberMove;
    view->force_powers_active = es.forcePowersActive; view->bolt1 = es.bolt1; view->other_entity_num2 = es.otherEntityNum2;
    view->saber_holstered = es.saberHolstered; view->event = es.event; view->event_parm = es.eventParm;
    view->weapon = es.weapon; view->ground_entity_num = es.groundEntityNum; view->powerups = es.powerups;
    view->loop_sound = es.loopSound; view->generic1 = es.generic1; view->modelindex2 = es.modelindex2;
    view->constant_light = es.constantLight; view->is_jedi_master = es.isJediMaster; view->time2 = es.time2; view->fireflag = es.fireflag;
    view->held_by_client = es.heldByClient; view->rag_attach = es.ragAttach; view->model_scale = es.iModelScale;
    view->broken_limbs = es.brokenLimbs; view->has_look_target = es.hasLookTarget; view->look_target = es.lookTarget;
    memcpy(view->custom_rgba, es.customRGBA, sizeof(view->custom_rgba)); view->vehicle_num = es.m_iVehicleNum;

    view->pm_type = p->ps.pm_type; view->view_height = p->ps.viewheight; view->health = p->ps.stats[STAT_HEALTH];
    view->dead_yaw = p->ps.stats[STAT_DEAD_YAW]; view->team = p->ps.persistant[PERS_TEAM]; view->zoom_mode = p->ps.zoomMode;
    view->emplaced_index = p->ps.emplacedIndex; view->force_hand_extend = p->ps.forceHandExtend;
    view->falling_to_death = p->ps.fallingToDeath;
}

/* BG_G2PlayerAngles bridge: execute the vendored OpenJK presentation math and
 * capture its Ghoul2 angle commands for the Rust Ghoul2 evaluator. */
static jka_player_angle_result *angle_result_target;
static mdxaBone_t angle_motion_matrix;
static qboolean angle_motion_matrix_valid;

static qboolean host_g2_set_bone_angles(void *ghoul2, int modelIndex, const char *boneName,
                                        const vec3_t angles, const int flags, const int up,
                                        const int right, const int forward, qhandle_t *modelList,
                                        int blendTime, int currentTime) {
    (void)ghoul2; (void)modelIndex; (void)modelList; (void)blendTime; (void)currentTime;
    if (!angle_result_target || angle_result_target->command_count >= JKA_MAX_BONE_ANGLE_COMMANDS) return qfalse;
    jka_bone_angle_command *cmd = &angle_result_target->commands[angle_result_target->command_count++];
    memset(cmd, 0, sizeof(*cmd));
    Q_strncpyz(cmd->bone_name, boneName, sizeof(cmd->bone_name));
    VectorCopy(angles, cmd->angles);
    cmd->flags = flags; cmd->up = up; cmd->right = right; cmd->forward = forward;
    return qtrue;
}

static qboolean host_g2_get_bolt_matrix_no_rec_no_rot(void *ghoul2, const int modelIndex,
                                                       const int boltIndex, mdxaBone_t *matrix,
                                                       const vec3_t angles, const vec3_t position,
                                                       const int frameNum, qhandle_t *modelList,
                                                       vec3_t scale) {
    (void)ghoul2; (void)modelIndex; (void)boltIndex; (void)angles; (void)position;
    (void)frameNum; (void)modelList; (void)scale;
    if (!matrix) return qfalse;
    if (angle_motion_matrix_valid) {
        memcpy(matrix, &angle_motion_matrix, sizeof(*matrix));
        return qtrue;
    }
    memset(matrix, 0, sizeof(*matrix));
    matrix->matrix[0][0] = matrix->matrix[1][1] = matrix->matrix[2][2] = 1.0f;
    return qfalse;
}

int jka_bg_g2_player_angles(const jka_player_angle_entity *input, int time,
                            const float *lerp_origin, const float *lerp_angles, int frametime,
                            const float *model_scale, int ci_legs, int ci_torso,
                            const float *look_angles, const float *motion_matrix,
                            jka_player_angle_state *state, jka_player_angle_result *result) {
    if (!input || !lerp_origin || !lerp_angles || !model_scale || !look_angles || !state || !result) return 0;
    if (setjmp(error_target)) {
        angle_result_target = NULL;
        angle_motion_matrix_valid = qfalse;
        return 0;
    }

    entityState_t cent;
    matrix3_t legs;
    vec3_t legsAngles, turAngles, lookAngles, scale;
    qboolean torsoYawing = state->torso_yawing ? qtrue : qfalse;
    qboolean torsoPitching = state->torso_pitching ? qtrue : qfalse;
    qboolean legsYawing = state->legs_yawing ? qtrue : qfalse;
    memset(&cent, 0, sizeof(cent));
    memset(result, 0, sizeof(*result));

    cent.number = input->number;
    cent.eType = input->entity_type;
    VectorCopy(input->velocity, cent.pos.trDelta);
    cent.angles2[YAW] = (float)input->movement_dir;
    cent.legsAnim = input->legs_anim;
    cent.torsoAnim = input->torso_anim;
    cent.eFlags = input->e_flags;
    cent.weapon = input->weapon;
    cent.groundEntityNum = input->ground_entity_num;
    cent.forceFrame = input->force_frame;
    cent.saberMove = input->saber_move;
    cent.m_iVehicleNum = input->vehicle_num;
    cent.heldByClient = input->held_by_client;
    cent.otherEntityNum2 = input->other_entity_num2;

    VectorCopy(look_angles, lookAngles);
    VectorCopy(model_scale, scale);
    angle_result_target = result;
    angle_motion_matrix_valid = motion_matrix ? qtrue : qfalse;
    if (motion_matrix) memcpy(&angle_motion_matrix.matrix[0][0], motion_matrix, sizeof(angle_motion_matrix.matrix));
    imports.G2API_SetBoneAngles = host_g2_set_bone_angles;
    imports.G2API_GetBoltMatrix_NoRecNoRot = host_g2_get_bolt_matrix_no_rec_no_rot;

    BG_G2PlayerAngles((void *)1, 0, &cent, time, (float *)lerp_origin, (float *)lerp_angles,
                      legs, legsAngles, &torsoYawing, &torsoPitching, &legsYawing,
                      &state->torso_yaw_angle, &state->torso_pitch_angle, &state->legs_yaw_angle,
                      frametime, turAngles, scale, ci_legs, ci_torso, &state->corr_time,
                      lookAngles, state->last_head_angles, state->look_time, NULL,
                      &state->super_smooth_time);

    state->torso_yawing = torsoYawing;
    state->torso_pitching = torsoPitching;
    state->legs_yawing = legsYawing;
    memcpy(result->legs_axis, legs, sizeof(result->legs_axis));
    VectorCopy(legsAngles, result->legs_angles);
    VectorCopy(turAngles, result->tur_angles);
    angle_result_target = NULL;
    angle_motion_matrix_valid = qfalse;
    return 1;
}

/* Read-only view of the pinned OpenJK bg_itemlist (immutable static data;
 * no host lock needed). Returns 0 for an out-of-range index. */
int jka_item_count(void) { return bg_numItems; }

int jka_item_info(int index, const char **classname, const char **world_model,
                  const char **world_model2, int *type, int *tag, int *quantity) {
    const gitem_t *item;
    if (index < 0 || index >= bg_numItems) return 0;
    item = &bg_itemlist[index];
    *classname = item->classname ? item->classname : "";
    *world_model = item->world_model[0] ? item->world_model[0] : "";
    *world_model2 = item->world_model[1] ? item->world_model[1] : "";
    *type = (int)item->giType;
    *tag = item->giTag;
    *quantity = item->quantity;
    return 1;
}

/* ---- Network prediction bridge (CG_PredictPlayerState's Pmove host) ----
 * The Rust CGame owns command selection and error decay; these entry points
 * only load a snapshot playerState into a real playerState_t, run the stock
 * PM_UpdateViewAngles / Pmove, and read the result back in wire order. */
#include <stddef.h>
#include "ps_fields.inc"

int jka_ps_field_count(void) { return JKA_PS_FIELD_COUNT; }

/* MSG_ReadDeltaPlayerstate's view of the struct: every netfield is written as
 * a 4-byte int/float at its offset; the four arrays are copied separately. */
int jka_player_set_network(void *player, const int32_t *fields, int count, const int32_t *stats,
                           const int32_t *persistant, const int32_t *ammo, const int32_t *powerups) {
    jka_player *p = player;
    int i;
    if (count != JKA_PS_FIELD_COUNT) return 0;
    memset(&p->ps, 0, sizeof(p->ps));
    for (i = 0; i < JKA_PS_FIELD_COUNT; i++)
        memcpy((byte *)&p->ps + jka_ps_field_offsets[i], &fields[i], 4);
    memcpy(p->ps.stats, stats, sizeof(int32_t) * MAX_STATS);
    memcpy(p->ps.persistant, persistant, sizeof(int32_t) * MAX_PERSISTANT);
    memcpy(p->ps.ammo, ammo, sizeof(int32_t) * 16);
    memcpy(p->ps.powerups, powerups, sizeof(int32_t) * MAX_POWERUPS);
    p->view_forced = qfalse;
#ifdef JKA_JAPRO
    p->entities_revision = next_entities_revision++;
#endif
    return 1;
}

int jka_player_get_network(const void *player, int32_t *fields, int count, int32_t *stats,
                           int32_t *persistant, int32_t *ammo, int32_t *powerups) {
    const jka_player *p = player;
    int i;
    if (count != JKA_PS_FIELD_COUNT) return 0;
    for (i = 0; i < JKA_PS_FIELD_COUNT; i++)
        memcpy(&fields[i], (const byte *)&p->ps + jka_ps_field_offsets[i], 4);
    memcpy(stats, p->ps.stats, sizeof(int32_t) * MAX_STATS);
    memcpy(persistant, p->ps.persistant, sizeof(int32_t) * MAX_PERSISTANT);
    memcpy(ammo, p->ps.ammo, sizeof(int32_t) * 16);
    memcpy(powerups, p->ps.powerups, sizeof(int32_t) * MAX_POWERUPS);
    return 1;
}

static void fill_cmd(usercmd_t *cmd, const jka_cmd *input) {
    memset(cmd, 0, sizeof(*cmd));
    cmd->serverTime = input->time;
    memcpy(cmd->angles, input->angles, sizeof(input->angles));
    cmd->buttons = input->buttons;
    cmd->weapon = input->weapon;
    cmd->forcesel = input->force_selection;
    cmd->invensel = input->inventory_selection;
    cmd->generic_cmd = input->generic_command;
    cmd->forwardmove = input->forward;
    cmd->rightmove = input->right;
    cmd->upmove = input->up;
}

/* cg_predict.c: "if ( cg_pmove.pmove_fixed ) PM_UpdateViewAngles( cg_pmove.ps, &cg_pmove.cmd );" */
void jka_player_update_view_angles(void *player, const jka_cmd *input) {
    jka_player *p = player;
    usercmd_t cmd;
    fill_cmd(&cmd, input);
#ifdef JKA_JAPRO
    p->move.ps = &p->ps;
    pm = &p->move;
#endif
    PM_UpdateViewAngles(&p->ps, &cmd);
}

/* One CG_PredictPlayerState replay step: stock Pmove (which itself chops the
 * command into PmoveSingle steps) with the server's pmove settings. */
int jka_player_predict(void *player, const jka_cmd *input, const jka_predict_settings *settings,
                       jka_trace_fn trace, jka_contents_fn contents, void *context) {
    jka_player *p = player;
    int client;
    if (setjmp(error_target)) return 0;
    trace_callback = trace; contents_callback = contents; callback_context = context;
    client = p->ps.clientNum;
#ifdef JKA_JAPRO
    if (p->ps.m_iVehicleNum) host_error(ERR_DROP, "Vehicle prediction requires a vehicle entity host");
    install_prediction_entities(p);
    cg.clientNum = client;
    prediction_snapshot.ps = p->ps;
    cg.predictedPlayerState = p->ps;
#endif
    if (client < 0 || client >= MAX_CLIENTS) host_error(ERR_DROP, "Predicted clientNum %d out of range", client);
    memset(&cg_entities[client], 0, sizeof(cg_entities[client]));
#ifdef JKA_JAPRO
    cg_entities[client].currentState.number = client;
    cg_entities[client].currentState.eType = ET_PLAYER;
#endif
    cg_entities[client].playerState = &p->ps;
    p->move.ps = &p->ps;
#ifdef JKA_JAPRO
    /* The reference sometimes writes cg.predictedPlayerState directly. Keep
     * exactly that alias while Pmove runs, including SP/surf crouch jumps. */
    cg_entities[client].playerState = &cg.predictedPlayerState;
    p->move.ps = &cg.predictedPlayerState;
#endif
    p->move.ghoul2 = NULL;
    VectorClear(p->move.modelScale);
    p->move.nonHumanoid = qfalse;
    p->move.baseEnt = (bgEntity_t *)cg_entities;
    p->move.entSize = sizeof(cg_entities[0]);
    p->move.animations = bgHumanoidAnimations;
    p->move.tracemask = settings->tracemask;
    p->move.noFootsteps = settings->no_footsteps;
    p->move.trace = host_trace;
    p->move.pointcontents = host_contents;
    p->move.pmove_fixed = settings->pmove_fixed;
    p->move.pmove_msec = settings->pmove_msec;
    p->move.pmove_float = settings->pmove_float;
    p->move.gametype = settings->gametype;
    p->move.debugMelee = settings->debug_melee;
    p->move.stepSlideFix = settings->step_slide_fix;
    p->move.noSpecMove = settings->no_spec_move;
    fill_cmd(&p->move.cmd, input);
    begin_pmove_view_tracking(p->move.ps);
    Pmove(&p->move);
    p->view_forced = end_pmove_view_tracking();
#ifdef JKA_JAPRO
    p->ps = cg.predictedPlayerState;
    p->move.ps = &p->ps;
    cg_entities[client].playerState = &p->ps;
#endif
    return 1;
}

/* weaponData[] fields CG_WeaponSelectable consults. */
int jka_weapon_info(int weapon, int *ammo_index, int *energy_per_shot, int *alt_energy_per_shot) {
    if (weapon < 0 || weapon >= WP_NUM_WEAPONS) return 0;
    *ammo_index = weaponData[weapon].ammoIndex;
    *energy_per_shot = weaponData[weapon].energyPerShot;
    *alt_energy_per_shot = weaponData[weapon].altEnergyPerShot;
    return 1;
}
