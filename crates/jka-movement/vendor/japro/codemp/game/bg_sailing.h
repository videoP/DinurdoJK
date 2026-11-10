/*
===========================================================================
Shared sailing-ocean definitions used by game and cgame.
===========================================================================
*/

#pragma once

#include "qcommon/q_shared.h"

#define MAX_SAILING_OCEANS 8
#define MAX_SAILING_SHORES 32
#define MAX_SAILING_CURRENTS 32
#define MAX_SAILING_RIVERS 16
#define MAX_SAILING_RIVER_SOURCES 32
#define MAX_SAILING_RIVER_SINKS 32
#define MAX_SAILING_RIVER_BASE_SHADERS 8
/* River authoring uses the conventional JKA scale of 12 map units/foot. */
#define SAILING_MAP_UNITS_PER_FOOT 12.0f
#define SAILING_CUBIC_MAP_UNITS_PER_CUBIC_FOOT \
	( SAILING_MAP_UNITS_PER_FOOT * SAILING_MAP_UNITS_PER_FOOT \
		* SAILING_MAP_UNITS_PER_FOOT )
#define SAILING_RIVER_CELL_ACTIVE 0x00000001u
#define SAILING_RIVER_CELL_SOURCE 0x00000002u
#define SAILING_RIVER_CELL_SINK 0x00000004u
#define SAILING_GERSTNER_MAX_WAVES 12
#define SAILING_MAX_AMPLITUDE 16384.0f
#define SAILING_SAIL_TRIM_MAX 80.0f
#define SAILING_SAIL_FORCE_SCALE 0.0035f
#define SAILING_DAMAGE_HULL_SAMPLES 5
#define SAILING_SLAM_RATE_THRESHOLD 240.0f
#define SAILING_LATERAL_SAFE_LOAD 390.0f
#define SAILING_STRESS_DAMAGE_START 0.35f
/* Matches DEFAULT_GRAVITY, restated here so the shared wave code does not have
 * to pull in bg_public.h. */
#define SAILING_GRAVITY 800.0f
/* Real water gravity for ocean wave dispersion: 9.81 m/s^2 at the ocean
 * renderers' 40 map units per metre. Deliberately not SAILING_GRAVITY, which is
 * game gravity; wave speed has to match the rendered sea, not player jumps. */
#define SAILING_WAVE_GRAVITY 392.4f
/* Authored ocean speed -1: derive the deep-water phase speed of the authored
 * wavelength, sqrt( g * wavelength / 2pi ). Any other negative speed still
 * runs the waves backwards. */
#define SAILING_OCEAN_SPEED_AUTO -1.0f
/* A negative authored sinkDepth asks for the physical Rankine depth implied by
 * spinSpeed instead of a hand-tuned constant. */
#define SAILING_CURRENT_DERIVE_SINK -1.0f
/*
 * Default solid-body core as a fraction of the brush radius. This sets how much
 * of the footprint is broad spinning apron rather than narrow throat: the
 * parabolic cap spans the core, and the 1/r^2 skirt plus the 1/r flow both
 * start from its edge. A narrow core packs nearly all the depth and nearly all
 * the speed into the middle and leaves a wide, flat, slow-looking outer ring.
 */
#define SAILING_CURRENT_CORE_FRACTION 0.35f
/*
 * Ceiling on the derived depth, as a fraction of the brush radius. Generous,
 * because the depth is concentrated in a narrow throat rather than spread over
 * a wide cone - a deep figure here reads as a plunging air core, not a pit.
 */
#define SAILING_CURRENT_MAX_SINK_FRACTION 0.5f
/*
 * The surface throat, as a fraction of coreRadius. Deliberately decoupled from
 * the flow core: the flow wants a wide core so the middle and outer footprint
 * still spin hard, but the depression wants a narrow one so the profile is a
 * plunging throat inside a broad gentle dish rather than a single wide bowl.
 */
#define SAILING_CURRENT_THROAT_FRACTION 0.20f
/* Fraction of the outer radius at which the vortex depression begins tapering
 * out. The 1/r^2 skirt is still meaningfully deep at the rim, so it needs a
 * zero-slope landing to avoid creasing against open water. */
