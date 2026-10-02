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

/* trap_SnapVector is the one engine-owned step in PmoveSingle that changes physics: it rounds
 * velocity to integers after every step. The retail game code is identical to OpenJK's here, but
 * retail engines on different platforms do not all round the way OpenJK's Sys_SnapVector does,
 * and at 1 ms steps (1000 "fps") the rounding rule dominates friction and gravity. The mode is
 * chosen by the host (jka_set_snap_mode, defined once in mod_dispatch.c):
 *   0 OpenJK Sys_SnapVector (nearest, ties away from zero)   1 truncate toward zero
 *   2 floor                                                  3 nearest, ties to even
 *   4 no snapping (float velocity) */
extern int jka_snap_mode;
static void host_snap_vector(float *v) {
    int i;
    switch (jka_snap_mode) {
    case 1: for (i = 0; i < 3; ++i) v[i] = truncf(v[i]); break;
    case 2: for (i = 0; i < 3; ++i) v[i] = floorf(v[i]); break;
    case 3: for (i = 0; i < 3; ++i) v[i] = nearbyintf(v[i]); break;
    case 4: break;
    default: Sys_SnapVector(v); break;
    }
}

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
    imports.SnapVector = host_snap_vector;
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
    /* Last single-saber stance (sess.saberLevel): restored when the loadout
     * changes from dual/staff back to a single saber. */
    int saber_single_level;
    /* Per-player saberInfo_t state. Keep this on the player object rather than
     * permanently in global cgs.clientinfo so local and remote prediction
     * cannot contaminate each other through the _CGAME OpenJK host. */
    saberInfo_t saber[2];
    qboolean saber_present[2];
    /* Host-only presentation metadata. This deliberately does not live in
     * playerState_t because OpenJK does not network this fact. */
    qboolean view_forced;
    /* *l_leg_foot / *r_leg_foot bolt positions in Ghoul2 model space, taken from
     * the presenter's current pose. Stands in for pmove_t::ghoul2 so
     * PM_AdjustStandAnimForSlope (leg dangle) runs during prediction. */
    qboolean foot_valid;
    float foot_bolt[2][3];
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
    out->bladeStyle2Start = info->blade_style2_start;
    out->singleBladeStyle = (saber_styles_t)info->single_blade_style;
    if (info->no_manual_deactivate)
        out->saberFlags2 |= SFL2_NO_MANUAL_DEACTIVATE;
    if (info->no_manual_deactivate2)
        out->saberFlags2 |= SFL2_NO_MANUAL_DEACTIVATE2;
    out->kataMove = LS_INVALID;
    out->lungeAtkMove = LS_INVALID;
    out->jumpAtkUpMove = LS_INVALID;
    out->jumpAtkFwdMove = LS_INVALID;
    out->jumpAtkBackMove = LS_INVALID;
    out->jumpAtkRightMove = LS_INVALID;
    out->jumpAtkLeftMove = LS_INVALID;
}

/* --- Local-authority saber stance rules ------------------------------------
 * The local server has no gentity/gclient, so the game-side pieces that decide
 * fd.saberAnimLevel / saberHolstered are ported here against the same
 * saberInfo_t data Pmove sees. Sources: bg_saberLoad.c (WP_UseFirstValidSaberStyle,
 * WP_SaberStyleValidForSaber, WP_SaberCanTurnOffSomeBlades), g_cmds.c
 * (Cmd_SaberAttackCycle_f), g_active.c (ClientThink_real stance block) and
 * w_saber.c (WP_SaberPositionUpdate queue handling). The toggle sounds
 * (G_Sound soundOn/soundOff) need the local server's sound-event path. */
static qboolean local_saber_can_turn_off_some_blades(const saberInfo_t *saber) {
    if (saber->bladeStyle2Start > 0 && saber->numBlades > saber->bladeStyle2Start) {
        if ((saber->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE) &&
            (saber->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE2))
            return qfalse;
    } else if (saber->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE) {
        return qfalse;
    }
    return qtrue;
}

static void local_saber_active(const saberInfo_t *saber1, const saberInfo_t *saber2,
                               int holstered, qboolean *saber1_active, qboolean *saber2_active) {
    *saber1_active = *saber2_active = qfalse;
    if (saber2->model[0]) { /* dual */
        if (holstered > 1) {
            *saber1_active = *saber2_active = qfalse;
        } else if (holstered > 0) {
            *saber1_active = qtrue;
        } else {
            *saber1_active = *saber2_active = qtrue;
        }
    } else if (!saber1->model[0]) {
        *saber1_active = qfalse;
    } else if (saber1->numBlades > 1) { /* staff */
        *saber1_active = holstered > 1 ? qfalse : qtrue;
    } else { /* single */
        *saber1_active = holstered ? qfalse : qtrue;
    }
}

