#pragma once
#include "bridge.h"

/* Host ABI only. Never an alias of either game's playerState_t / pmove_t. */
typedef struct jk2_settings_s {
    int32_t gameplay_version, pmove_fixed, pmove_msec, gametype, tracemask, no_footsteps;
} jk2_settings;

void *jk2_player_new(int version, const unsigned char *animations, int length);
void jk2_player_free(void *player);
void *jk2_player_clone(const void *player);
int jk2_player_set_network(void *player, const uint32_t *fields, int count,
    const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups);
int jk2_player_get_network(const void *player, int normalized, uint32_t *fields, int count,
    int32_t *stats, int32_t *persistant, int32_t *ammo, int32_t *powerups);
int jk2_player_predict(void *player, const jka_cmd *cmd, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, void *context);
void jk2_player_rollback(void *player);
const char *jk2_movement_error(void);
int jk2_contract(int index);
int jk2_ps_field_count(int version);
const char *jk2_ps_field_name(int version, int index);
int jk2_native_state(const void *player, unsigned char *out, int size);
const char *jk2_native_word_name(int byte_offset);
int jk2_oracle_replay(const void *seed, const jka_cmd *commands, int count, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, void *context, unsigned char *states, int state_size);