#define SAILING_CURRENT_RIM_TAPER 0.75f
/* Azimuthal shear rate, in reciprocal seconds, at which vortex foam saturates.
 * A vortex peaks at 2 * spinSpeed / coreRadius, so the stock whirlpool reaches
 * roughly half foam at its core edge and a lazier one stays lacy. */
#define SAILING_CURRENT_FOAM_SHEAR 6.0f
/* Half-width, as a fraction of coreRadius, of the band the foam blends across
 * at the core boundary. The Rankine profile's shear is genuinely discontinuous
 * there and steps straight from nothing to saturated without this. */
#define SAILING_CURRENT_FOAM_BAND 0.35f
/*
 * Flow speed at which the broad surface raft reaches full whitewater. Shear
 * alone only whitens the lip of the throat, so a vortex read as a dark hole
 * from any angle but straight down.
 *
 * Kept deliberately weak. The raft is a broad wash under the sheared ring and
 * the flow-advected lace; pushed high it floods the threshold into one solid
 * blob and swallows the streaking that actually reads as motion.
 */
#define SAILING_CURRENT_FOAM_SPEED 1400.0f
/*
 * The presentation gain that concentrates the ocean's own whitecap is
 * cg_sailingVortexFoamGain. It lives in cgame rather than shared physics
 * because it changes only the render descriptor.
 */

/*
 * Two-phase advection policy, after Crest's ShapeCombine.shader. Two copies of
 * an advected layer drift along the flow half a period apart and are
 * crossfaded, so each resets long before its coordinates can shear or fold.
 *
 * The half period is derived from how far the fastest part of a vortex drags
 * the layer relative to the feature size of whatever is being advected, so each
 * consumer supplies its own reference tile - but the policy lives here, once,
 * rather than being restated per render path.
 *
 * DRAG_FRACTION is how far a phase may drag its layer before restarting, as a
 * fraction of the reference tile. It does NOT set how fast the layer appears to
 * move: each copy's sample point travels at the flow speed no matter how long
 * the period is, so apparent motion is always |flow|.
 *
 * What it does set is how far apart the two crossfaded copies sit. Each copy's
 * weight peaks in the middle of its travel and falls to zero at both ends, so a
 * copy always fades in while it is behind the mean and fades out while it is
 * ahead of it. That is a backward-moving ghost against a forward-moving one,
 * and it becomes plainly visible - foam appearing to run both ways at once -
 * once the separation approaches the size of a feature in the advected texture.
 * Keep it well under that.
 */
#define SAILING_CURRENT_DRAG_FRACTION 0.05f
#define SAILING_CURRENT_MIN_DRAG 16.0f
#define SAILING_CURRENT_MIN_HALF_PERIOD 0.01f
#define SAILING_CURRENT_MAX_HALF_PERIOD 4.0f
/* Finest FFT cascade the enhanced renderer advects; its reference tile. */
#define SAILING_CURRENT_RENDER_TILE 640.0f
/*
 * Fraction of the inflow that advects the surface texture. Buoyant surface
 * material does not follow the bulk flow down - the radial inflow is strongest
 * at depth, while floating foam lingers and circulates - so the visible surface
 * spirals far tighter than the water carrying a hull. Advecting the texture
 * with the full physics inflow made it track almost straight into the throat.
 *
 * At stock speeds this turns a 14 degree spiral doing under two revolutions
 * into a 3.6 degree one doing about eight. Physics is untouched.
 */
#define SAILING_CURRENT_SURFACE_INFLOW 0.25f

/*
 * A single moving/local atmospheric hazard is enough for the first
 * waterspout implementation. Unlike misc_sailing_current this state is
 * runtime-owned and replaced from each server snapshot on cgame.
 */
typedef struct sailingWaterspout_s {
	vec3_t origin;
	float radius;
	float coreRadius;
	float height;
	float tangentialSpeed;
	float inflowSpeed;
	float liftSpeed;
	float intensity;
	unsigned int seed;
} sailingWaterspout_t;

