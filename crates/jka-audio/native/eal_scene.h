#pragma once
#include <stdint.h>

#define DJK_EAL_VOICES 96
#define DJK_EAL_FRAMES 256

// Fixed-layout, bounded block transport. Coordinates at this boundary are JKA.
struct DjkEalVoice {
    uint64_t id;
    float position[3];
    uint32_t active;
    char name[96];
    float pcm[DJK_EAL_FRAMES];
    float max_distance;
    float ambience_gain_hf;
};
struct DjkEalFrame {
    float listener[3];
    float ahead[3];
    float up[3];
    uint32_t hrtf;
    uint32_t debug_mode;
    struct DjkEalVoice voices[DJK_EAL_VOICES];
};
struct DjkEalStats {
    uint32_t sources;
    uint32_t rooms;
    uint32_t filtered_sources;
    uint32_t diffracted_sources;
    uint32_t directional_rooms;
    uint32_t hrtf;
    // Cumulative since the most recent diagnostic mode switch.
    uint32_t queue_starves;
    uint32_t restarts;
    uint32_t multi_processed;
    uint32_t buffer_reuse;
    float input_edge_max;
    float output_edge_max;
    uint64_t callback_underfeed_samples;
    uint64_t callback_dropped_samples;
    uint32_t transport; // 0=raw PCM, 1=callback, 2=legacy queued
    uint32_t hrtf_status; // ALC_HRTF_STATUS_SOFT, queried from the device
};

// Independent EFX snapshot ABI; the existing EAL frame is unchanged.
struct DjkEchoSlot { float density,diffusion,gain,gain_hf,gain_lf,decay_time,decay_hf_ratio,decay_lf_ratio,reflections_gain,reflections_delay,reflections_pan[3],late_gain,late_delay,late_pan[3],echo_time,echo_depth,modulation_time,modulation_depth,air_absorption_gain_hf,hf_reference,lf_reference,room_rolloff; };
struct DjkEchoVoice { uint64_t id; float position[3],direct_gain,direct_cutoff,send_gain[4],send_cutoff[4]; uint32_t redirected; };
struct DjkEchoFrame {
    struct DjkEchoSlot slots[4];
    struct DjkEchoVoice voices[DJK_EAL_VOICES];
    uint32_t disable_authored_diffraction;
    uint32_t reserved;
};
#ifdef __cplusplus
extern "C" {
#endif
void* djk_eal_create(const uint16_t* dll, const uint8_t* data, uint32_t bytes, uint32_t rate, char* error, uint32_t capacity);
int djk_eal_render(void* scene, const struct DjkEalFrame* frame, float* stereo, struct DjkEalStats* stats, char* error, uint32_t capacity);
int djk_echo_render(void* scene, const struct DjkEalFrame* frame, const struct DjkEchoFrame* echo, float* stereo, struct DjkEalStats* stats, char* error, uint32_t capacity);
void djk_eal_destroy(void* scene);
#ifdef __cplusplus
}
#endif
