// Camera-window block culling (not Unreal). The window grid draws as
// BLOCK x BLOCK-cell instances; a block is live when any cell within one cell
// of it holds water depth. A quad's vertices and pixels read only the cells
// within one cell of it (cell-centred bilinear zero loads), and the material
// discards every pixel whose bilinear depth is zero, so a culled block could
// not have produced a visible pixel.

struct Template {
    indices: u32, // index count
    first: u32,   // first index in the shared index buffer
    pad0: u32,
    pad1: u32,
}
struct Settings {
    cells: vec2<u32>,  // mesh cells xy (= SimGridRT dimensions)
    blocks: vec2<u32>, // block grid xy
    // r_riverAdaptiveMesh templates (river_blocks.rs), TEMPLATE_* order.
    templates: array<Template, 4>,
}
@group(0) @binding(0) var<uniform> settings: Settings;
@group(0) @binding(1) var surface: texture_2d<f32>;
@group(0) @binding(2) var<storage, read_write> flags: array<u32>;
@group(0) @binding(3) var<storage, read_write> blocks: array<u32>;
@group(0) @binding(4) var<storage, read_write> args: array<u32>;
struct Tuning {
    // cell size cm, r_riverMeshTurbulence, unused, unused
    grid: vec4<f32>,
    // The vertex wave height's speed response (wave_advection::Height):
    // low-velocity scale, velocity min, velocity max (cm/s), power.
    wave: vec4<f32>,
}
@group(0) @binding(5) var<uniform> tuning: Tuning;
@group(0) @binding(6) var<storage, read_write> template_of: array<u32>;
// B hysteresis: each block's held (decaying) turbulence score.
@group(0) @binding(7) var<storage, read_write> block_score: array<f32>;
// B: blocks [x0, y0]..[x1, y1] around the solver's wake bodies (player
// water deformation). They keep the tile resolution while a body is there.
struct Wake {
    count: vec4<i32>,
    rects: array<vec4<i32>, 16>,
}
@group(0) @binding(8) var<uniform> wake: Wake;

const BLOCK: i32 = 16;
const SPAN: i32 = 18; // BLOCK plus the one-cell border each side

var<workgroup> live: atomic<u32>;

// One workgroup per block.
@compute @workgroup_size(64)
fn mark(@builtin(workgroup_id) block: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    if lane == 0u { atomicStore(&live, 0u); }
    workgroupBarrier();
    let first = vec2<i32>(block.xy) * BLOCK - vec2<i32>(1);
    let size = vec2<i32>(settings.cells);
    for (var i = i32(lane); i < SPAN * SPAN; i += 64) {
        let cell = first + vec2<i32>(i % SPAN, i / SPAN);
        if all(cell >= vec2<i32>(0)) && all(cell < size) && textureLoad(surface, cell, 0).y > 1e-8 {
            atomicStore(&live, 1u);
        }
    }
    workgroupBarrier();
    if lane == 0u {
        flags[block.y * settings.blocks.x + block.x] = atomicLoad(&live);
    }
}

// r_riverBlockCull 0: every block live (the full grid, for A/B).
@compute @workgroup_size(64)
fn mark_all(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x < settings.blocks.x * settings.blocks.y { flags[id.x] = 1u; }
}

var<workgroup> scan: array<u32, 256>;
var<workgroup> base: u32;

// One workgroup: ordered compaction of the live blocks (row-major, as the
// static grid draws) and the DrawIndexedIndirect arguments.
@compute @workgroup_size(256)
fn compact(@builtin(local_invocation_index) lane: u32) {
    let count = settings.blocks.x * settings.blocks.y;
    if lane == 0u { base = 0u; }
    workgroupBarrier();
    let chunks = (count + 255u) / 256u;
    for (var chunk = 0u; chunk < chunks; chunk++) {
        let i = chunk * 256u + lane;
        var flag = 0u;
        if i < count { flag = flags[i]; }
        scan[lane] = flag;
        workgroupBarrier();
        for (var offset = 1u; offset < 256u; offset <<= 1u) {
            var add = 0u;
            if lane >= offset { add = scan[lane - offset]; }
            workgroupBarrier();
            scan[lane] += add;
            workgroupBarrier();
        }
        let start = base;
        if flag != 0u {
            blocks[start + scan[lane] - 1u] = (i % settings.blocks.x) | ((i / settings.blocks.x) << 16u);
        }
        workgroupBarrier();
        if lane == 255u { base = start + scan[255]; }
        workgroupBarrier();
    }
    if lane == 0u {
        args[0] = u32(BLOCK * BLOCK * 6);
        args[1] = base;
        args[2] = 0u;
        args[3] = 0u;
        args[4] = 0u;
    }
}

