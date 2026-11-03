#pragma once
#include "bridge.h"

/* Host ABI only. Never an alias of either game's playerState_t / pmove_t. */
typedef struct jk2_settings_s {
    int32_t gameplay_version, pmove_fixed, pmove_msec, gametype, tracemask, no_footsteps;
    int32_t spectator_server_timing;
    int32_t predict_items; /* cg_predictItems */
} jk2_settings;

/* trap_CM_BoxTrace(result, start, start, mins, maxs, model, -1) against an
 * inline model at its BSP position (CG_TouchTriggerPrediction). */
typedef void (*jk2_model_trace_fn)(void *, jka_trace *, const float *start, const float *mins,
    const float *maxs, int32_t model);

/* CG_PredictPlayerState's per-frame saber-lock facing inputs: cg.time,
 * cg.snap->ps and cg_entities[saberLockEnemy].lerpOrigin (host coordinates). */
typedef struct jk2_frame_s {
    int32_t cg_time, lock_time, lock_enemy, has_lock_origin;
    float snap_origin[3], lock_origin[3];
} jk2_frame;

void *jk2_player_new(int version, const unsigned char *animations, int length);
void jk2_player_free(void *player);
void *jk2_player_clone(const void *player);
int jk2_player_set_network(void *player, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups);
int jk2_player_get_network(const void *player, int normalized, uint32_t *fields, int count,
    int32_t *stats, int32_t *persistant, int32_t *ammo, int32_t *powerups);
int jk2_player_predict(void *player, const jka_cmd *cmd, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context);
void jk2_player_rollback(void *player);
const char *jk2_movement_error(void);
int jk2_contract(int index);
int jk2_ps_field_count(int version);
const char *jk2_ps_field_name(int version, int index);
int jk2_entity_field_count(int version);
const char *jk2_entity_field_name(int version, int index);
int jk2_native_state(const void *player, unsigned char *out, int size);
const char *jk2_native_word_name(int byte_offset);
int jk2_oracle_replay(const void *seed, const jka_cmd *commands, int count, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context,
    unsigned char *states, int state_size);

/* One raw JK2 wire playerstate of the connection's version. */
typedef struct jk2_wire_s {
    const uint32_t *fields;
    int32_t count;
    const int32_t *stats, *persistant, *ammo, *powerups;
} jk2_wire;

/* Pinned MVSDK CGame selection (cg.weaponSelect / forceSelect / itemSelect in
 * native JK2 IDs, -1 = none). `snap` is cg.snap->ps, `predicted` is
 * cg.predictedPlayerState. Results: 0 ignored, 1 selection time updated,
 * 2 CG_Weapon_f's sv_saberswitch, 3 handled without a time update; -1 invalid input. */
int jk2_cg_weapon_selectable(int version, int gameplay, const jk2_wire *predicted, int weapon);
int jk2_cg_cycle_weapon(int version, int gameplay, const jk2_wire *snap, const jk2_wire *predicted, int32_t *select, int forward);
int jk2_cg_weapon_slot(int version, int gameplay, const jk2_wire *snap, const jk2_wire *predicted, int32_t *select, int slot);
int jk2_cg_out_of_ammo(int version, int gameplay, const jk2_wire *predicted, int32_t *select, int old_weapon, int autoswitch);
int jk2_cg_cycle_force(int version, int gameplay, const jk2_wire *snap, int32_t *select, int forward);
int jk2_cg_cycle_inventory(int version, int gameplay, const jk2_wire *snap, int32_t *select, int forward);

int jk2_item_info(int index, const char **strings, int *type, int *tag, int *quantity);
int jk2_legalize_force_powers(int version, int gameplay, char *powers, int size,
    int max_rank, int free_saber, int team_force, int gametype, int fp_disabled);

/* Prediction-owned copy of cg_entities[0..MAX_CLIENTS).currentState. */
void jk2_context_reset(void *player);
int jk2_context_set_self(void *player, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups);
int jk2_context_set_entity(void *player, int number, const uint32_t *fields, int count);
int jk2_context_get_entity(const void *player, int number, uint32_t *fields, int count);
void jk2_context_set_frame(void *player, const jk2_frame *frame);
void jk2_context_mark(void *player);
void jk2_context_restore_mark(void *player);

/* Prediction-owned cg_entities[MAX_CLIENTS..).currentState for the trigger
 * types CG_BuildSolidList collects (ET_ITEM, ET_PUSH_TRIGGER,
 * ET_TELEPORT_TRIGGER). set_world_entity is CG_TransitionEntity of one wire
 * entity of such a type; clear_world_entities forgets slots whose new
 * currentState is any other type. `svflags` is cgs.mvsdk_svFlags. */
int jk2_context_set_world_entity(void *player, int number, const uint32_t *fields, int count, int svflags);
void jk2_context_clear_world_entities(void *player, const int32_t *numbers, int count);
/* CG_BuildSolidList's trigger list: the selected snapshot's entity numbers in
 * order; slots without trigger-type currentState are skipped. */
void jk2_context_set_triggers(void *player, const int32_t *numbers, int count);
/* cg.hyperspace of the last committed frame; EF_NODRAW added by predicted
 * item pickups to the slot's currentState (until its next transition). */
int jk2_context_hyperspace(const void *player);
int jk2_context_predicted_nodraw(const void *player, int32_t *numbers, int capacity);