typedef struct sailingOcean_s {
	vec3_t mins;
	vec3_t maxs;
	float baseHeight;
	float amplitude;
	float wavelength;
	/* Resolved phase speed every reader uses. Never author this directly: set
	 * speedAuthored and let BG resolve it, so -1 follows wavelength. */
	float speed;
	/* What the map/target_weatherchange asked for; SAILING_OCEAN_SPEED_AUTO
	 * derives speed from wavelength. This is what the configstring carries. */
	float speedAuthored;
	float steepness;
	float slosh;
	vec2_t direction;
	/* Wind is owned by misc_dynamic_weather (BG_DynamicWeather( 0 )); the ocean
	 * carries none. A map without one has no wind. */
	float rainIntensity;
	float currentScale;
	float textureDriftScale;
	float swimControlScale;
	/* Swimmer float spring. buoyancyDepth is the submersion past which lift
	 * stops growing, and diveDrag is the fraction of water drag that resists a
	 * fast descent, so a plunge keeps its momentum instead of being braked. */
	float buoyancy;
	float buoyancyDamping;
	float buoyancyDepth;
	float diveDrag;
	float gridSpacing;
	float renderDistance;
	float textureScale;
	vec3_t fogColor;
	float fogDistance;
	unsigned int waveSeed;
	int gerstnerWaveCount;
	/* Server-resolved rollout gate. Effective choppiness also multiplies the
	 * authored steepness, so a zero-steepness ocean never moves laterally. */
	float horizontalDisplacementScale;
	char baseShader[MAX_QPATH];
	char meshShader[MAX_QPATH];
} sailingOcean_t;

typedef struct sailingGerstnerRenderWave_s {
	vec2_t direction;
	float amplitude;
	float waveNumber;
	float angularFrequency;
	float phase;
} sailingGerstnerRenderWave_t;

/*
 * The sea physics floats in: the DinurdoJK client's Hybrid ocean (Blue Water's
 * FFT spectrum plus one analytic swell), reduced to the waves that matter for
 * height. Everything below is in Blue Water's frame: metres, x = map x,
 * z = -map y. See BG_SailingSea in bg_sailing.c.
 */
#define SAILING_SEA_UNITS_PER_METRE 40.0f
/* Blue Water's FFT tiles: 1024, 181 and 31 m (grown together for long seas),
 * 256 texels across. The first two carry the physics waves; the third (fine
 * chop under ~6 u) is drawn only. */
#define SAILING_SEA_CASCADES 3
#define SAILING_SEA_RESOLUTION 256
#define SAILING_SEA_PHYSICS_CASCADES 2
/* Each physics tile is rebuilt from this many waves with the same energy. */
#define SAILING_SEA_WAVES_PER_CASCADE 128
#define SAILING_SEA_MAX_WAVES \
	( SAILING_SEA_PHYSICS_CASCADES * SAILING_SEA_WAVES_PER_CASCADE )
/* Every angular frequency is a whole multiple of 2pi / this many seconds. */
#define SAILING_SEA_FOLD_PERIOD 4096.0

typedef struct sailingSeaWave_s {
	int cascade;
	/* Signed lattice index of the FFT texel holding +m (the twin at -nx, -nz
	 * holds the conjugate and -m). */
	int nx;
	int nz;
	/* Complex amplitude at that texel (m): height 2 Re( a e^{i(k.x - wt)} ). */
	float re;
	float im;
	/* w = 2 pi m / SAILING_SEA_FOLD_PERIOD. */
	float m;
	float kx;
	float kz;
	float k;
} sailingSeaWave_t;

typedef struct sailingSea_s {
	qboolean valid;
	/* Blue Water spectrum inputs (WaveParameters). */
	float significantHeight;
	float directionDegrees;
	float peakWavelength;
	float choppiness;
	float windSpeed;
	float tileSize[SAILING_SEA_CASCADES];
	/* Hybrid's FFT weight (the map's slosh); the swell gets 1 - it. */
	float fftBlend;
	/* Hybrid's analytic swell as authored (map units): amplitude, wavelength,
	 * phase speed, steepness with the horizontal gate applied. */
	float swellAmplitude;
	float swellWavelength;
	float swellSpeed;
	float swellSteepness;
	int waveCount;
	/* Waves are stored tile by tile: tile c holds cascadeWaves[c] of them,
	 * after the previous tiles'. */
	int cascadeWaves[SAILING_SEA_PHYSICS_CASCADES];
	sailingSeaWave_t wave[SAILING_SEA_MAX_WAVES];
} sailingSea_t;

