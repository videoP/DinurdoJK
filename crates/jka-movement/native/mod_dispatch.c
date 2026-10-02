/* Native backend ownership. No physics or playerState layout crosses this boundary. */
#include "bridge.h"
#include <stdlib.h>
#include <string.h>
typedef struct mod_player_s {
    void *base, *japro;
    jka_predict_settings settings;
} mod_player;
static int last_backend;
/* See host_snap_vector in movement.c. One global shared by both backends. */
int jka_snap_mode = 0;
void jka_set_snap_mode(int mode) { jka_snap_mode = mode; }
extern int japro_set_entities(void *, const jka_prediction_entity *, int);
extern int japro_clip_entity(const void *, const jka_prediction_entity *);
int jka_player_set_entities(void *player, const jka_prediction_entity *entities, int count) {
    mod_player *p = player;
    return japro_set_entities(p->japro, entities, count);
}
int jka_player_clip_entity(const void *player, const jka_prediction_entity *entity) {
    const mod_player *p = player;
    return p->settings.server_mod == 1 ? japro_clip_entity(p->japro, entity) : 1;
}
extern void stock_jka_player_jump_level(void *player, int level);
extern void japro_jka_player_jump_level(void *player, int level);
extern void stock_jka_player_knockback(void *player, const float *velocity, int duration);
extern void japro_jka_player_knockback(void *player, const float *velocity, int duration);
extern void stock_jka_player_set_noclip(void *player, int enabled);
extern void japro_jka_player_set_noclip(void *player, int enabled);
extern void stock_jka_player_teleport(void *player, const float *origin, const float *angles, int speed);
extern void japro_jka_player_teleport(void *player, const float *origin, const float *angles, int speed);
extern void stock_jka_player_give_all(void *player);
extern void japro_jka_player_give_all(void *player);
extern int stock_jka_player_set_saber_movement_info(void *player, int saber_num, const jka_saber_movement_info *info);
extern int japro_jka_player_set_saber_movement_info(void *player, int saber_num, const jka_saber_movement_info *info);
extern int stock_jka_player_set_foot_bolts(void *player, const float *left, const float *right);
extern int japro_jka_player_set_foot_bolts(void *player, const float *left, const float *right);
extern void stock_jka_player_offline_force_tick(void *player, int time, int requested_power);
extern void japro_jka_player_offline_force_tick(void *player, int time, int requested_power);
extern void * stock_jka_player_new(const float *origin, float yaw, int spectator);
extern void * japro_jka_player_new(const float *origin, float yaw, int spectator);
extern void stock_jka_player_free(void *player);
extern void japro_jka_player_free(void *player);
extern void * stock_jka_player_clone(const void *player);
extern void * japro_jka_player_clone(const void *player);
extern int stock_jka_player_step(void *player, const jka_cmd *input, int tick, jka_trace_fn trace, jka_contents_fn contents, void *context);
extern int japro_jka_player_step(void *player, const jka_cmd *input, int tick, jka_trace_fn trace, jka_contents_fn contents, void *context);
extern void stock_jka_player_view(const void *player, jka_view *view);
extern void japro_jka_player_view(const void *player, jka_view *view);
extern void stock_jka_player_entity_view(const void *player, jka_entity_view *view);
extern void japro_jka_player_entity_view(const void *player, jka_entity_view *view);
extern int stock_jka_player_set_network(void *player, const int32_t *fields, int count, const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups);
extern int japro_jka_player_set_network(void *player, const int32_t *fields, int count, const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups);
extern int stock_jka_player_get_network(const void *player, int32_t *fields, int count, int32_t *stats, int32_t *persistant, int32_t *ammo, int32_t *powerups);
extern int japro_jka_player_get_network(const void *player, int32_t *fields, int count, int32_t *stats, int32_t *persistant, int32_t *ammo, int32_t *powerups);
extern void stock_jka_player_update_view_angles(void *player, const jka_cmd *input);
extern void japro_jka_player_update_view_angles(void *player, const jka_cmd *input);
extern int stock_jka_player_predict(void *player, const jka_cmd *input, const jka_predict_settings *settings, jka_trace_fn trace, jka_contents_fn contents, void *context);
extern int japro_jka_player_predict(void *player, const jka_cmd *input, const jka_predict_settings *settings, jka_trace_fn trace, jka_contents_fn contents, void *context);
extern int jka_ps_field_count(void);
extern void japro_configure(const jka_predict_settings *settings);
extern int stock_jka_load_animations(const unsigned char *, int);
extern int japro_jka_load_animations(const unsigned char *, int);
extern const char *stock_jka_movement_error(void);
extern const char *japro_jka_movement_error(void);
static int is_japro(const mod_player *p) { return p->settings.server_mod == 1; }
int jka_load_animations(const unsigned char *data, int length) {
    last_backend = 0;
    if (!stock_jka_load_animations(data, length)) return 0;
    last_backend = 1;
    return japro_jka_load_animations(data, length);
}
const char *jka_movement_error(void) {
    return last_backend ? japro_jka_movement_error() : stock_jka_movement_error();
}
void *jka_player_new(const float *origin, float yaw, int spectator) {
    mod_player *p = calloc(1, sizeof(*p));
    if (!p) return NULL;
    p->base = stock_jka_player_new(origin, yaw, spectator);
    p->japro = japro_jka_player_new(origin, yaw, spectator);
    if (!p->base || !p->japro) {
        stock_jka_player_free(p->base); japro_jka_player_free(p->japro); free(p); return NULL;
    }
    return p;
}
void jka_player_free(void *player) {
    mod_player *p = player;
    if (!p) return;
    stock_jka_player_free(p->base); japro_jka_player_free(p->japro); free(p);
}
void *jka_player_clone(const void *player) {
    const mod_player *p = player;
    mod_player *copy = malloc(sizeof(*copy));
    if (!copy) return NULL;
    *copy = *p;
    copy->base = stock_jka_player_clone(p->base);
    copy->japro = japro_jka_player_clone(p->japro);
    if (!copy->base || !copy->japro) { jka_player_free(copy); return NULL; }
    return copy;
}
/* Configure before loading the authoritative snapshot and before fixed-step
 * view-angle updates. Changing backend transfers only the wire contract. */
