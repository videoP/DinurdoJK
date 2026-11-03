/* JK2 map collision: the pinned jk2mv engine's own clip-map code (cm_load,
 * cm_trace, cm_test, cm_patch, cm_polylib, q_math, q_shared), unmodified and
 * namespaced (jk2_cm_prelude.h). This host only supplies the engine services
 * those files call and keeps one loaded clip map per world, mirroring
 * collision.cpp's OpenJK host. JKA maps keep using OpenJK's collision. */
#include "jk2_cm_prelude.h"
#include "bridge.h"
#include <algorithm>
#include <mutex>
#include <stdexcept>
#include <string>
#include <vector>

namespace jk2mv_cm {
#include "qcommon/cm_local.h"

extern cmodel_t box_model;
extern cplane_t *box_planes;
extern cbrush_t *box_brush;
qboolean CM_DeleteCachedMap(qboolean bGuaranteedOkToDelete);

struct World {
    clipMap_t map{};
    cmodel_t box{};
    cplane_t *box_planes = nullptr;
    cbrush_t *box_brush = nullptr;
    // Inline models and leaves hold pointers into hunk allocations; keep one
    // contiguous arena per world, as the engine's hunk is.
    static constexpr size_t capacity = 256u * 1024u * 1024u;
    unsigned char *arena = static_cast<unsigned char *>(malloc(capacity));
    size_t used = 0;
    std::vector<void *> zone;
    World() { if (!arena) throw std::bad_alloc(); }
    ~World() { for (void *p : zone) free(p); free(arena); }
};

static std::recursive_mutex mutex;
static thread_local std::string last_error;
static World *loading;
static const unsigned char *map_data;
static int map_length;
static const char *const map_name = "maps/jk2.bsp";

// cm_load's globals belong to the clip map that is being traced.
static void activate(World *world) {
    cm = world->map;
    box_model = world->box;
    box_planes = world->box_planes;
    box_brush = world->box_brush;
}
static void capture(World *world) {
    world->map = cm;
    world->box = box_model;
    world->box_planes = box_planes;
    world->box_brush = box_brush;
}

void QDECL Com_Error(errorParm_t, const char *format, ...) {
    char buffer[1024];
    va_list args;
    va_start(args, format);
    vsnprintf(buffer, sizeof(buffer), format, args);
    va_end(args);
    throw std::runtime_error(buffer);
}
void QDECL Com_Printf(const char *, ...) {}
void QDECL Com_DPrintf(const char *, ...) {}

void *Hunk_Alloc(int size, ha_pref) {
    if (!loading || size < 0) throw std::runtime_error("Invalid JK2 collision allocation");
    size_t length = (static_cast<size_t>(std::max(size, 1)) + 15u) & ~size_t(15u);
    if (length > World::capacity - loading->used) throw std::runtime_error("JK2 collision arena exceeds 256 MiB");
    void *p = loading->arena + loading->used;
    loading->used += length;
    memset(p, 0, length);
    return p;
}
void *Z_Malloc(int size, memtag_t, qboolean zero) {
    if (size < 0) throw std::runtime_error("Invalid JK2 zone allocation");
    void *p = zero ? calloc(1, std::max(size, 1)) : malloc(std::max(size, 1));
    if (!p) throw std::bad_alloc();
    if (loading) loading->zone.push_back(p);
    return p;
}
void Z_Free(void *p) {
    if (loading) {
        auto found = std::find(loading->zone.begin(), loading->zone.end(), p);
        if (found != loading->zone.end()) loading->zone.erase(found);
    }
    free(p);
}

// cm_noAreas 0, cm_noCurves 0, cm_playerCurveClip 1: their registered values.
static cvar_t cvars[8];
static int cvar_count;
cvar_t *Cvar_Get(const char *, const char *value, int) {
    if (cvar_count == static_cast<int>(sizeof(cvars) / sizeof(cvars[0]))) throw std::runtime_error("JK2 collision cvar table full");
    cvar_t *cvar = &cvars[cvar_count++];
    cvar->integer = atoi(value);
    cvar->value = static_cast<float>(atof(value));
    return cvar;
}

int FS_FOpenFileRead(const char *name, fileHandle_t *handle, qboolean, module_t, qboolean) {
    if (!map_data || strcmp(name, map_name)) { *handle = 0; return -1; }
    *handle = 1;
    return map_length;
}
int FS_Read(void *buffer, int length, fileHandle_t handle, module_t) {
    if (handle != 1 || length < 0 || length > map_length) throw std::runtime_error("Invalid JK2 BSP read");
    memcpy(buffer, map_data, length);
    return length;
}
void FS_FCloseFile(fileHandle_t, module_t) {}
/* CM_DrawDebugSurface (renderer debug view) only; no botlib here. */
void BotDrawDebugPolygons(void (*)(int, int, float *), int) {}
/* The map checksum is the server's business; prediction never uses it. */
unsigned Com_BlockChecksum(const void *, int) { return 0; }
}  // namespace jk2mv_cm