typedef enum sailingShoreType_e {
	SAILING_SHORE_BEACH,
	SAILING_SHORE_CLIFF
} sailingShoreType_t;

typedef struct sailingShore_s {
	vec3_t mins;
	vec3_t maxs;
	vec2_t direction;
	int oceanIndex;
	sailingShoreType_t type;
} sailingShore_t;

typedef enum sailingCurrentType_e {
	SAILING_CURRENT_VORTEX
} sailingCurrentType_t;

typedef struct sailingCurrent_s {
	vec3_t origin;
	int oceanIndex;
	sailingCurrentType_t type;
	float radius;
	float coreRadius;
	/* Dead calm center. Inside it the flow field cuts out entirely, giving the
	 * funnel a still eye to read against. */
	float eyeRadius;
	float spinSpeed;
	float pullSpeed;
	float sinkDepth;
	float waveScale;
	float direction;
} sailingCurrent_t;

/*
 * Map-authored domain for a baked river simulation. Ordinary worldspawn water
 * remains the vanilla-compatible medium; this descriptor only identifies the
 * region and assets that jaPRO will progressively layer over it.
 */
typedef struct sailingRiver_s {
	vec3_t mins;
	vec3_t maxs;
	char name[MAX_QPATH];
	char bakeFile[MAX_QPATH];
	/*
	 * Fallback water materials to hide, parsed from a comma-separated
	 * baseShader key. A reach is often built from more than one water shader,
	 * and hiding only the first leaves the rest rendering as vanilla water
	 * through the solved surface. Names must be authored exactly as compiled,
	 * including the textures/ prefix; there is intentionally no
	 * canonicalization.
	 */
	char baseShaders[MAX_SAILING_RIVER_BASE_SHADERS][MAX_QPATH];
	int baseShaderCount;
	char meshShader[MAX_QPATH];
	float cellSize;
	/*
	 * Swimmer float spring, same contract as sailingOcean_t. Rivers previously
	 * had none of this and fell through to stock JKA water, which sinks the
	 * player at 60 units/second unless jump is held - wrong for water you are
	 * meant to be swept along by. buoyancyDepth is the submersion past which
	 * lift stops growing, and diveDrag is the fraction of water drag a fast
	 * descent does not pay, so a plunge off a fall keeps its momentum.
	 */
	float buoyancy;
	float buoyancyDamping;
	float buoyancyDepth;
	float diveDrag;
	/*
	 * Solver settings, per river rather than server-global: bed roughness and
	 * the capture walk's step allowance both depend on the geometry of this
	 * particular channel, and a map with two rivers cannot share one value.
	 * Replicated to clients through CS_SAILING_RIVERS so prediction solves
	 * with the same numbers the authoritative field does.
	 *
	 * friction is the Chezy drag coefficient. Uniform flow gives
	 * Fr = sqrt(slope / friction), so this is the dominant control on how much
	 * the bed prints through to the surface: roughly 0.02 for a smooth
	 * channel, 0.05 to 0.15 for a rough or boulder bed.
	 *
	 * captureHeadroom is how far the capture walk may climb when a neighbour's
	 * floor has risen. Keep it small: it only needs to clear terrain bumps,
	 * and a large value lets the walk climb a wall and capture the roof of a
	 * tunnel as riverbed. Zero forbids climbing entirely, which is correct for
	 * a channel that only ever runs downhill.
	 */
	float friction;
	float captureHeadroom;
} sailingRiver_t;

typedef enum sailingRiverSourceType_e {
	SAILING_RIVER_SOURCE_DISCHARGE,
	SAILING_RIVER_SOURCE_VELOCITY
} sailingRiverSourceType_t;