int jka_player_configure(void *player, const jka_predict_settings *settings) {
    mod_player *p = player;
    if (settings->server_mod != 0 && settings->server_mod != 1) return 0;
    if (settings->pmove_fixed && (settings->pmove_msec < 1 || settings->pmove_msec > 66)) return 0;
    if (is_japro(p) != (settings->server_mod == 1)) {
        int count = jka_ps_field_count(), stats[16], persistant[16], ammo[16], powerups[16];
        int *fields = malloc(count * sizeof(int));
        int ok;
        if (!fields) return 0;
        if (is_japro(p)) {
            ok = japro_jka_player_get_network(p->japro, fields, count, stats, persistant, ammo, powerups);
            if (ok) ok = stock_jka_player_set_network(p->base, fields, count, stats, persistant, ammo, powerups);
        } else {
            ok = stock_jka_player_get_network(p->base, fields, count, stats, persistant, ammo, powerups);
            if (ok) ok = japro_jka_player_set_network(p->japro, fields, count, stats, persistant, ammo, powerups);
        }
        free(fields);
        if (!ok) return 0;
    }
    p->settings = *settings;
    if (is_japro(p)) japro_configure(settings);
    return 1;
}

void jka_player_jump_level(void *player, int level) {
    mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_jump_level(p->japro, level);
        return;
    }
    stock_jka_player_jump_level(p->base, level);
}

void jka_player_knockback(void *player, const float *velocity, int duration) {
    mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_knockback(p->japro, velocity, duration);
        return;
    }
    stock_jka_player_knockback(p->base, velocity, duration);
}

void jka_player_set_noclip(void *player, int enabled) {
    mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_set_noclip(p->japro, enabled);
        return;
    }
    stock_jka_player_set_noclip(p->base, enabled);
}