// r_riverAdaptiveMesh 1 (B, renderer only): each live block's mesh from its
// water's turbulence, with no distance term. It only removes triangles: the
// tile resolution (FULL) is the most any block gets. Templates
// (river_blocks.rs):
const TEMPLATE_FULL: u32 = 0u;  // one vertex per cell (A's mesh)
const TEMPLATE_STEP2: u32 = 1u; // 2x2-cell interior quads
const TEMPLATE_STEP4: u32 = 2u;
const TEMPLATE_STEP8: u32 = 3u;
const TEMPLATES: u32 = 4u;
//
// Each wet cell scores max(curvature / 0.3, waves / 0.35):
//   curvature: the solved surface height's largest second difference there,
//     in cells (cm / cell size), less fp16 rounding. A step-L interior
//     misses about curvature * L^2 / 8 of height, so each coarser template
//     needs a 4x smaller score.
//   waves: the vertex wave height's speed response, pow(response, power)
//     (river_advected_height): fast water carries bigger vertex waves.
// The block's score is its SCORE_RANK-th roughest cell (a log2 histogram),
// so a few noisy cells cannot hold a calm block at full detail. Hysteresis:
// the stored score rises at once and decays by SCORE_DECAY per update
// (solver frame), and a block coarsens only when that score is
// COARSEN_MARGIN below the coarser template's threshold. Blocks with wet
// and dry cells stay STEP4 or finer (the bank's extrapolated heights meet
// the water there).
const SCORE_RANK: u32 = 6u;
const SCORE_DECAY: f32 = 0.97;
const COARSEN_MARGIN: f32 = 1.5;
const WAKE_SCORE: f32 = 4.0;
const BINS: u32 = 12u; // bin b holds scores in [2^(b-8), 2^(b-7)); bin 0 also below
var<workgroup> lod_wet: atomic<u32>;
var<workgroup> lod_dry: atomic<u32>;
var<workgroup> lod_histogram: array<atomic<u32>, 12>;

fn height_at(cell: vec2<i32>) -> f32 {
    return textureLoad(surface, clamp(cell, vec2<i32>(0), vec2<i32>(settings.cells) - vec2<i32>(1)), 0).x;
}

// Rounding step of an RGBA16Float value of magnitude |h|.
fn half_ulp(h: f32) -> f32 {
    return exp2(floor(log2(max(abs(h), 6.1e-5))) - 10.0);
}

fn wave_weight(speed: f32) -> f32 {
    let response = smoothstep(tuning.wave.y, tuning.wave.z, speed);
    let a = mix(1.0, response, 1.0 - clamp(tuning.wave.x, 0.0, 1.0));
    return pow(max(a, 0.0), tuning.wave.w);
}

fn score_bin(score: f32) -> u32 {
    if !(score >= exp2(-7.0)) { return 0u; } // also NaN
    return u32(clamp(floor(log2(min(score, 1.0e9))) + 8.0, 0.0, f32(BINS - 1u)));
}

// FULL at score 1, then each template at a 4x smaller score.
fn level_of(score: f32) -> u32 {
    if score >= 1.0 { return TEMPLATE_FULL; }
    if score >= 0.25 { return TEMPLATE_STEP2; }
    if score >= 0.0625 { return TEMPLATE_STEP4; }
    return TEMPLATE_STEP8;
}