typedef struct sailingRiverSource_s {
	vec3_t mins;
	vec3_t maxs;
	char riverName[MAX_QPATH];
	vec2_t direction;
	sailingRiverSourceType_t type;
	/* Internal volume per second in map units cubed.  Entity parsers accept
	 * cubic feet per second and convert at registration time. */
	float discharge;
	/* Prescribed downstream speed. Used by velocity sources. */
	float speed;
	float surfaceHeight;
} sailingRiverSource_t;

typedef enum sailingRiverSinkType_e {
	SAILING_RIVER_SINK_OPEN,
	SAILING_RIVER_SINK_FIXED_STAGE,
	/*
	 * Outlet stage computed from uniform flow at the arriving discharge rather
	 * than authored, so tailwater rises with a flood instead of pinning the
	 * reach to one elevation at every discharge.
	 */
	SAILING_RIVER_SINK_NORMAL_DEPTH,
	/*
	 * Free overfall: a pit, a waterfall lip, a drop into a void. The brink
	 * itself is the control and the flow passes through critical depth there,
	 * so no slope or roughness enters - which is the point, because at a pit
	 * the bed falls away and any local slope estimate is meaningless.
	 */
	SAILING_RIVER_SINK_OVERFALL
} sailingRiverSinkType_t;

typedef struct sailingRiverSink_s {
	vec3_t mins;
	vec3_t maxs;
	char riverName[MAX_QPATH];
	vec2_t direction;
	sailingRiverSinkType_t type;
	float surfaceHeight;
} sailingRiverSink_t;

/*
 * Dense, caller-owned development/bake field. Momentum is stored instead of
 * velocity so the shallow-water update remains conservative; serialized river
 * assets can still store velocity after dividing by depth.
 */
typedef struct sailingRiverCell_s {
	float bedHeight;
	/*
	 * Solid directly above this cell's floor, or the top of the river brush
	 * where the channel is open. The capture walk must never start a trace
	 * above it: a wall that reaches the ceiling is shorter than the walk's
	 * climb allowance, so without this limit the walk climbs the wall, lands
	 * on the roof, and records the roof as riverbed for the whole tunnel.
	 */
	float ceilingHeight;
	float depth;
	vec2_t momentum;
	unsigned int flags;
} sailingRiverCell_t;

typedef struct sailingRiverField_s {
	vec2_t origin;
	float cellSize;
	int width;
	int height;
	int cellCount;
	int stepCount;
	float simulationTime;
	/*
	 * Guard counters from the most recent step, for diagnostics only. Both
	 * should be zero in a trustworthy solve: clampedCells means the 4096
	 * unit/second momentum ceiling fired, so that step silently did not
	 * conserve momentum, and resetCells means a cell went non-finite and was
	 * zeroed. Either one makes the field something other than a solution of
	 * the equations, so cross-section numbers taken from it are meaningless.
	 * Ordinary drying is not counted; it is normal.
	 */
	int clampedCells;
	int resetCells;
	/*
	 * Steady-state residual from the most recent step: the mean rate of depth
	 * change over wet cells, and the mean depth to normalize it against.
	 * Convergence is deliberately measured rather than assumed from a step
	 * count, because the time to steady state is set by transit time and so
	 * varies enormously with roughness and discharge - 4000 steps was ample at
	 * friction 0.02 and badly short at 0.30 on the same map.
	 */
	float depthResidual;
	float meanDepth;
	/*
	 * Total water volume and its rate of change. This, not depthResidual, is
	 * what convergence is tested against: the residual is a mean ABSOLUTE
	 * depth change, so a reach that has reached steady state but is sloshing
	 * reports a large residual forever. Sloshing conserves volume, so dV/dt
	 * ignores it and only reports genuine filling or draining.
	 */
	float volume;
	float volumeRate;
	sailingRiverCell_t *cells;
	sailingRiverCell_t *scratch;
} sailingRiverField_t;

/*
 * Baked field asset. The cell array is POD and contiguous, so it serializes
 * directly after this header with no per-cell marshalling. Storing the bed as
 * well as the water means a load needs no BSP tracing at all, which is the
 * expensive half of a build.
 */
#define SAILING_RIVER_BAKE_MAGIC ( ( 'R' << 24 ) | ( 'I' << 16 ) \
	| ( 'V' << 8 ) | 'R' )
#define SAILING_RIVER_BAKE_VERSION 1