static qboolean local_saber_style_valid(const saberInfo_t *saber1, const saberInfo_t *saber2,
                                        int holstered, int level) {
    qboolean saber1_active, saber2_active;
    const qboolean dual = saber2->model[0] ? qtrue : qfalse;
    local_saber_active(saber1, saber2, holstered, &saber1_active, &saber2_active);

    if (saber1_active && saber1->model[0] && saber1->stylesForbidden &&
        (saber1->stylesForbidden & (1 << level)))
        return qfalse;
    if (dual && saber2_active) {
        if (saber2->stylesForbidden && (saber2->stylesForbidden & (1 << level)))
            return qfalse;
        /* Two sabers: only dual, or tavion when both learned it, are allowed. */
        if (level != SS_DUAL) {
            if (level != SS_TAVION)
                return qfalse;
            if (!(saber1_active && (saber1->stylesLearned & (1 << SS_TAVION))) ||
                !(saber2->stylesLearned & (1 << SS_TAVION)))
                return qfalse;
        }
    }
    return qtrue;
}

static void local_saber_use_first_valid_style(const saberInfo_t *saber1, const saberInfo_t *saber2,
                                              int holstered, int *level) {
    qboolean saber1_active, saber2_active, style_invalid = qfalse;
    const qboolean dual = saber2->model[0] ? qtrue : qfalse;
    int valid = (1 << SS_NUM_SABER_STYLES) - 2; /* mask off 1 << SS_NONE */
    int style;
    local_saber_active(saber1, saber2, holstered, &saber1_active, &saber2_active);

    if (saber1_active && saber1->model[0] && saber1->stylesForbidden &&
        (saber1->stylesForbidden & (1 << *level))) {
        style_invalid = qtrue;
        valid &= ~saber1->stylesForbidden;
    }
    if (dual && saber2_active && saber2->stylesForbidden &&
        (saber2->stylesForbidden & (1 << *level))) {
        style_invalid = qtrue;
        valid &= ~saber2->stylesForbidden;
    }
    if (!valid || !style_invalid)
        return;
    for (style = SS_FAST; style < SS_NUM_SABER_STYLES; ++style) {
        if (valid & (1 << style)) {
            *level = style;
            return;
        }
    }
}

/* Cmd_SaberAttackCycle_f. Blade on/off toggling and stance changes are applied
 * immediately when idle; a swing in progress queues the new stance instead so
 * chaining is not reinterpreted halfway through the move. */
static void local_set_or_queue_saber_style(jka_player *p, int level) {
    if (p->ps.weaponTime <= 0)
        p->ps.fd.saberAnimLevel = level;
    else
        p->saber_cycle_queue = level;
}

