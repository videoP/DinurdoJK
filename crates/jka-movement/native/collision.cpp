#include "qcommon/cm_local.h"
#include "bridge.h"
#include <algorithm>
#include <mutex>
#include <stdexcept>
#include <string>
#include <vector>

static std::recursive_mutex engine_mutex;
extern "C" void jka_lock() { engine_mutex.lock(); }
extern "C" void jka_unlock() { engine_mutex.unlock(); }
static thread_local std::string last_error;
extern "C" const char *jka_collision_error() { return last_error.c_str(); }

struct World {
    clipMap_t map{};
    // CM's inline models store offsets between hunk allocations. Keep a single
    // contiguous arena, as the stock engine does, so those offsets remain valid.
    static constexpr size_t capacity = 256u * 1024u * 1024u;
    unsigned char *arena = static_cast<unsigned char *>(malloc(capacity));
    size_t used = 0;
    std::vector<void *> temporary;
    World() { if (!arena) throw std::bad_alloc(); }
    ~World() { for (void *p : temporary) free(p); free(arena); }
};
static World *loading;
static const unsigned char *map_data;
static int map_length;
static cvar_t zero_cvar{}, one_cvar{};
cvar_t *com_dedicated = &one_cvar;
qboolean CM_DeleteCachedMap(qboolean);

void QDECL JKA_CM_Printf(const char *, ...) {}
void QDECL Com_DPrintf(const char *, ...) {}
void BotDrawDebugPolygons(void (*)(int, int, float *), int) {}
void NORETURN QDECL JKA_CM_Error(int, const char *format, ...) {
    char buffer[1024];
    va_list args;
    va_start(args, format);
    vsnprintf(buffer, sizeof(buffer), format, args);
    va_end(args);
    throw std::runtime_error(buffer);
}
void *Hunk_Alloc(int size, ha_pref) {
    if (!loading || size < 0) throw std::runtime_error("Invalid collision allocation");
    size_t length = (static_cast<size_t>(std::max(size, 1)) + 15u) & ~size_t(15u);
    if (length > World::capacity - loading->used) throw std::runtime_error("Collision arena exceeds 256 MiB");
    void *p = loading->arena + loading->used;
    loading->used += length;
    memset(p, 0, length);
    return p;
}
void *Z_Malloc(int size, memtag_t, qboolean zero, int) {
    if (size < 0) throw std::runtime_error("Invalid zone allocation");
    void *p = zero ? calloc(1, std::max(size, 1)) : malloc(std::max(size, 1));
    if (!p) throw std::bad_alloc();
    if (loading) loading->temporary.push_back(p);
    return p;
}
void Z_Free(void *p) {
    if (loading) {
        auto &blocks = loading->temporary;
        auto found = std::find(blocks.begin(), blocks.end(), p);
        if (found != blocks.end()) blocks.erase(found);
    }
    free(p);
}
void *Hunk_AllocateTempMemory(int size) { return Z_Malloc(size, TAG_TEMP_WORKSPACE, qtrue, 4); }
void Hunk_FreeTempMemory(void *p) { Z_Free(p); }
cvar_t *Cvar_Get(const char *, const char *value, uint32_t, const char *) {
    one_cvar.integer = 1; one_cvar.value = 1;
    return atoi(value) ? &one_cvar : &zero_cvar;
}
long FS_FOpenFileRead(const char *name, fileHandle_t *handle, qboolean) {
    if (strcmp(name, "maps/offline.bsp")) { *handle = 0; return -1; }
    *handle = 1; return map_length;
}
int FS_Read(void *buffer, int length, fileHandle_t handle) {
    if (handle != 1 || length < 0 || length > map_length) throw std::runtime_error("Invalid BSP read");
    memcpy(buffer, map_data, length); return length;
}
void FS_FCloseFile(fileHandle_t) {}
qboolean Sys_LowPhysicalMemory() { return qtrue; }
/* The checksum is unused by this offline host. Network checksum support is separate. */
uint32_t Com_BlockChecksum(const void *, int) { return 0; }

extern "C" void *jka_world_new(const unsigned char *data, int length) {
    std::lock_guard<std::recursive_mutex> lock(engine_mutex);
    World *world = nullptr;
    try {
        world = new World;
        loading = world; map_data = data; map_length = length;
        int checksum;
        CM_LoadMap("maps/offline.bsp", qfalse, &checksum);
        world->map = cmg;
        loading = nullptr; map_data = nullptr; map_length = 0;
        return world;
    } catch (const std::exception &e) {
        last_error = e.what();
        CM_DeleteCachedMap(qtrue);
        CM_ClearMap();
        loading = nullptr; map_data = nullptr; map_length = 0;
        delete world;
        return nullptr;
    }
}
extern "C" void jka_world_free(void *handle) {
    std::lock_guard<std::recursive_mutex> lock(engine_mutex);
    CM_ClearMap();
    delete static_cast<World *>(handle);
}
static void export_trace(const trace_t &t, jka_trace *out) {
    out->fraction = t.fraction;
    VectorCopy(t.endpos, out->end); VectorCopy(t.plane.normal, out->normal);
    out->distance = t.plane.dist; out->surface_flags = t.surfaceFlags;
    out->contents = t.contents; out->entity = t.entityNum;
    out->start_solid = t.startsolid; out->all_solid = t.allsolid;
}
extern "C" int jka_world_trace(void *handle, jka_trace *out, const float *start, const float *mins,
                               const float *maxs, const float *end, int mask, int model,
                               const float *origin, const float *angles) {
    std::lock_guard<std::recursive_mutex> lock(engine_mutex);
    World *world = static_cast<World *>(handle);
    cmg = world->map;
    try {
        trace_t t{};
        if (model) CM_TransformedBoxTrace(&t, start, end, mins, maxs, model, mask, origin, angles, 0);
        else CM_BoxTrace(&t, start, end, mins, maxs, 0, mask, 0);
        t.entityNum = (t.fraction < 1 || t.startsolid) ? ENTITYNUM_WORLD : ENTITYNUM_NONE;
        export_trace(t, out);
        world->map = cmg;
        return 1;
    } catch (const std::exception &e) { last_error = e.what(); return 0; }
}
extern "C" int jka_world_contents(void *handle, const float *point, int model, const float *origin, const float *angles) {
    std::lock_guard<std::recursive_mutex> lock(engine_mutex);
    World *world = static_cast<World *>(handle);
    cmg = world->map;
    try {
        return model ? CM_TransformedPointContents(point, model, origin, angles) : CM_PointContents(point, 0);
    } catch (const std::exception &e) { last_error = e.what(); return -1; }
}
/* CG_ClipMoveToEntities for an encoded-bbox entity: CM_TempBoxModel placed at
 * the entity origin with no rotation (trap->CM_TempModel + CM_TransformedTrace). */
extern "C" int jka_world_trace_box_entity(void *handle, jka_trace *out, const float *start, const float *mins,
                                          const float *maxs, const float *end, int mask,
                                          const float *box_mins, const float *box_maxs, const float *origin) {
    std::lock_guard<std::recursive_mutex> lock(engine_mutex);
    World *world = static_cast<World *>(handle);
    cmg = world->map;
    try {
        trace_t t{};
        const vec3_t no_rotation = {0, 0, 0};
        clipHandle_t box = CM_TempBoxModel(box_mins, box_maxs, 0);
        CM_TransformedBoxTrace(&t, start, end, mins, maxs, box, mask, origin, no_rotation, 0);
        export_trace(t, out);
        world->map = cmg;
        return 1;
    } catch (const std::exception &e) { last_error = e.what(); return 0; }
}