typedef struct sailingRiverBakeHeader_s {
	int magic;
	int version;
	int width;
	int height;
	int cellCount;
	float cellSize;
	float origin[2];
	float simulationTime;
	int stepCount;
} sailingRiverBakeHeader_t;

typedef struct sailingRiverSample_s {
	float bedHeight;
	/* Reported so a mapper can tell a captured floor from a captured roof, and
	 * see when the solved surface has risen through the rock above it. */
	float ceilingHeight;
	float surfaceHeight;
	float depth;
	vec2_t velocity;
	unsigned int flags;
} sailingRiverSample_t;

typedef struct sailingBoatHull_s {
	vec3_t centerPoint;
	vec3_t bowPoint;
	vec3_t sternPoint;
	vec3_t leftPoint;
	vec3_t rightPoint;
	vec3_t centerWaterVelocity;
	vec2_t averageSlope;
	float averageVerticalVelocity;
	float centerHeight;
	float bowHeight;
	float sternHeight;
	float leftHeight;
	float rightHeight;
	float pitch;
	float roll;
} sailingBoatHull_t;

typedef struct sailingBoatLoad_s {
	vec3_t waterlinePoint[SAILING_DAMAGE_HULL_SAMPLES];
	float hullDepth[SAILING_DAMAGE_HULL_SAMPLES];
	float sailLateralAcceleration;
	float hullLateralAcceleration;
	float waveLateralAcceleration;
	float lateralLoad;
} sailingBoatLoad_t;

void BG_SailingClearOceans( void );
void BG_SailingOceanDefaults( sailingOcean_t *ocean );
int BG_SailingAddOcean( const sailingOcean_t *ocean );
int BG_SailingOceanCount( void );
const sailingOcean_t *BG_SailingOcean( int index );
const sailingOcean_t *BG_SailingOceanForPoint( float x, float y );
void BG_SailingShoreDefaults( sailingShore_t *shore );
int BG_SailingAddShore( const sailingShore_t *shore );
int BG_SailingShoreCount( void );
const sailingShore_t *BG_SailingShore( int index );
void BG_SailingShoreInfluence( const sailingOcean_t *ocean, float x, float y,
	float *waveScale, float *beachFoam, float *cliffFoam );
void BG_SailingCurrentDefaults( sailingCurrent_t *current );
int BG_SailingAddCurrent( const sailingCurrent_t *current );
int BG_SailingCurrentCount( void );
const sailingCurrent_t *BG_SailingCurrent( int index );
void BG_SailingRiverDefaults( sailingRiver_t *river );
/* Fills baseShaders from a comma-separated list; blanks and overflow are
 * skipped rather than treated as errors. */
void BG_SailingRiverSetBaseShaders( sailingRiver_t *river, const char *list );
int BG_SailingAddRiver( const sailingRiver_t *river );
int BG_SailingRiverCount( void );
const sailingRiver_t *BG_SailingRiver( int index );
const sailingRiver_t *BG_SailingRiverForPoint( const vec3_t point );
void BG_SailingBuildRiverStateString( const sailingRiver_t *river,
	char *buffer, int bufferSize );
qboolean BG_SailingApplyRiverStateString( int index, const char *state );
qboolean BG_SailingSetRiverSolverSettings( int index, float friction,
	float captureHeadroom );
int BG_SailingRiverIndexForName( const char *name );
void BG_SailingRiverSourceDefaults( sailingRiverSource_t *source );
int BG_SailingAddRiverSource( const sailingRiverSource_t *source );
int BG_SailingRiverSourceCount( void );
const sailingRiverSource_t *BG_SailingRiverSource( int index );
void BG_SailingRiverSinkDefaults( sailingRiverSink_t *sink );
int BG_SailingAddRiverSink( const sailingRiverSink_t *sink );
int BG_SailingRiverSinkCount( void );
const sailingRiverSink_t *BG_SailingRiverSink( int index );
qboolean BG_SailingRiverFieldInit( sailingRiverField_t *field,
	const sailingRiver_t *river, sailingRiverCell_t *cells,
	sailingRiverCell_t *scratch, int capacity );