static void local_saber_attack_cycle(jka_player *p) {
    playerState_t *ps = &p->ps;
    saberInfo_t *saber1 = &p->saber[0];
    saberInfo_t *saber2 = &p->saber[1];
    int select_level = 0;

    if (ps->stats[STAT_HEALTH] <= 0 || ps->pm_type != PM_NORMAL || ps->weapon != WP_SABER)
        return;

    if (saber1->model[0] && saber2->model[0]) { /* no style cycling for akimbo */
        if (local_saber_can_turn_off_some_blades(saber2)) {
            if (ps->saberHolstered == 1) { /* unholster the second saber */
                ps->saberHolstered = 0;
                ps->fd.saberAnimLevel = SS_DUAL;
            } else if (ps->saberHolstered == 0) {
                if ((saber2->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE) ||
                    (saber2->bladeStyle2Start > 0 &&
                     (saber2->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE2))) {
                    /* can't turn it off manually */
                } else {
                    ps->saberHolstered = 1;
                    ps->fd.saberAnimLevel = SS_FAST;
                }
            }
            return;
        }
    } else if (saber1->numBlades > 1 && local_saber_can_turn_off_some_blades(saber1)) {
        /* staff: toggle the second blade set */
        if (ps->saberHolstered == 1) {
            if (ps->saberInFlight) /* can't relight it while it's in the air */
                return;
            ps->saberHolstered = 0;
            if (saber1->stylesForbidden) { /* have a style we have to use */
                local_saber_use_first_valid_style(saber1, saber2, ps->saberHolstered, &select_level);
                local_set_or_queue_saber_style(p, select_level);
            }
        } else if (ps->saberHolstered == 0) {
            if ((saber1->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE) ||
                (saber1->bladeStyle2Start > 0 &&
                 (saber1->saberFlags2 & SFL2_NO_MANUAL_DEACTIVATE2))) {
                /* can't turn it off manually */
            } else {
                ps->saberHolstered = 1;
                if (saber1->singleBladeStyle != SS_NONE)
                    local_set_or_queue_saber_style(p, saber1->singleBladeStyle);
            }
        }
        return;
    }

    select_level = p->saber_cycle_queue ? p->saber_cycle_queue : ps->fd.saberAnimLevel;
    select_level++;
    if (select_level > ps->fd.forcePowerLevel[FP_SABER_OFFENSE])
        select_level = FORCE_LEVEL_1;
    local_saber_use_first_valid_style(saber1, saber2, ps->saberHolstered, &select_level);

    if (ps->weaponTime <= 0) {
        ps->fd.saberAnimLevelBase = ps->fd.saberAnimLevel = select_level;
    } else {
        ps->fd.saberAnimLevelBase = p->saber_cycle_queue = select_level;
    }
    if (select_level >= SS_FAST && select_level <= SS_STRONG)
        p->saber_single_level = select_level;
}

/* WP_SaberPositionUpdate: the HUD-facing draw level tracks the queued style
 * immediately, even while a swing keeps fd.saberAnimLevel deferred. The queue
 * is applied as soon as the player is no longer busy. */
static void local_apply_queued_saber_style(jka_player *p) {
    p->ps.fd.saberDrawAnimLevel = p->saber_cycle_queue ? p->saber_cycle_queue : p->ps.fd.saberAnimLevel;
    if (p->saber_cycle_queue && (p->ps.weaponTime <= 0 || p->ps.stats[STAT_HEALTH] < 1)) {
        p->ps.fd.saberAnimLevel = p->saber_cycle_queue;
        p->saber_cycle_queue = 0;
    }
}

/* ClientThink_real: keep saberAnimLevel/Base coherent with dual and staff
 * loadouts and with which blades are currently lit. Runs before Pmove, exactly
 * where the server does, so the stance Pmove reads is already settled. */
static void local_client_think_saber_style(jka_player *p) {
    playerState_t *ps = &p->ps;
    const saberInfo_t *saber1 = &p->saber[0];
    const saberInfo_t *saber2 = &p->saber[1];

    if (!saber1->model[0])
        return;

    if (saber2->model[0]) { /* with two sabers always use akimbo style */
        if (ps->saberHolstered == 1) {
            ps->fd.saberAnimLevelBase = SS_DUAL;
            ps->fd.saberAnimLevel = SS_FAST;
        } else if (!local_saber_style_valid(saber1, saber2, ps->saberHolstered, ps->fd.saberAnimLevel)) {
            ps->fd.saberAnimLevelBase = ps->fd.saberAnimLevel = SS_DUAL;
        }
        ps->fd.saberDrawAnimLevel = ps->fd.saberAnimLevel;
    } else {
        if (saber1->stylesLearned == (1 << SS_STAFF)) /* then *always* use the staff style */
            ps->fd.saberAnimLevelBase = SS_STAFF;
        if (ps->fd.saberAnimLevelBase == SS_STAFF) {
            if (ps->saberHolstered == 1 && saber1->singleBladeStyle != SS_NONE)
                ps->fd.saberAnimLevel = saber1->singleBladeStyle;
            else
                ps->fd.saberAnimLevel = SS_STAFF;
            ps->fd.saberDrawAnimLevel = ps->fd.saberAnimLevel;
        }
    }
    if (ps->fd.saberAnimLevel >= SS_FAST && ps->fd.saberAnimLevel <= SS_STRONG)
        p->saber_single_level = ps->fd.saberAnimLevel;
}

/* ClientSpawn / ClientUserinfoChanged "changedSaber": pick the stance that
 * belongs to a newly equipped loadout and make sure it is valid for it. */