using namespace jk2mv_cm;

static void export_trace(const trace_t &t, jka_trace *out) {
    out->fraction = t.fraction;
    VectorCopy(t.endpos, out->end);
    VectorCopy(t.plane.normal, out->normal);
    out->distance = t.plane.dist;
    out->surface_flags = t.surfaceFlags;
    out->contents = t.contents;
    out->entity = t.entityNum;
    out->start_solid = t.startsolid;
    out->all_solid = t.allsolid;
}

extern "C" const char *jk2_collision_error() { return last_error.c_str(); }

extern "C" void *jk2_world_new(const unsigned char *data, int length) {
    std::lock_guard<std::recursive_mutex> lock(mutex);
    World *world = nullptr;
    try {
        world = new World;
        loading = world; map_data = data; map_length = length; cvar_count = 0;
        int checksum;
        CM_LoadMap(map_name, qfalse, &checksum);
        // The engine keeps the disk image cached for the renderer; free it now.
        CM_DeleteCachedMap(qtrue);
        capture(world);
        loading = nullptr; map_data = nullptr; map_length = 0;
        return world;
    } catch (const std::exception &e) {
        last_error = e.what();
        if (world) CM_DeleteCachedMap(qtrue);
        CM_ClearMap();
        loading = nullptr; map_data = nullptr; map_length = 0;
        delete world;
        return nullptr;
    }
}
extern "C" void jk2_world_free(void *handle) {
    std::lock_guard<std::recursive_mutex> lock(mutex);
    CM_ClearMap();
    delete static_cast<World *>(handle);
}
extern "C" int jk2_world_trace(void *handle, jka_trace *out, const float *start, const float *mins,
                               const float *maxs, const float *end, int mask, int model,
                               const float *origin, const float *angles) {
    std::lock_guard<std::recursive_mutex> lock(mutex);
    World *world = static_cast<World *>(handle);
    activate(world);
    try {
        trace_t t{};
        if (model) CM_TransformedBoxTrace(&t, start, end, mins, maxs, model, mask, origin, angles, qfalse);
        else CM_BoxTrace(&t, start, end, mins, maxs, 0, mask, qfalse);
        t.entityNum = (t.fraction < 1 || t.startsolid) ? ENTITYNUM_WORLD : ENTITYNUM_NONE;
        export_trace(t, out);
        capture(world);
        return 1;
    } catch (const std::exception &e) { last_error = e.what(); return 0; }
}
/* trap_CM_BoxTrace with an inline model handle: CM_BoxTrace at the model's
 * BSP position, untransformed (CG_TouchTriggerPrediction). */
extern "C" int jk2_world_trace_model(void *handle, jka_trace *out, const float *start, const float *mins,
                                     const float *maxs, const float *end, int mask, int model) {
    std::lock_guard<std::recursive_mutex> lock(mutex);
    World *world = static_cast<World *>(handle);
    activate(world);
    try {
        trace_t t{};
        CM_BoxTrace(&t, start, end, mins, maxs, CM_InlineModel(model), mask, qfalse);
        t.entityNum = (t.fraction < 1 || t.startsolid) ? ENTITYNUM_WORLD : ENTITYNUM_NONE;
        export_trace(t, out);
        capture(world);
        return 1;
    } catch (const std::exception &e) { last_error = e.what(); return 0; }
}
extern "C" int jk2_world_contents(void *handle, const float *point, int model, const float *origin, const float *angles) {
    std::lock_guard<std::recursive_mutex> lock(mutex);
    World *world = static_cast<World *>(handle);
    activate(world);
    try {
        const int contents = model ? CM_TransformedPointContents(point, model, origin, angles) : CM_PointContents(point, 0);
        capture(world);
        return contents;
    } catch (const std::exception &e) { last_error = e.what(); return -1; }
}
/* CG_ClipMoveToEntities for an encoded-bbox entity: trap_CM_TempBoxModel at
 * the entity origin, unrotated (jk2mv CM_TempBoxModel, capsule false). */
extern "C" int jk2_world_trace_box_entity(void *handle, jka_trace *out, const float *start, const float *mins,
                                          const float *maxs, const float *end, int mask,
                                          const float *box_mins, const float *box_maxs, const float *origin) {
    std::lock_guard<std::recursive_mutex> lock(mutex);
    World *world = static_cast<World *>(handle);
    activate(world);
    try {
        trace_t t{};
        const vec3_t no_rotation = {0, 0, 0};
        clipHandle_t box = CM_TempBoxModel(box_mins, box_maxs, qfalse);
        CM_TransformedBoxTrace(&t, start, end, mins, maxs, box, mask, origin, no_rotation, qfalse);
        export_trace(t, out);
        capture(world);
        return 1;
    } catch (const std::exception &e) { last_error = e.what(); return 0; }
}
