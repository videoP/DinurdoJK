/* Direct native replay oracle. No wire import/export or per-command host-step
 * entry is used in this loop. Scheduling follows pinned cg_predict.c. */
#include "jk2_host_internal.h"

int jk2_oracle_replay_impl(const void *opaque, const jka_cmd *commands, int count, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, void *context, unsigned char *states, int state_size) {
    const jk2_player *seed = opaque;
    playerState_t ps = seed->ps, clients[MAX_CLIENTS];
    pmove_t move;
    int i, n;
    if (state_size != sizeof(playerState_t) || count < 0 || count > 10000 || !trace || !contents
        || settings->pmove_msec < 8 || settings->pmove_msec > 33
        || !jk2_select_versions(seed->version, settings->gameplay_version)) return 0;
    memcpy(clients, seed->clients, sizeof(clients));
    memset(&move, 0, sizeof(move));
    move.ps = &ps; move.animations = (animation_t *)seed->animations;
    move.trace = jk2_host_trace; move.pointcontents = jk2_host_contents;
    move.tracemask = settings->tracemask; move.noFootsteps = settings->no_footsteps;
    move.gametype = settings->gametype; move.pmove_fixed = settings->pmove_fixed; move.pmove_msec = settings->pmove_msec;
    for (i = 0; i < MAX_CLIENTS; i++) move.bgClients[i] = &clients[i];
    jk2_bind_services(trace, contents, context);
    for (n = 0; n < count; n++) {
        jk2_copy_command(&move.cmd, &commands[n]);
        if (move.pmove_fixed) PM_UpdateViewAngles(&ps, &move.cmd);
        if (move.cmd.serverTime <= ps.commandTime) {
            memcpy(states + (size_t)n * state_size, &ps, state_size);
            continue;
        }
        if (move.pmove_fixed) move.cmd.serverTime = ((move.cmd.serverTime + move.pmove_msec-1)/move.pmove_msec)*move.pmove_msec;
        jk2_require_context(&ps, move.cmd.serverTime);
        Pmove(&move);
        memcpy(states + (size_t)n * state_size, &ps, state_size);
    }
    return 1;
}