static void local_saber_loadout_changed(jka_player *p) {
    playerState_t *ps = &p->ps;
    const saberInfo_t *saber1 = &p->saber[0];
    const saberInfo_t *saber2 = &p->saber[1];
    int level;

    if (!saber1->model[0])
        return;
    if (ps->fd.saberAnimLevel >= SS_FAST && ps->fd.saberAnimLevel <= SS_STRONG)
        p->saber_single_level = ps->fd.saberAnimLevel;
    if (ps->saberHolstered == 1) /* the old loadout's half-holstered state means nothing now */
        ps->saberHolstered = 0;
    p->saber_cycle_queue = 0;

    if (saber2->model[0]) { /* dual */
        ps->fd.saberAnimLevelBase = ps->fd.saberAnimLevel = ps->fd.saberDrawAnimLevel = SS_DUAL;
    } else if (saber1->saberFlags & SFL_TWO_HANDED) { /* staff */
        ps->fd.saberAnimLevel = ps->fd.saberDrawAnimLevel = SS_STAFF;
    } else {
        level = p->saber_single_level;
        if (level < SS_FAST) level = SS_FAST;
        if (level > SS_STRONG) level = SS_STRONG;
        if (level > ps->fd.forcePowerLevel[FP_SABER_OFFENSE])
            level = ps->fd.forcePowerLevel[FP_SABER_OFFENSE];
        ps->fd.saberAnimLevelBase = ps->fd.saberAnimLevel = ps->fd.saberDrawAnimLevel = level;
    }
    if (!local_saber_style_valid(saber1, saber2, ps->saberHolstered, ps->fd.saberAnimLevel)) {
        level = ps->fd.saberAnimLevel;
        local_saber_use_first_valid_style(saber1, saber2, ps->saberHolstered, &level);
        ps->fd.saberAnimLevelBase = ps->fd.saberAnimLevel = ps->fd.saberDrawAnimLevel = level;
    }
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
    if (info->owns_entity_slot)
        p->ps.saberEntityNum = p->saber_present[0] ? MAX_CLIENTS : 0;
    /* The local authority owns the stance, so a new loadout picks a fitting
     * one (slot 1 is installed last). Network prediction keeps the snapshot's. */
    if (info->owns_entity_slot && saber_num == 1)
        local_saber_loadout_changed(p);
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

/* pmove_t::ghoul2 stand-in: the only G2API_GetBoltMatrix caller in Pmove is
 * PM_FootSlopeTrace (bolt 0 = left foot, 1 = right), and it discards the bolt's
 * z. Place the model-space foot exactly as G2 would for angles = {0, yaw, 0}. */
static const jka_player *foot_bolt_player;
static qboolean host_g2_get_foot_bolt_matrix(void *ghoul2, const int modelIndex, const int boltIndex,
                                              mdxaBone_t *matrix, const vec3_t angles, const vec3_t position,
                                              const int frameNum, qhandle_t *modelList, vec3_t scale) {
    const jka_player *p = foot_bolt_player;
    (void)ghoul2; (void)modelIndex; (void)frameNum; (void)modelList; (void)scale;
    memset(matrix, 0, sizeof(*matrix));
    matrix->matrix[0][0] = matrix->matrix[1][1] = matrix->matrix[2][2] = 1.0f;
    if (!p || !p->foot_valid || boltIndex < 0 || boltIndex > 1) return qfalse;
    {
        const float yaw = DEG2RAD(angles[YAW]), s = sinf(yaw), c = cosf(yaw);
        const float *bolt = p->foot_bolt[boltIndex];
        matrix->matrix[0][3] = position[0] + c * bolt[0] - s * bolt[1];
        matrix->matrix[1][3] = position[1] + s * bolt[0] + c * bolt[1];
        matrix->matrix[2][3] = position[2] + bolt[2];
    }
    return qtrue;
}

/* Null bolts clear it (spectators, follow cam, no model yet). */
int jka_player_set_foot_bolts(void *player, const float *left, const float *right) {
    jka_player *p = player;
    int i;
    if (!p) return 0;
    if (!left || !right) { p->foot_valid = qfalse; return 1; }
    for (i = 0; i < 3; i++)
        if (!isfinite(left[i]) || !isfinite(right[i])) return 0;
    VectorCopy(left, p->foot_bolt[0]);
    VectorCopy(right, p->foot_bolt[1]);
    p->foot_valid = qtrue;
    return 1;
}

/* Call around Pmove: the bolt host reads the player through a file-local pointer. */
static void begin_foot_bolts(jka_player *p) {
    p->move.g2Bolts_LFoot = 0;
    p->move.g2Bolts_RFoot = 1;
    p->move.ghoul2 = p->foot_valid && p->ps.persistant[PERS_TEAM] != TEAM_SPECTATOR ? (void *)p : NULL;
    foot_bolt_player = p;
    imports.G2API_GetBoltMatrix = host_g2_get_foot_bolt_matrix;
}
static void end_foot_bolts(void) { foot_bolt_player = NULL; }

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
    ps->fd.saberDrawAnimLevel = SS_MEDIUM;
    p->saber_single_level = SS_MEDIUM;
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
 * ordering here and add cases as the local server grows. The saber stance
 * rules they drive (holster/unholster, dual, staff) live with the other
 * saberInfo_t helpers above jka_player_set_saber_movement_info. */
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
        /* Show the result now rather than after the next WP_SaberPositionUpdate. */
        p->ps.fd.saberDrawAnimLevel = p->saber_cycle_queue ? p->saber_cycle_queue : p->ps.fd.saberAnimLevel;
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
    local_client_think_saber_style(p);
    const int saber_client_num = install_player_saber_info(p);
    begin_foot_bolts(p);
    begin_pmove_view_tracking(p->move.ps);
    Pmove(&p->move);
    p->view_forced = end_pmove_view_tracking();
    end_foot_bolts();
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

/* `bg_itemlist[index].pickup_sound` (CG_EntityEvent EV_ITEM_PICKUP); "" if none. */
const char *jka_item_pickup_sound(int index) {
    if (index < 0 || index >= bg_numItems) return "";
    return bg_itemlist[index].pickup_sound ? bg_itemlist[index].pickup_sound : "";
}

/* `bg_itemlist[index].icon` (CG_DrawPickupItem); "" if none. */
const char *jka_item_icon(int index) {
    if (index < 0 || index >= bg_numItems) return "";
    return bg_itemlist[index].icon ? bg_itemlist[index].icon : "";
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
    /* cg_predict.c: slopeRecalcTime is "the only value we want to maintain
     * separately on server/client" - reloading the snapshot must not reset it. */
    const int slope_recalc_time = p->ps.slopeRecalcTime;
    if (count != JKA_PS_FIELD_COUNT) return 0;
    memset(&p->ps, 0, sizeof(p->ps));
    for (i = 0; i < JKA_PS_FIELD_COUNT; i++)
        memcpy((byte *)&p->ps + jka_ps_field_offsets[i], &fields[i], 4);
    memcpy(p->ps.stats, stats, sizeof(int32_t) * MAX_STATS);
    memcpy(p->ps.persistant, persistant, sizeof(int32_t) * MAX_PERSISTANT);
    memcpy(p->ps.ammo, ammo, sizeof(int32_t) * 16);
    memcpy(p->ps.powerups, powerups, sizeof(int32_t) * MAX_POWERUPS);
    p->ps.slopeRecalcTime = slope_recalc_time;
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
    /* BG_MySaber reads cgs.clientinfo[].saber[] in the real cgame. Install the
     * predicted client's equipped sabers for exactly this Pmove. */
    const int saber_client_num = install_player_saber_info(p);
    /* cg_predict.c CG_PredictPlayerState ("THIS is pretty much bad, but..."):
     * fd.saberAnimLevelBase is not networked, so the client re-derives it before
     * every Pmove. Without it BG_SabersOff/PM_InSecondaryStyle treat a half-lit
     * staff or a single-mode dual saber (saberHolstered == 1) as "sabers off",
     * so prediction re-ignites the blades and picks the wrong swings, and skips
     * the BOTH_SABERPULL guiding pose while a thrown saber is out. */
    p->move.ps->fd.saberAnimLevelBase = p->move.ps->fd.saberAnimLevel;
    if (p->move.ps->saberHolstered == 1 && saber_client_num >= 0) {
        const clientInfo_t *ci = &cgs.clientinfo[saber_client_num];
        if (ci->saber[0].numBlades > 0)
            p->move.ps->fd.saberAnimLevelBase = SS_STAFF;
        else if (ci->saber[1].model[0])
            p->move.ps->fd.saberAnimLevelBase = SS_DUAL;
    }
    begin_foot_bolts(p);
    begin_pmove_view_tracking(p->move.ps);
    Pmove(&p->move);
    p->view_forced = end_pmove_view_tracking();
    end_foot_bolts();
    clear_player_saber_info(saber_client_num);
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
