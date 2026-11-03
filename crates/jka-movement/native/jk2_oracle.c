/* Direct native replay oracle. No wire import/export or per-command host-step
 * entry is used in this loop. Scheduling follows pinned cg_predict.c. */
#include "jk2_host_internal.h"

int jk2_oracle_replay_impl(const void *opaque, const jka_cmd *commands, int count, const jk2_settings *settings,
    jka_trace_fn trace, jka_contents_fn contents, jk2_model_trace_fn model_trace, void *context,
    unsigned char *states, int state_size) {
    const jk2_player *seed = opaque;
    static playerState_t send[MAX_CLIENTS]; /* caller holds the shared native lock */
    static entityState_t remote[MAX_CLIENTS];
    static jk2_centity triggers[JK2_SNAPSHOT_ENTITIES];
    jk2_centity *list[JK2_SNAPSHOT_ENTITIES];
    playerState_t ps = seed->ps;
    pmove_t move;
    int n;
    if (state_size != sizeof(playerState_t) || count < 0 || count > 10000 || !trace || !contents || !model_trace
        || settings->pmove_msec < 8 || settings->pmove_msec > 33
        || !jk2_select_versions(seed->version, settings->gameplay_version)) return 0;
    memcpy(remote, seed->remote, sizeof(remote));
    for (n = 0; n < seed->trigger_count; n++) { triggers[n] = *seed->world[seed->trigger[n]]; list[n] = &triggers[n]; }
    memset(&move, 0, sizeof(move));
    move.ps = &ps; move.animations = (animation_t *)seed->animations;
    move.trace = jk2_host_trace; move.pointcontents = jk2_host_contents;
    move.tracemask = settings->tracemask; move.noFootsteps = settings->no_footsteps;
    if (settings->spectator_server_timing && ps.pm_type != PM_SPECTATOR) return 0;
    move.gametype = settings->gametype;
    move.pmove_fixed = settings->spectator_server_timing ? 0 : settings->pmove_fixed;
    move.pmove_msec = settings->pmove_msec;
    jk2_bind_services(trace, contents, model_trace, context);
    for (n = 0; n < count; n++) {
        jk2_copy_command(&move.cmd, &commands[n]);
        if (settings->pmove_fixed) PM_UpdateViewAngles(&ps, &move.cmd);
        if (move.cmd.serverTime <= ps.commandTime) {
            memcpy(states + (size_t)n * state_size, &ps, state_size);
            continue;
        }
        if (settings->pmove_fixed) move.cmd.serverTime = ((move.cmd.serverTime + move.pmove_msec-1)/move.pmove_msec)*move.pmove_msec;
        jk2_context_pmove(&move, remote, send, &seed->frame, seed->context_ready);
        jk2_touch_triggers(&ps, &move, list, seed->trigger_count, settings, seed->frame.cg_time);
        memcpy(states + (size_t)n * state_size, &ps, state_size);
    }
    return 1;
}