fn classify_block(block: vec3<u32>, lane: u32, force_all: bool) {
    if lane == 0u {
        atomicStore(&lod_wet, 0u);
        atomicStore(&lod_dry, 0u);
    }
    if lane < BINS { atomicStore(&lod_histogram[lane], 0u); }
    workgroupBarrier();
    // The same 18x18 neighbourhood as mark, so flags (also read by the foam
    // stage) are identical to A's.
    let first = vec2<i32>(block.xy) * BLOCK - vec2<i32>(1);
    let size = vec2<i32>(settings.cells);
    let cell_cm = max(tuning.grid.x, 1e-3);
    for (var i = i32(lane); i < SPAN * SPAN; i += 64) {
        let cell = first + vec2<i32>(i % SPAN, i / SPAN);
        if all(cell >= vec2<i32>(0)) && all(cell < size) {
            let s = textureLoad(surface, cell, 0);
            if s.y > 1e-8 {
                atomicStore(&lod_wet, 1u);
                let dx = height_at(cell - vec2<i32>(1, 0)) + height_at(cell + vec2<i32>(1, 0)) - 2.0 * s.x;
                let dy = height_at(cell - vec2<i32>(0, 1)) + height_at(cell + vec2<i32>(0, 1)) - 2.0 * s.x;
                let curvature = max(max(abs(dx), abs(dy)) - 4.0 * half_ulp(s.x), 0.0) / cell_cm;
                let score = max(curvature / 0.3, wave_weight(length(s.zw)) / 0.35);
                atomicAdd(&lod_histogram[score_bin(score)], 1u);
            } else {
                atomicStore(&lod_dry, 1u);
            }
        }
    }
    workgroupBarrier();
    if lane == 0u {
        let i = block.y * settings.blocks.x + block.x;
        let wet = atomicLoad(&lod_wet) != 0u;
        flags[i] = select(0u, 1u, wet || force_all);
        // The SCORE_RANK-th largest cell score (its bin's lower bound).
        var measured = 0.0;
        var seen = 0u;
        for (var b = i32(BINS) - 1; b > 0; b--) {
            seen += atomicLoad(&lod_histogram[b]);
            if seen >= SCORE_RANK {
                measured = exp2(f32(b) - 8.0);
                break;
            }
        }
        var woken = false;
        for (var k = 0; k < min(wake.count.x, 16); k++) {
            let r = wake.rects[k];
            let at = vec2<i32>(block.xy);
            if all(at >= r.xy) && all(at <= r.zw) { woken = true; }
        }
        var held = max(measured, block_score[i] * SCORE_DECAY);
        // The wake behind a body also stays fine while this decays.
        if woken { held = max(held, WAKE_SCORE / max(tuning.grid.y, 0.05)); }
        block_score[i] = held;
        let score = held * tuning.grid.y;
        var choice = TEMPLATE_STEP8; // dry: drawn only with r_riverBlockCull 0, all discarded
        if wet {
            let previous = min(template_of[i], TEMPLATE_STEP8);
            let wanted = level_of(score);
            if wanted < previous {
                choice = wanted;
            } else {
                choice = max(previous, level_of(score * COARSEN_MARGIN));
            }
            if atomicLoad(&lod_dry) != 0u {
                choice = min(choice, TEMPLATE_STEP4);
            }
            if woken { choice = TEMPLATE_FULL; }
        }
        template_of[i] = choice;
    }
}

// One workgroup per block (r_riverBlockCull 1).
@compute @workgroup_size(64)
fn mark_adaptive(@builtin(workgroup_id) block: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    classify_block(block, lane, false);
}

// r_riverBlockCull 0: every block live, still classified.
@compute @workgroup_size(64)
fn mark_adaptive_all(@builtin(workgroup_id) block: vec3<u32>, @builtin(local_invocation_index) lane: u32) {
    classify_block(block, lane, true);
}

// One workgroup: compact into one bucket per template, bucket t at
// [t * count, (t + 1) * count). Record t starts at template t's first
// index; first_instance stays 0 (no INDIRECT_FIRST_INSTANCE): the template's
// index values carry t to vs_block.
@compute @workgroup_size(256)
fn compact_adaptive(@builtin(local_invocation_index) lane: u32) {
    let count = settings.blocks.x * settings.blocks.y;
    let chunks = (count + 255u) / 256u;
    for (var t = 0u; t < TEMPLATES; t++) {
        if lane == 0u { base = 0u; }
        workgroupBarrier();
        for (var chunk = 0u; chunk < chunks; chunk++) {
            let i = chunk * 256u + lane;
            var flag = 0u;
            if i < count && flags[i] != 0u && template_of[i] == t { flag = 1u; }
            scan[lane] = flag;
            workgroupBarrier();
            for (var offset = 1u; offset < 256u; offset <<= 1u) {
                var add = 0u;
                if lane >= offset { add = scan[lane - offset]; }
                workgroupBarrier();
                scan[lane] += add;
                workgroupBarrier();
            }
            let start = base;
            if flag != 0u {
                blocks[t * count + start + scan[lane] - 1u] = (i % settings.blocks.x) | ((i / settings.blocks.x) << 16u);
            }
            workgroupBarrier();
            if lane == 255u { base = start + scan[255]; }
            workgroupBarrier();
        }
        if lane == 0u {
            args[t * 5u] = settings.templates[t].indices;
            args[t * 5u + 1u] = base;
            args[t * 5u + 2u] = settings.templates[t].first;
            args[t * 5u + 3u] = 0u;
            args[t * 5u + 4u] = 0u;
        }
        workgroupBarrier();
    }
}