void BG_SailingRiverFieldCellCenter( const sailingRiverField_t *field,
	int x, int y, vec2_t center );
/*
 * Downward trace at a column, supplied by the caller because cgame and game
 * reach the collision model through different traps. Reports the hit height,
 * whether anything was hit at all, and whether the start point was already
 * inside solid.
 */
typedef void (*sailingRiverFloorTrace_t)( float x, float y, float startZ,
	float endZ, float *hitZ, qboolean *hit, qboolean *startSolid );
/*
 * Captures the bed by walking the channel outward from the source rather than
 * tracing every column independently from the top of the river brush.
 *
 * Tracing from the top takes the first solid it meets, so any ceiling, bridge
 * or overhang is recorded as riverbed and the channel underneath it never
 * exists. Seeding at the source - which the mapper draws inside the channel,
 * so its floor is unambiguous - and then tracing each neighbour from just
 * above its predecessor's floor keeps every trace inside the open space the
 * water actually occupies. Roofs above and voids below both stop mattering.
 *
 * Only geometry connected to the source is captured, which also keeps
 * disconnected basins elsewhere in the brush out of the solve.
 */
void BG_SailingRiverFieldCaptureBed( sailingRiverField_t *field,
	const sailingRiver_t *river, sailingRiverFloorTrace_t traceFloor,
	float headroomStep, int *capturedCount );
qboolean BG_SailingRiverFieldSetBed( sailingRiverField_t *field,
	int x, int y, float bedHeight, qboolean active );
/* prefill false starts the field dry, which is the default: see the note on
 * the definition for why blanket-filling is only safe on a tight brush. */
void BG_SailingRiverFieldInitializeWater( sailingRiverField_t *field,
	const sailingRiver_t *river, qboolean prefill );
float BG_SailingRiverFieldTimeStep( const sailingRiverField_t *field,
	float gravity, float cfl, float maximumTimeStep );
/* friction is the same Chezy coefficient the step uses; a normal-depth outlet
 * needs it to solve uniform flow. Pass 0 where it is not known yet, and that
 * outlet degrades to transmissive rather than guessing. */
void BG_SailingRiverFieldApplyBoundaries( sailingRiverField_t *field,
	const sailingRiver_t *river, float timeStep, float friction );
void BG_SailingRiverFieldApplyBoundariesOverride( sailingRiverField_t *field,
	const sailingRiver_t *river, float timeStep, float dischargeOverride,
	float friction );
qboolean BG_SailingRiverFieldStep( sailingRiverField_t *field,
	float timeStep, float gravity, float friction );
/*
 * True once the field is neither filling nor draining: volume is changing by
 * less than tolerance of the inflow discharge. A caller should require this
 * to hold over several consecutive checks rather than trusting one step.
 */
qboolean BG_SailingRiverFieldConverged( const sailingRiverField_t *field,
	float inflowDischarge, float tolerance );
/* Total authored inflow for a river, honouring a development override. */
float BG_SailingRiverInflowDischarge( const sailingRiver_t *river,
	float dischargeOverride );
void BG_SailingRiverFieldFillBakeHeader( const sailingRiverField_t *field,
	sailingRiverBakeHeader_t *header );
/* Rejects a bake that does not describe the field the descriptor asks for, so
 * a stale asset against an edited map falls back to a fresh build. */
qboolean BG_SailingRiverBakeHeaderValid( const sailingRiverBakeHeader_t *header,
	const sailingRiverField_t *field );
qboolean BG_SailingRiverFieldSample( const sailingRiverField_t *field,
	float x, float y, sailingRiverSample_t *sample );
/*
 * The asset path a river's bake is written to and read from.
 *
 * Shared because the writer and both loaders have to agree exactly. They used
 * to read the authored `bake` key directly, so a bake written anywhere else -
 * including by passing a filename to sailingRiverBake - was written, reported
 * as a success, and then never loaded by anything. Deriving the path in one
 * place makes that class of mistake impossible.
 *
 * An authored key wins. Otherwise the path is derived from the map and river
 * names, so a river needs no `bake` key at all to have a working bake:
 *
 *   maps/<map>_<river>.river
 *
 * rawMapName is "maps/<map>", which is what both modules already cache as
 * level.rawmapname and cgs.rawmapname.
 */
