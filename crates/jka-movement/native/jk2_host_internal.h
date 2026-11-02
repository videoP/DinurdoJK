#pragma once
#include "game/q_shared.h"
#include "game/bg_public.h"
#include "game/bg_local.h"
#include "jk2_bridge.h"

typedef struct jk2_player_s {
    playerState_t ps, checkpoint, clients[MAX_CLIENTS];
    pmove_t move;
    animation_t animations[MAX_TOTALANIMATIONS];
    int version;
} jk2_player;

void jk2_bind_services(jka_trace_fn trace, jka_contents_fn contents, void *context);
int jk2_select_versions(int version, int gameplay);
void jk2_host_trace(trace_t *, const vec3_t, const vec3_t, const vec3_t, const vec3_t, int, int);
int jk2_host_contents(const vec3_t, int);
void jk2_copy_command(usercmd_t *out, const jka_cmd *cmd);
void jk2_require_context(const playerState_t *ps, int time);
