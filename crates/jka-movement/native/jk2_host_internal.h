#pragma once
#include "game/q_shared.h"
#include "game/bg_public.h"
#include "game/bg_local.h"
#include "jk2_bridge.h"

/* cgame/cg_public.h MAX_ENTITIES_IN_SNAPSHOT (that header has no include guard). */
#define JK2_SNAPSHOT_ENTITIES 256

/* A trigger-type cg_entities slot: currentState and miscTime as CGame keeps
 * them. `predicted_nodraw` (host only) marks EF_NODRAW added by CG_TouchItem. */
typedef struct jk2_centity_s {
    entityState_t currentState;
    int miscTime, predicted_nodraw;
} jk2_centity;

/* `remote` is the prediction-owned cg_entities[i].currentState for each client
 * slot, kept in native 1.04 identifiers. `send` is CGame's cgSendPS scratch. */
typedef struct jk2_player_s {
    playerState_t ps, checkpoint, send[MAX_CLIENTS];
    entityState_t remote[MAX_CLIENTS], remote_checkpoint[MAX_CLIENTS], remote_mark[MAX_CLIENTS];
    jk2_frame frame;
    int context_ready, context_ready_mark;
    /* Trigger-type slots >= MAX_CLIENTS (NULL otherwise) and the current
     * CG_BuildSolidList trigger list. Checkpoint/mark keep each listed slot's
     * mutable eFlags/miscTime/predicted_nodraw, and cg.hyperspace. */
    jk2_centity *world[MAX_GENTITIES];
    int trigger[JK2_SNAPSHOT_ENTITIES], trigger_count;
    int trigger_checkpoint[JK2_SNAPSHOT_ENTITIES][3], trigger_mark[JK2_SNAPSHOT_ENTITIES][3];
    int hyperspace, hyperspace_checkpoint, hyperspace_mark;
    pmove_t move;
    animation_t animations[MAX_TOTALANIMATIONS];
    int version;
} jk2_player;

void jk2_bind_services(jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context);
int jk2_select_versions(int version, int gameplay);
void jk2_host_trace(trace_t *, const vec3_t, const vec3_t, const vec3_t, const vec3_t, int, int);
int jk2_host_contents(const vec3_t, int);
void jk2_host_model_trace(trace_t *, const vec3_t, const vec3_t, const vec3_t, int);
void jk2_copy_command(usercmd_t *out, const jka_cmd *cmd);
int jk2_import_wire(int version, const jk2_wire *wire, playerState_t *out);
void jk2_context_pmove(pmove_t *move, entityState_t remote[MAX_CLIENTS], playerState_t send[MAX_CLIENTS],
    const jk2_frame *frame, int context_ready);
int jk2_touch_triggers(playerState_t *ps, const pmove_t *move, jk2_centity *const *list, int count,
    const jk2_settings *settings, int time);