void jka_player_teleport(void *player, const float *origin, const float *angles, int speed) {
    mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_teleport(p->japro, origin, angles, speed);
        return;
    }
    stock_jka_player_teleport(p->base, origin, angles, speed);
}

void jka_player_give_all(void *player) {
    mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_give_all(p->japro);
        return;
    }
    stock_jka_player_give_all(p->base);
}

int jka_player_set_saber_movement_info(void *player, int saber_num, const jka_saber_movement_info *info) {
    mod_player *p = player;
    if (saber_num < 0 || saber_num > 1) return 0;
    /* Keep equipment metadata available in both native backends. */
    stock_jka_player_set_saber_movement_info(p->base, saber_num, info);
    return japro_jka_player_set_saber_movement_info(p->japro, saber_num, info);
}

int jka_player_set_foot_bolts(void *player, const float *left, const float *right) {
    mod_player *p = player;
    /* Both backends keep the pose: a server_mod switch must not lose it. */
    stock_jka_player_set_foot_bolts(p->base, left, right);
    return japro_jka_player_set_foot_bolts(p->japro, left, right);
}

void jka_player_offline_force_tick(void *player, int time, int requested_power) {
    mod_player *p = player;
    last_backend = is_japro(p);
    if (last_backend) {
        japro_configure(&p->settings);
        japro_jka_player_offline_force_tick(p->japro, time, requested_power);
        return;
    }
    stock_jka_player_offline_force_tick(p->base, time, requested_power);
}

int jka_player_step(void *player, const jka_cmd *input, int tick, jka_trace_fn trace, jka_contents_fn contents, void *context) {
    mod_player *p = player;
    last_backend = is_japro(p);
    if (last_backend) {
        japro_configure(&p->settings);
        return japro_jka_player_step(p->japro, input, tick, trace, contents, context);
    }
    return stock_jka_player_step(p->base, input, tick, trace, contents, context);
}

void jka_player_view(const void *player, jka_view *view) {
    const mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_view(p->japro, view);
        return;
    }
    stock_jka_player_view(p->base, view);
}

void jka_player_entity_view(const void *player, jka_entity_view *view) {
    const mod_player *p = player;
    if (is_japro(p)) {
        japro_jka_player_entity_view(p->japro, view);
        return;
    }
    stock_jka_player_entity_view(p->base, view);
}

int jka_player_set_network(void *player, const int32_t *fields, int count, const int32_t *stats, const int32_t *persistant, const int32_t *ammo, const int32_t *powerups) {
    mod_player *p = player;
    if (is_japro(p)) {
        return japro_jka_player_set_network(p->japro, fields, count, stats, persistant, ammo, powerups);
    }
    return stock_jka_player_set_network(p->base, fields, count, stats, persistant, ammo, powerups);
}

int jka_player_get_network(const void *player, int32_t *fields, int count, int32_t *stats, int32_t *persistant, int32_t *ammo, int32_t *powerups) {
    const mod_player *p = player;
    if (is_japro(p)) {
        return japro_jka_player_get_network(p->japro, fields, count, stats, persistant, ammo, powerups);
    }
    return stock_jka_player_get_network(p->base, fields, count, stats, persistant, ammo, powerups);
}

void jka_player_update_view_angles(void *player, const jka_cmd *input) {
    mod_player *p = player;
    last_backend = is_japro(p);
    if (last_backend) {
        japro_configure(&p->settings);
        japro_jka_player_update_view_angles(p->japro, input);
        return;
    }
    stock_jka_player_update_view_angles(p->base, input);
}

int jka_player_predict(void *player, const jka_cmd *input, const jka_predict_settings *settings, jka_trace_fn trace, jka_contents_fn contents, void *context) {
    mod_player *p = player;
    if (!jka_player_configure(player, settings)) return 0;
    last_backend = is_japro(p);
    if (last_backend) {
        japro_configure(&p->settings);
        return japro_jka_player_predict(p->japro, input, settings, trace, contents, context);
    }
    return stock_jka_player_predict(p->base, input, settings, trace, contents, context);
}