void BG_SailingRiverBakePath( const sailingRiver_t *river,
	const char *rawMapName, char *out, int outSize );
void BG_SailingSetRuntimeRiverField( int riverIndex,
	const sailingRiverField_t *field );
void BG_SailingClearRuntimeRiverField( void );
const sailingRiver_t *BG_SailingRuntimeRiver( void );
qboolean BG_SailingRuntimeRiverSample( float x, float y,
	sailingRiverSample_t *sample );
void BG_SailingCurrentInfluence( const sailingOcean_t *ocean, float x, float y,
	float *surfaceOffset, float *waveScale, vec2_t surfaceGradient,
	vec2_t waveScaleGradient, float *foam );
void BG_SailingEvaluateWave( const sailingOcean_t *ocean, float x, float y, int time,
	float *height, vec3_t normal, vec3_t waterVelocity );
const sailingSea_t *BG_SailingSea( const sailingOcean_t *ocean );
qboolean BG_SailingEvaluateGerstnerWave( const sailingOcean_t *ocean,
	float x, float y, int time, float *height, vec3_t normal,
	vec3_t waterVelocity );
qboolean BG_SailingGetGerstnerRenderWaves( const sailingOcean_t *ocean,
	sailingGerstnerRenderWave_t *waves, int maxWaves, int *waveCount,
	float *choppiness );
void BG_SailingEvaluateBoatHull( const sailingOcean_t *ocean, const vec3_t origin,
	float yaw, float hullLength, float hullWidth, int time, sailingBoatHull_t *hull );
void BG_SailingEvaluateBoatLoad( const sailingOcean_t *ocean,
	const sailingBoatHull_t *hull, const vec3_t origin, const vec3_t velocity,
	const vec3_t angles, float hullLength, float hullWidth, float floatHeight,
	float lateralDrag, float sailArea, float sailTrim, int time,
	sailingBoatLoad_t *load );
float BG_SailingImpactDamageForRate( float impactRate, float threshold );
float BG_SailingUpdateHullStress( float stress, float lateralLoad, float dt );
float BG_SailingHullStressDamageRate( float stress, float lateralLoad );
void BG_SailingCurrentVelocityAt( const sailingOcean_t *ocean, float x, float y,
	int time, vec3_t currentVelocity );
void BG_SailingCurrentAdvectionPhase( float referenceTile, float dragFraction,
	float peakSpeed, int time, vec2_t offsets, vec2_t weights );
void BG_SailingCurrentSurfaceFlowAt( const sailingOcean_t *ocean, float x,
	float y, vec2_t flow );
void BG_SailingWindVelocity( const sailingOcean_t *ocean, int time,
	vec3_t windVelocity );
void BG_SailingSetWaterspout( const sailingWaterspout_t *waterspout );
void BG_SailingClearWaterspout( void );
const sailingWaterspout_t *BG_SailingWaterspout( void );
float BG_SailingWaterspoutWindAt( const vec3_t point, int time,
	vec3_t windVelocity );
void BG_SailingWindVelocityAt( const sailingOcean_t *ocean,
	const vec3_t point, int time, vec3_t windVelocity );
float BG_SailingResolveOceanSpeed( float speedAuthored, float wavelength );
qboolean BG_SailingSetOceanWeather( int index, float currentScale,
	float amplitude, float steepness, float wavelength, float slosh,
	float speedAuthored, float rainIntensity, float waveDirectionX,
	float waveDirectionY );
void BG_SailingBuildOceanWeatherString( const sailingOcean_t *ocean,
	char *buffer, int bufferSize );
qboolean BG_SailingApplyOceanWeatherString( int index, const char *state );
void BG_SailingSailAcceleration( const vec3_t apparentWind, float sailAngle,
	float sailArea, vec3_t acceleration, float *angleOfAttack,
	float *liftCoefficient, float *dragCoefficient );
int BG_SailingEncodeBoatControls( float trim, float rudder );
float BG_SailingDecodeTrim( int encodedTrim );
float BG_SailingDecodeRudder( int encodedControls );
