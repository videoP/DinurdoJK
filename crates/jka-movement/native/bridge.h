#pragma once
#include <stdint.h>

typedef struct jka_trace_s {
    float fraction, end[3], normal[3], distance;
    int32_t surface_flags, contents, entity, start_solid, all_solid;
} jka_trace;
typedef void (*jka_trace_fn)(void *, jka_trace *, const float *, const float *, const float *, const float *, int32_t, int32_t);
typedef int32_t (*jka_contents_fn)(void *, const float *, int32_t);
typedef struct jka_cmd_s {
    int32_t time, angles[3], buttons;
    uint8_t weapon, force_selection, inventory_selection, generic_command;
    int8_t forward, right, up;
} jka_cmd;
typedef struct jka_view_s {
    float origin[3], velocity[3], angles[3], mins[3], maxs[3];
    int32_t time, pm_type, flags, timer, ground_entity, view_height, bob_cycle;
    int32_t legs_anim, legs_timer, torso_anim, torso_timer;
    int32_t water_level, water_type, force_power, force_jump_level;
    int32_t event_sequence, events[2], event_parms[2];
    int32_t touch_count, touches[32];
    int32_t delta_angles[3], health, active_powers, rage_recovery;
    int32_t armor, max_health, force_power_max, weapon, ammo;
} jka_view;

/* Read-only presentation projection produced by OpenJK BG_PlayerStateToEntityState. */
typedef struct jka_entity_view_s {
    int32_t number, e_type, client_num;
    float origin[3], velocity[3], angles[3], speed, origin2[3];
    int32_t trickedentindex, trickedentindex2, trickedentindex3, trickedentindex4;
    int32_t force_frame, emplaced_owner, generic_enemy_index, active_force_pass, movement_dir;
    int32_t legs_anim, torso_anim, legs_flip, torso_flip, e_flags, e_flags2;
    int32_t saber_in_flight, saber_entity_num, saber_move, force_powers_active;
    int32_t bolt1, other_entity_num2, saber_holstered, event, event_parm;
    int32_t weapon, ground_entity_num, powerups, loop_sound, generic1;
    int32_t modelindex2, constant_light, is_jedi_master, time2, fireflag;
    int32_t held_by_client, rag_attach, model_scale, broken_limbs;
    int32_t has_look_target, look_target, custom_rgba[4], vehicle_num;
    /* PlayerState values used by OpenJK's CG_DrawActiveFrame camera decision. */
    int32_t pm_type, view_height, health, dead_yaw, team, zoom_mode;
    int32_t emplaced_index, force_hand_extend, falling_to_death;
} jka_entity_view;


/* Minimal entityState projection consumed by stock OpenJK BG_G2PlayerAngles. */
typedef struct jka_player_angle_entity_s {
    int32_t number, entity_type;
    float velocity[3];
    int32_t movement_dir, legs_anim, torso_anim, e_flags, weapon, ground_entity_num;
    int32_t force_frame, saber_move, vehicle_num, held_by_client, other_entity_num2;
} jka_player_angle_entity;

/* Persistent centity/clientInfo presentation state consumed and updated by
 * OpenJK BG_G2PlayerAngles / BG_SwingAngles / BG_UpdateLookAngles. */
typedef struct jka_player_angle_state_s {
    int32_t torso_yawing, torso_pitching, legs_yawing;
    float torso_yaw_angle, torso_pitch_angle, legs_yaw_angle;
    int32_t corr_time, look_time, super_smooth_time;
    float last_head_angles[3];
} jka_player_angle_state;

#define JKA_MAX_BONE_ANGLE_COMMANDS 16
#define JKA_BONE_NAME_CAP 64
typedef struct jka_bone_angle_command_s {
    char bone_name[JKA_BONE_NAME_CAP];
    float angles[3];
    int32_t flags, up, right, forward;
} jka_bone_angle_command;

typedef struct jka_player_angle_result_s {
    float legs_axis[9];
    float legs_angles[3];
    float tur_angles[3];
    int32_t command_count;
    jka_bone_angle_command commands[JKA_MAX_BONE_ANGLE_COMMANDS];
} jka_player_angle_result;

/* Server-provided Pmove configuration for CG_PredictPlayerState. */
typedef struct jka_predict_settings_s {
    int32_t pmove_fixed, pmove_msec, pmove_float, gametype;
    int32_t debug_melee, step_slide_fix, no_spec_move, tracemask, no_footsteps;
} jka_predict_settings;
