/* Jump-trace diagnostics shared by the TaystJK Pmove host and its Rust callers. */
#include "jump_trace.h"
int jka_jump_trace_enabled;
int jka_force_jump_slope_enabled;
static jka_jump_event *jump_events;
static int jump_capacity, jump_count;
void jka_jump_trace_begin(jka_jump_event *events, int capacity) {
    jump_events = events;
    jump_capacity = capacity;
    jump_count = 0;
    jka_jump_trace_enabled = events && capacity > 0;
}
void jka_jump_trace_record(const jka_jump_event *event) {
    if (jump_count < jump_capacity) jump_events[jump_count++] = *event;
}
int jka_jump_trace_end(void) {
    int count = jump_count;
    jka_jump_trace_enabled = 0;
    jump_events = 0;
    jump_capacity = jump_count = 0;
    return count;
}
