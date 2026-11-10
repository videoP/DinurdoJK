/*
===========================================================================
Deterministic sailing-ocean wave evaluation.

This source is compiled into both game and cgame.  Each VM therefore owns the
same map-authored ocean definitions and evaluates exactly the same height field.
===========================================================================
*/

#include "bg_sailing.h"
#include "bg_weather.h"

#define SAILING_HULL_CACHE_SIZE 8
#define SAILING_GERSTNER_INVERSION_STEPS 3

typedef struct sailingGerstnerWave_s {
	vec2_t direction;
	float amplitude;
	float waveNumber;
	float angularFrequency;
	float phase;
} sailingGerstnerWave_t;

typedef struct sailingGerstnerState_s {
	qboolean initialized;
	unsigned int revision;
	int waveCount;
	float horizontalNormalization;
	sailingGerstnerWave_t wave[SAILING_GERSTNER_MAX_WAVES];
} sailingGerstnerState_t;

typedef struct sailingHullCache_s {
	qboolean valid;
	const sailingOcean_t *ocean;
	vec3_t origin;
	float yaw;
	float hullLength;
	float hullWidth;
	int time;
	sailingBoatHull_t hull;
} sailingHullCache_t;

static sailingOcean_t bgSailingOceans[MAX_SAILING_OCEANS];
static sailingGerstnerState_t bgSailingGerstnerStates[MAX_SAILING_OCEANS];
static int bgNumSailingOceans;
static sailingShore_t bgSailingShores[MAX_SAILING_SHORES];
static int bgNumSailingShores;
static sailingCurrent_t bgSailingCurrents[MAX_SAILING_CURRENTS];
static int bgNumSailingCurrents;
static sailingRiver_t bgSailingRivers[MAX_SAILING_RIVERS];
static int bgNumSailingRivers;
static sailingRiverSource_t bgSailingRiverSources[MAX_SAILING_RIVER_SOURCES];
static int bgNumSailingRiverSources;
static sailingRiverSink_t bgSailingRiverSinks[MAX_SAILING_RIVER_SINKS];
static int bgNumSailingRiverSinks;
static const sailingRiverField_t *bgSailingRuntimeRiverField;
static int bgSailingRuntimeRiverIndex = -1;
static sailingWaterspout_t bgSailingWaterspout;
static qboolean bgSailingWaterspoutValid;
/* Union AABB of every shore's XY footprint. Recomputed only when shores change,
 * so the per-vertex hot path can reject open-water points (the overwhelming
 * majority) with a single box test instead of scanning all shores. */
static vec2_t bgSailingShoreBoundsMins;
static vec2_t bgSailingShoreBoundsMaxs;
static vec2_t bgSailingCurrentBoundsMins;
static vec2_t bgSailingCurrentBoundsMaxs;

static void BG_SailingRebuildShoreBounds( void ) {
	int i;

	if ( bgNumSailingShores <= 0 ) {
		/* Empty box that rejects everything. */
		bgSailingShoreBoundsMins[0] = bgSailingShoreBoundsMins[1] = 1.0f;
		bgSailingShoreBoundsMaxs[0] = bgSailingShoreBoundsMaxs[1] = -1.0f;
		return;
	}
	bgSailingShoreBoundsMins[0] = bgSailingShoreBoundsMins[1] = 1e30f;
	bgSailingShoreBoundsMaxs[0] = bgSailingShoreBoundsMaxs[1] = -1e30f;
	for ( i = 0; i < bgNumSailingShores; i++ ) {
		const sailingShore_t *shore = &bgSailingShores[i];
		if ( shore->mins[0] < bgSailingShoreBoundsMins[0] )
			bgSailingShoreBoundsMins[0] = shore->mins[0];
		if ( shore->mins[1] < bgSailingShoreBoundsMins[1] )
			bgSailingShoreBoundsMins[1] = shore->mins[1];
		if ( shore->maxs[0] > bgSailingShoreBoundsMaxs[0] )
			bgSailingShoreBoundsMaxs[0] = shore->maxs[0];
		if ( shore->maxs[1] > bgSailingShoreBoundsMaxs[1] )
			bgSailingShoreBoundsMaxs[1] = shore->maxs[1];
	}
}

static void BG_SailingRebuildCurrentBounds( void ) {
	int i;

	if ( bgNumSailingCurrents <= 0 ) {
		bgSailingCurrentBoundsMins[0] = bgSailingCurrentBoundsMins[1] = 1.0f;
		bgSailingCurrentBoundsMaxs[0] = bgSailingCurrentBoundsMaxs[1] = -1.0f;
		return;
	}
	bgSailingCurrentBoundsMins[0] = bgSailingCurrentBoundsMins[1] = 1e30f;
	bgSailingCurrentBoundsMaxs[0] = bgSailingCurrentBoundsMaxs[1] = -1e30f;
	for ( i = 0; i < bgNumSailingCurrents; i++ ) {
		const sailingCurrent_t *current = &bgSailingCurrents[i];
		float minX = current->origin[0] - current->radius;
		float minY = current->origin[1] - current->radius;
		float maxX = current->origin[0] + current->radius;
		float maxY = current->origin[1] + current->radius;

		if ( minX < bgSailingCurrentBoundsMins[0] )
			bgSailingCurrentBoundsMins[0] = minX;
		if ( minY < bgSailingCurrentBoundsMins[1] )
			bgSailingCurrentBoundsMins[1] = minY;
		if ( maxX > bgSailingCurrentBoundsMaxs[0] )
			bgSailingCurrentBoundsMaxs[0] = maxX;
		if ( maxY > bgSailingCurrentBoundsMaxs[1] )
			bgSailingCurrentBoundsMaxs[1] = maxY;
	}
}
static sailingHullCache_t bgSailingHullCache[SAILING_HULL_CACHE_SIZE];
static int bgSailingHullCacheNext;

static void BG_SailingInvalidateHullCache( void ) {
	memset( bgSailingHullCache, 0, sizeof( bgSailingHullCache ) );
	bgSailingHullCacheNext = 0;
}

void BG_SailingClearOceans( void ) {
	memset( bgSailingOceans, 0, sizeof( bgSailingOceans ) );
	memset( bgSailingGerstnerStates, 0, sizeof( bgSailingGerstnerStates ) );
	bgNumSailingOceans = 0;
	memset( bgSailingShores, 0, sizeof( bgSailingShores ) );
	bgNumSailingShores = 0;
	memset( bgSailingCurrents, 0, sizeof( bgSailingCurrents ) );
	bgNumSailingCurrents = 0;
	memset( bgSailingRivers, 0, sizeof( bgSailingRivers ) );
	bgNumSailingRivers = 0;
	memset( bgSailingRiverSources, 0, sizeof( bgSailingRiverSources ) );
	bgNumSailingRiverSources = 0;
	memset( bgSailingRiverSinks, 0, sizeof( bgSailingRiverSinks ) );
	bgNumSailingRiverSinks = 0;
	BG_SailingClearRuntimeRiverField();
	BG_SailingClearWaterspout();
	BG_SailingRebuildShoreBounds();
	BG_SailingRebuildCurrentBounds();
	BG_SailingInvalidateHullCache();
}

void BG_SailingOceanDefaults( sailingOcean_t *ocean ) {
	if ( !ocean ) {
		return;
	}

	memset( ocean, 0, sizeof( *ocean ) );
	ocean->amplitude = 128.0f;
	ocean->wavelength = 1400.0f;
	ocean->speedAuthored = 180.0f;
	ocean->speed = 180.0f;
	ocean->steepness = 0.65f;
	ocean->slosh = 0.0f;
	ocean->direction[0] = 1.0f;
	ocean->rainIntensity = 0.0f;
	ocean->currentScale = 0.25f;
	ocean->textureDriftScale = 0.25f;
	ocean->swimControlScale = 1.0f;
	ocean->buoyancy = 12.0f;
	ocean->buoyancyDamping = 7.0f;
	ocean->buoyancyDepth = 48.0f;
	ocean->diveDrag = 0.2f;
	ocean->gridSpacing = 96.0f;
	ocean->renderDistance = 4096.0f;
	ocean->textureScale = 256.0f;
	VectorSet( ocean->fogColor, 0.45f, 0.6f, 0.75f );
	ocean->fogDistance = 300.0f;
	ocean->waveSeed = 85619173u;
	ocean->gerstnerWaveCount = 8;
	ocean->horizontalDisplacementScale = 0.0f;
	Q_strncpyz( ocean->meshShader, "textures/sailing/ocean_mesh", sizeof( ocean->meshShader ) );
}

int BG_SailingAddOcean( const sailingOcean_t *ocean ) {
	sailingOcean_t *stored;
	float directionLength;

	if ( !ocean || bgNumSailingOceans >= MAX_SAILING_OCEANS ) {
		return -1;
	}

	stored = &bgSailingOceans[bgNumSailingOceans];
	*stored = *ocean;

	stored->amplitude = Com_Clamp( 0.0f, SAILING_MAX_AMPLITUDE, stored->amplitude );
	stored->wavelength = Com_Clamp( 64.0f, 32768.0f, stored->wavelength );
	stored->speedAuthored = Com_Clamp( -4096.0f, 4096.0f, stored->speedAuthored );
	stored->speed = BG_SailingResolveOceanSpeed( stored->speedAuthored,
		stored->wavelength );
	stored->steepness = Com_Clamp( 0.0f, 1.0f, stored->steepness );
	stored->slosh = Com_Clamp( 0.0f, 1.0f, stored->slosh );
	stored->rainIntensity = Com_Clamp( 0.0f, 1.0f, stored->rainIntensity );
	stored->currentScale = Com_Clamp( -2.0f, 2.0f, stored->currentScale );
	stored->textureDriftScale = Com_Clamp( -2.0f, 2.0f, stored->textureDriftScale );
	stored->swimControlScale = Com_Clamp( 0.0f, 1.0f, stored->swimControlScale );
	stored->buoyancy = Com_Clamp( 0.0f, 64.0f, stored->buoyancy );
	stored->buoyancyDamping = Com_Clamp( 0.0f, 32.0f, stored->buoyancyDamping );
	/* A saturation depth below one body length would make the lift constant
	 * even while wading, which robs the surface spring of its settling range. */
	stored->buoyancyDepth = Com_Clamp( 16.0f, 1024.0f, stored->buoyancyDepth );
	stored->diveDrag = Com_Clamp( 0.0f, 1.0f, stored->diveDrag );
	stored->gridSpacing = Com_Clamp( 32.0f, 1024.0f, stored->gridSpacing );
	stored->renderDistance = Com_Clamp( 512.0f, 16384.0f, stored->renderDistance );
	stored->textureScale = Com_Clamp( 16.0f, 8192.0f, stored->textureScale );
	stored->fogColor[0] = Com_Clamp( 0.0f, 1.0f, stored->fogColor[0] );
	stored->fogColor[1] = Com_Clamp( 0.0f, 1.0f, stored->fogColor[1] );
	stored->fogColor[2] = Com_Clamp( 0.0f, 1.0f, stored->fogColor[2] );
	stored->fogDistance = Com_Clamp( 64.0f, 4096.0f, stored->fogDistance );
	if ( !stored->waveSeed ) {
		stored->waveSeed = 85619173u;
	}
	stored->gerstnerWaveCount = (int)Com_Clamp( 2.0f,
		(float)SAILING_GERSTNER_MAX_WAVES,
		(float)stored->gerstnerWaveCount );
	stored->horizontalDisplacementScale = Com_Clamp( 0.0f, 1.0f,
		stored->horizontalDisplacementScale );

	directionLength = sqrtf( stored->direction[0] * stored->direction[0]
		+ stored->direction[1] * stored->direction[1] );
	if ( directionLength <= 0.001f ) {
		stored->direction[0] = 1.0f;
		stored->direction[1] = 0.0f;
	}
	else {
		stored->direction[0] /= directionLength;
		stored->direction[1] /= directionLength;
	}

	BG_SailingInvalidateHullCache();
	memset( &bgSailingGerstnerStates[bgNumSailingOceans], 0,
		sizeof( bgSailingGerstnerStates[bgNumSailingOceans] ) );
	return bgNumSailingOceans++;
}

int BG_SailingOceanCount( void ) {
	return bgNumSailingOceans;
}

const sailingOcean_t *BG_SailingOcean( int index ) {
	if ( index < 0 || index >= bgNumSailingOceans ) {
		return NULL;
	}

	return &bgSailingOceans[index];
}

const sailingOcean_t *BG_SailingOceanForPoint( float x, float y ) {
	int i;

	for ( i = 0; i < bgNumSailingOceans; i++ ) {
		const sailingOcean_t *ocean = &bgSailingOceans[i];
		if ( x >= ocean->mins[0] && x <= ocean->maxs[0]
			&& y >= ocean->mins[1] && y <= ocean->maxs[1] ) {
			return ocean;
		}
	}

	return NULL;
}

void BG_SailingShoreDefaults( sailingShore_t *shore ) {
	if ( !shore ) {
		return;
	}
	memset( shore, 0, sizeof( *shore ) );
	shore->direction[0] = 1.0f;
	shore->oceanIndex = 0;
	shore->type = SAILING_SHORE_BEACH;
}

int BG_SailingAddShore( const sailingShore_t *shore ) {
	sailingShore_t *stored;
	float directionLength;

	if ( !shore || bgNumSailingShores >= MAX_SAILING_SHORES
		|| shore->oceanIndex < 0 || shore->oceanIndex >= MAX_SAILING_OCEANS ) {
		return -1;
	}
	stored = &bgSailingShores[bgNumSailingShores];
	*stored = *shore;
	directionLength = sqrtf( stored->direction[0] * stored->direction[0]
		+ stored->direction[1] * stored->direction[1] );
	if ( directionLength <= 0.001f ) {
		stored->direction[0] = 1.0f;
		stored->direction[1] = 0.0f;
	}
	else {
		stored->direction[0] /= directionLength;
		stored->direction[1] /= directionLength;
	}
	bgNumSailingShores++;
	BG_SailingRebuildShoreBounds();
	BG_SailingInvalidateHullCache();
	return bgNumSailingShores - 1;
}

int BG_SailingShoreCount( void ) {
	return bgNumSailingShores;
}

const sailingShore_t *BG_SailingShore( int index ) {
	if ( index < 0 || index >= bgNumSailingShores ) {
		return NULL;
	}
	return &bgSailingShores[index];
}

void BG_SailingCurrentDefaults( sailingCurrent_t *current ) {
	if ( !current ) {
		return;
	}
	memset( current, 0, sizeof( *current ) );
	current->oceanIndex = 0;
	current->type = SAILING_CURRENT_VORTEX;
	current->radius = 768.0f;
	current->coreRadius = current->radius * SAILING_CURRENT_CORE_FRACTION;
	current->eyeRadius = current->coreRadius * 0.15f;
	current->spinSpeed = 480.0f;
	current->pullSpeed = 120.0f;
	current->sinkDepth = SAILING_CURRENT_DERIVE_SINK;
	current->waveScale = 0.25f;
	current->direction = 1.0f;
}

int BG_SailingAddCurrent( const sailingCurrent_t *current ) {
	sailingCurrent_t *stored;

	if ( !current || bgNumSailingCurrents >= MAX_SAILING_CURRENTS
		|| current->oceanIndex < 0
		|| current->oceanIndex >= MAX_SAILING_OCEANS
		|| current->type != SAILING_CURRENT_VORTEX ) {
		return -1;
	}
	stored = &bgSailingCurrents[bgNumSailingCurrents];
	*stored = *current;
	stored->radius = Com_Clamp( 32.0f, 32768.0f, stored->radius );
	stored->coreRadius = Com_Clamp( 8.0f, stored->radius, stored->coreRadius );
	stored->eyeRadius = Com_Clamp( 0.0f, stored->coreRadius * 0.9f,
		stored->eyeRadius );
	stored->spinSpeed = Com_Clamp( 0.0f, 4096.0f, fabsf( stored->spinSpeed ) );
	stored->pullSpeed = Com_Clamp( 0.0f, 4096.0f, fabsf( stored->pullSpeed ) );
	if ( stored->sinkDepth < 0.0f ) {
		/* Rankine free surface: a core spinning at spinSpeed depresses the
		 * center by omega^2 * Rc^2 / g, and omega = spinSpeed / Rc, so the core
		 * radius cancels out. Deriving the depth stops spin and sink from being
		 * tuned into visible disagreement. */
		stored->sinkDepth = stored->spinSpeed * stored->spinSpeed
			/ SAILING_GRAVITY;
		stored->sinkDepth = fminf( stored->sinkDepth,
			stored->radius * SAILING_CURRENT_MAX_SINK_FRACTION );
	}
	stored->sinkDepth = Com_Clamp( 0.0f, 16384.0f, stored->sinkDepth );
	stored->waveScale = Com_Clamp( 0.0f, 1.0f, stored->waveScale );
	stored->direction = stored->direction < 0.0f ? -1.0f : 1.0f;
	bgNumSailingCurrents++;
	BG_SailingRebuildCurrentBounds();
	BG_SailingInvalidateHullCache();
	return bgNumSailingCurrents - 1;
}

int BG_SailingCurrentCount( void ) {
	return bgNumSailingCurrents;
}

const sailingCurrent_t *BG_SailingCurrent( int index ) {
	if ( index < 0 || index >= bgNumSailingCurrents ) {
		return NULL;
	}
	return &bgSailingCurrents[index];
}

void BG_SailingRiverDefaults( sailingRiver_t *river ) {
	if ( !river ) {
		return;
	}

	memset( river, 0, sizeof( *river ) );
	river->cellSize = 16.0f;
	/* Matches the ocean defaults; a river swimmer should float the same way an
	 * ocean swimmer does. */
	river->buoyancy = 12.0f;
	river->buoyancyDamping = 7.0f;
	river->buoyancyDepth = 48.0f;
	river->diveDrag = 0.2f;
	river->friction = 0.12f;
	/* Small on purpose. Enough for terrain bumps, nowhere near enough to
	 * climb a wall onto a tunnel roof. */
	river->captureHeadroom = 8.0f;
	Q_strncpyz( river->meshShader, "textures/sailing/river_mesh",
		sizeof( river->meshShader ) );
}

void BG_SailingRiverSetBaseShaders( sailingRiver_t *river, const char *list ) {
	const char *cursor = list;

	if ( !river ) {
		return;
	}
	memset( river->baseShaders, 0, sizeof( river->baseShaders ) );
	river->baseShaderCount = 0;
	if ( !list || !list[0] ) {
		return;
	}
	while ( *cursor && river->baseShaderCount
		< MAX_SAILING_RIVER_BASE_SHADERS ) {
		char name[MAX_QPATH];
		int length = 0;

		/* Tolerate spaces around the separators so "a, b" authors the same as
		 * "a,b" - a mapper typing a list should not have to be careful. */
		while ( *cursor == ',' || *cursor == ' ' || *cursor == '\t' ) {
			cursor++;
		}
		while ( *cursor && *cursor != ','
			&& length < (int)sizeof( name ) - 1 ) {
			name[length++] = *cursor++;
		}
		while ( length > 0 && ( name[length - 1] == ' '
			|| name[length - 1] == '\t' ) ) {
			length--;
		}
		name[length] = '\0';
		if ( !length ) {
			continue;
		}
		Q_strncpyz( river->baseShaders[river->baseShaderCount], name,
			sizeof( river->baseShaders[0] ) );
		river->baseShaderCount++;
	}
}

int BG_SailingAddRiver( const sailingRiver_t *river ) {
	int i;
	sailingRiver_t *stored;

	if ( !river || bgNumSailingRivers >= MAX_SAILING_RIVERS
		|| !river->name[0]
		|| river->maxs[0] <= river->mins[0]
		|| river->maxs[1] <= river->mins[1]
		|| river->maxs[2] <= river->mins[2] ) {
		return -1;
	}
	for ( i = 0; i < bgNumSailingRivers; i++ ) {
		if ( !Q_stricmp( bgSailingRivers[i].name, river->name ) ) {
			return -1;
		}
	}

	stored = &bgSailingRivers[bgNumSailingRivers];
	*stored = *river;
	stored->cellSize = Com_Clamp( 4.0f, 256.0f, stored->cellSize );
	/* Same ranges the ocean clamps to, so a river cannot be authored into a
	 * float spring that launches the player or oscillates. */
	stored->buoyancy = Com_Clamp( 0.0f, 64.0f, stored->buoyancy );
	stored->buoyancyDamping = Com_Clamp( 0.0f, 32.0f, stored->buoyancyDamping );
	stored->buoyancyDepth = Com_Clamp( 16.0f, 1024.0f, stored->buoyancyDepth );
	stored->diveDrag = Com_Clamp( 0.0f, 1.0f, stored->diveDrag );
	stored->friction = Com_Clamp( 0.001f, 2.0f, stored->friction );
	stored->captureHeadroom = Com_Clamp( 0.0f, 256.0f,
		stored->captureHeadroom );
	return bgNumSailingRivers++;
}

int BG_SailingRiverCount( void ) {
	return bgNumSailingRivers;
}

const sailingRiver_t *BG_SailingRiver( int index ) {
	if ( index < 0 || index >= bgNumSailingRivers ) {
		return NULL;
	}

	return &bgSailingRivers[index];
}

const sailingRiver_t *BG_SailingRiverForPoint( const vec3_t point ) {
	int i;

	if ( !point ) {
		return NULL;
	}

	for ( i = 0; i < bgNumSailingRivers; i++ ) {
		const sailingRiver_t *river = &bgSailingRivers[i];

		if ( point[0] >= river->mins[0] && point[0] <= river->maxs[0]
			&& point[1] >= river->mins[1] && point[1] <= river->maxs[1]
			&& point[2] >= river->mins[2] && point[2] <= river->maxs[2] ) {
			return river;
		}
	}

	return NULL;
}

/*
 * Solver settings replicated to clients, mirroring the ocean weather string.
 * Only the tunable numbers travel; bounds, shaders and cellSize come from the
 * entity the client spawns for itself.
 */
void BG_SailingBuildRiverStateString( const sailingRiver_t *river,
	char *buffer, int bufferSize ) {
	if ( !buffer || bufferSize <= 0 ) {
		return;
	}
	if ( !river ) {
		buffer[0] = '\0';
		return;
	}
	Com_sprintf( buffer, bufferSize, "%.4f %.2f",
		river->friction, river->captureHeadroom );
}

qboolean BG_SailingApplyRiverStateString( int index, const char *state ) {
	float friction;
	float captureHeadroom;
	int parsed;

	if ( !state || !state[0] || index < 0 || index >= bgNumSailingRivers ) {
		return qfalse;
	}
	friction = bgSailingRivers[index].friction;
	captureHeadroom = bgSailingRivers[index].captureHeadroom;
	parsed = sscanf( state, "%f %f", &friction, &captureHeadroom );
	if ( parsed < 1 ) {
		return qfalse;
	}
	bgSailingRivers[index].friction = Com_Clamp( 0.001f, 2.0f, friction );
	if ( parsed >= 2 ) {
		bgSailingRivers[index].captureHeadroom = Com_Clamp( 0.0f, 256.0f,
			captureHeadroom );
	}
	return qtrue;
}

qboolean BG_SailingSetRiverSolverSettings( int index, float friction,
	float captureHeadroom ) {
	if ( index < 0 || index >= bgNumSailingRivers ) {
		return qfalse;
	}
	bgSailingRivers[index].friction = Com_Clamp( 0.001f, 2.0f, friction );
	bgSailingRivers[index].captureHeadroom = Com_Clamp( 0.0f, 256.0f,
		captureHeadroom );
	return qtrue;
}

int BG_SailingRiverIndexForName( const char *name ) {
	int i;

	if ( !name || !name[0] ) {
		return -1;
	}
	for ( i = 0; i < bgNumSailingRivers; i++ ) {
		if ( !Q_stricmp( bgSailingRivers[i].name, name ) ) {
			return i;
		}
	}
	return -1;
}

void BG_SailingRiverSourceDefaults( sailingRiverSource_t *source ) {
	if ( !source ) {
		return;
	}
	memset( source, 0, sizeof( *source ) );
	source->direction[0] = 1.0f;
	source->type = SAILING_RIVER_SOURCE_DISCHARGE;
}

int BG_SailingAddRiverSource( const sailingRiverSource_t *source ) {
	sailingRiverSource_t *stored;

	if ( !source || bgNumSailingRiverSources >= MAX_SAILING_RIVER_SOURCES
		|| !source->riverName[0]
		|| source->maxs[0] <= source->mins[0]
		|| source->maxs[1] <= source->mins[1]
		|| source->maxs[2] <= source->mins[2]
		|| ( source->type == SAILING_RIVER_SOURCE_DISCHARGE
			&& source->discharge <= 0.0f )
		|| ( source->type == SAILING_RIVER_SOURCE_VELOCITY
			&& source->speed <= 0.0f ) ) {
		return -1;
	}

	stored = &bgSailingRiverSources[bgNumSailingRiverSources];
	*stored = *source;
	stored->discharge = Com_Clamp( 0.0f, 1.0e12f, stored->discharge );
	stored->speed = Com_Clamp( 0.0f, 4096.0f, stored->speed );
	return bgNumSailingRiverSources++;
}

int BG_SailingRiverSourceCount( void ) {
	return bgNumSailingRiverSources;
}

const sailingRiverSource_t *BG_SailingRiverSource( int index ) {
	if ( index < 0 || index >= bgNumSailingRiverSources ) {
		return NULL;
	}
	return &bgSailingRiverSources[index];
}

void BG_SailingRiverSinkDefaults( sailingRiverSink_t *sink ) {
	if ( !sink ) {
		return;
	}
	memset( sink, 0, sizeof( *sink ) );
	sink->direction[0] = 1.0f;
	sink->type = SAILING_RIVER_SINK_OPEN;
}

int BG_SailingAddRiverSink( const sailingRiverSink_t *sink ) {
	if ( !sink || bgNumSailingRiverSinks >= MAX_SAILING_RIVER_SINKS
		|| !sink->riverName[0]
		|| sink->maxs[0] <= sink->mins[0]
		|| sink->maxs[1] <= sink->mins[1]
		|| sink->maxs[2] <= sink->mins[2] ) {
		return -1;
	}

	bgSailingRiverSinks[bgNumSailingRiverSinks] = *sink;
	return bgNumSailingRiverSinks++;
}

int BG_SailingRiverSinkCount( void ) {
	return bgNumSailingRiverSinks;
}

const sailingRiverSink_t *BG_SailingRiverSink( int index ) {
	if ( index < 0 || index >= bgNumSailingRiverSinks ) {
		return NULL;
	}
	return &bgSailingRiverSinks[index];
}

static int BG_SailingRiverFieldIndex( const sailingRiverField_t *field,
	int x, int y ) {
	if ( !field || x < 0 || x >= field->width
		|| y < 0 || y >= field->height ) {
		return -1;
	}
	return y * field->width + x;
}

qboolean BG_SailingRiverFieldInit( sailingRiverField_t *field,
	const sailingRiver_t *river, sailingRiverCell_t *cells,
	sailingRiverCell_t *scratch, int capacity ) {
	float gridSpanX;
	float gridSpanY;
	int width;
	int height;

	if ( !field || !river || !cells || !scratch || capacity <= 0
		|| river->cellSize <= 0.0f ) {
		return qfalse;
	}
	width = (int)ceilf( ( river->maxs[0] - river->mins[0] )
		/ river->cellSize );
	height = (int)ceilf( ( river->maxs[1] - river->mins[1] )
		/ river->cellSize );
	if ( width <= 0 || height <= 0 || width > capacity / height ) {
		return qfalse;
	}

	memset( field, 0, sizeof( *field ) );
	field->cellSize = river->cellSize;
	field->width = width;
	field->height = height;
	field->cellCount = width * height;
	field->cells = cells;
	field->scratch = scratch;
	/* Center the padded grid so every cell center remains inside the AABB. */
	gridSpanX = width * river->cellSize;
	gridSpanY = height * river->cellSize;
	field->origin[0] = 0.5f * ( river->mins[0] + river->maxs[0]
		- gridSpanX );
	field->origin[1] = 0.5f * ( river->mins[1] + river->maxs[1]
		- gridSpanY );
	memset( cells, 0, field->cellCount * sizeof( *cells ) );
	memset( scratch, 0, field->cellCount * sizeof( *scratch ) );
	return qtrue;
}

void BG_SailingRiverFieldCellCenter( const sailingRiverField_t *field,
	int x, int y, vec2_t center ) {
	if ( !center ) {
		return;
	}
	if ( !field || BG_SailingRiverFieldIndex( field, x, y ) < 0 ) {
		VectorClear2( center );
		return;
	}
	center[0] = field->origin[0] + ( x + 0.5f ) * field->cellSize;
	center[1] = field->origin[1] + ( y + 0.5f ) * field->cellSize;
}

qboolean BG_SailingRiverFieldSetBed( sailingRiverField_t *field,
	int x, int y, float bedHeight, qboolean active ) {
	int index = BG_SailingRiverFieldIndex( field, x, y );
	sailingRiverCell_t *cell;

	if ( index < 0 || Q_isnan( bedHeight ) ) {
		return qfalse;
	}
	cell = &field->cells[index];
	memset( cell, 0, sizeof( *cell ) );
	cell->bedHeight = bedHeight;
	if ( active ) {
		cell->flags = SAILING_RIVER_CELL_ACTIVE;
	}
	field->scratch[index] = *cell;
	return qtrue;
}

static qboolean BG_SailingRiverCellOverlapsBounds(
	const sailingRiverField_t *field, int x, int y,
	const vec3_t mins, const vec3_t maxs ) {
	vec2_t center;
	float halfCell;

	BG_SailingRiverFieldCellCenter( field, x, y, center );
	halfCell = 0.5f * field->cellSize;
	return center[0] + halfCell >= mins[0]
		&& center[0] - halfCell <= maxs[0]
		&& center[1] + halfCell >= mins[1]
		&& center[1] - halfCell <= maxs[1];
}

/*
 * prefill blanket-fills every active cell to a stage ramped between the source
 * and sink, which is only correct when the river brush hugs its channel. On a
 * brush any larger it invents an enormous lake: a 278x432 test grid spanning
 * 1044 units of Z started with 1.8 billion cubic units of water, or about an
 * hour of the authored discharge, placed everywhere at once - including on top
 * of terrain the channel never touches. What follows is that lake sloshing,
 * not a river.
 *
 * Starting dry is therefore the default. Water then enters only through the
 * inlet and finds its own path, which costs some convergence time but cannot
 * fabricate water that the boundaries never supplied.
 */
/*
 * Finds the floor of the open space containing fromZ.
 *
 * Deliberately starts as LOW as the caller allows and only climbs when the
 * start is buried in solid, which happens where the floor has risen. Starting
 * low is what stops the search popping through a low ceiling into the space
 * above it - the failure a fixed generous headroom would reintroduce.
 */
static qboolean BG_SailingRiverFindFloor( const sailingRiver_t *river,
	sailingRiverFloorTrace_t traceFloor, float x, float y, float fromZ,
	float ceilingLimit, float headroomStep, float *floorZ ) {
	float bottom = river->mins[2] - 1.0f;
	/*
	 * The climb may never pass the ceiling of the space we are walking through.
	 * Without this a wall inside a tunnel becomes a staircase: the trace start
	 * is buried in the wall, the search climbs until it clears the wall top,
	 * and then finds the roof and calls it riverbed.
	 */
	float top = Q_min( river->maxs[2] + 1.0f, ceilingLimit );
	float startZ = Q_min( fromZ, top );
	int attempt;

	for ( attempt = 0; attempt < 12; attempt++ ) {
		float hitZ = 0.0f;
		qboolean hit = qfalse;
		qboolean startSolid = qfalse;

		traceFloor( x, y, startZ, bottom, &hitZ, &hit, &startSolid );
		if ( !startSolid && hit && hitZ >= bottom
			&& hitZ <= river->maxs[2] + 1.0f ) {
			*floorZ = hitZ;
			return qtrue;
		}
		if ( startZ >= top || headroomStep <= 0.0f ) {
			break;
		}
		startZ = Q_min( startZ + headroomStep, top );
	}
	return qfalse;
}

/* Solid above a captured floor, or the top of the brush where open. */
static float BG_SailingRiverFindCeiling( const sailingRiver_t *river,
	sailingRiverFloorTrace_t traceFloor, float x, float y, float floorZ ) {
	float top = river->maxs[2] + 1.0f;
	float hitZ = 0.0f;
	qboolean hit = qfalse;
	qboolean startSolid = qfalse;

	traceFloor( x, y, floorZ + 2.0f, top, &hitZ, &hit, &startSolid );
	if ( startSolid || !hit || hitZ <= floorZ ) {
		return top;
	}
	return hitZ;
}

#define SAILING_RIVER_CAPTURE_QUEUE 131072
static int bgSailingRiverCaptureQueue[SAILING_RIVER_CAPTURE_QUEUE];

void BG_SailingRiverFieldCaptureBed( sailingRiverField_t *field,
	const sailingRiver_t *river, sailingRiverFloorTrace_t traceFloor,
	float headroomStep, int *capturedCount ) {
	int head = 0;
	int tail = 0;
	int captured = 0;
	int i;
	int x;
	int y;

	if ( capturedCount ) {
		*capturedCount = 0;
	}
	if ( !field || !river || !field->cells || !traceFloor
		|| field->cellCount > SAILING_RIVER_CAPTURE_QUEUE ) {
		return;
	}
	/* Negative asks for the default; zero is honoured as "never climb", which
	 * is the correct setting for a course that only ever runs downhill and the
	 * only way to make scaling a wall onto a roof impossible. */
	if ( headroomStep < 0.0f ) {
		headroomStep = 32.0f;
	}
	for ( i = 0; i < field->cellCount; i++ ) {
		field->cells[i].flags &= ~SAILING_RIVER_CELL_ACTIVE;
		field->cells[i].bedHeight = river->mins[2];
		field->cells[i].ceilingHeight = river->maxs[2] + 1.0f;
		field->cells[i].depth = 0.0f;
		VectorClear2( field->cells[i].momentum );
	}

	for ( i = 0; i < bgNumSailingRiverSources; i++ ) {
		const sailingRiverSource_t *source = &bgSailingRiverSources[i];

		if ( Q_stricmp( source->riverName, river->name ) ) {
			continue;
		}
		for ( y = 0; y < field->height; y++ ) {
			for ( x = 0; x < field->width; x++ ) {
				int index = BG_SailingRiverFieldIndex( field, x, y );
				vec2_t center;
				float floorZ;

				if ( index < 0
					|| ( field->cells[index].flags
						& SAILING_RIVER_CELL_ACTIVE )
					|| !BG_SailingRiverCellOverlapsBounds( field, x, y,
						source->mins, source->maxs ) ) {
					continue;
				}
				BG_SailingRiverFieldCellCenter( field, x, y, center );
				/* The source brush sits in the channel, so its own underside
				 * is a trustworthy seed even under a roof. */
				if ( !BG_SailingRiverFindFloor( river, traceFloor, center[0],
					center[1], source->mins[2] + 1.0f,
					river->maxs[2] + 1.0f, headroomStep, &floorZ ) ) {
					continue;
				}
				field->cells[index].bedHeight = floorZ;
				field->cells[index].ceilingHeight = BG_SailingRiverFindCeiling(
					river, traceFloor, center[0], center[1], floorZ );
				field->cells[index].flags |= SAILING_RIVER_CELL_ACTIVE;
				bgSailingRiverCaptureQueue[tail++] = index;
				captured++;
			}
		}
	}

	while ( head < tail ) {
		int index = bgSailingRiverCaptureQueue[head++];
		int cellX = index % field->width;
		int cellY = index / field->width;
		float fromZ = field->cells[index].bedHeight + 2.0f;
		/* Confine the neighbour search to the space this cell is in. */
		float ceilingLimit = field->cells[index].ceilingHeight;
		int side;

		for ( side = 0; side < 4; side++ ) {
			static const int offsetX[4] = { 1, -1, 0, 0 };
			static const int offsetY[4] = { 0, 0, 1, -1 };
			int neighborX = cellX + offsetX[side];
			int neighborY = cellY + offsetY[side];
			int neighbor = BG_SailingRiverFieldIndex( field, neighborX,
				neighborY );
			vec2_t center;
			float floorZ;

			if ( neighbor < 0 || ( field->cells[neighbor].flags
				& SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			BG_SailingRiverFieldCellCenter( field, neighborX, neighborY,
				center );
			if ( !BG_SailingRiverFindFloor( river, traceFloor, center[0],
				center[1], fromZ, ceilingLimit, headroomStep, &floorZ ) ) {
				continue;
			}
			field->cells[neighbor].bedHeight = floorZ;
			field->cells[neighbor].ceilingHeight = BG_SailingRiverFindCeiling(
				river, traceFloor, center[0], center[1], floorZ );
			field->cells[neighbor].flags |= SAILING_RIVER_CELL_ACTIVE;
			bgSailingRiverCaptureQueue[tail++] = neighbor;
			captured++;
		}
	}
	if ( capturedCount ) {
		*capturedCount = captured;
	}
}

void BG_SailingRiverFieldInitializeWater( sailingRiverField_t *field,
	const sailingRiver_t *river, qboolean prefill ) {
	const sailingRiverSource_t *stageSource = NULL;
	const sailingRiverSink_t *stageSink = NULL;
	vec2_t sourceCenter;
	vec2_t sinkCenter;
	vec2_t reach;
	float reachLengthSquared = 0.0f;
	int x;
	int y;
	int i;

	if ( !field || !river || !field->cells ) {
		return;
	}
	for ( i = 0; i < bgNumSailingRiverSources; i++ ) {
		if ( !Q_stricmp( bgSailingRiverSources[i].riverName, river->name ) ) {
			stageSource = &bgSailingRiverSources[i];
			break;
		}
	}
	for ( i = 0; i < bgNumSailingRiverSinks; i++ ) {
		if ( !Q_stricmp( bgSailingRiverSinks[i].riverName, river->name ) ) {
			stageSink = &bgSailingRiverSinks[i];
			break;
		}
	}
	if ( stageSource ) {
		sourceCenter[0] = 0.5f * ( stageSource->mins[0]
			+ stageSource->maxs[0] );
		sourceCenter[1] = 0.5f * ( stageSource->mins[1]
			+ stageSource->maxs[1] );
	}
	else {
		VectorClear2( sourceCenter );
	}
	if ( stageSink ) {
		sinkCenter[0] = 0.5f * ( stageSink->mins[0]
			+ stageSink->maxs[0] );
		sinkCenter[1] = 0.5f * ( stageSink->mins[1]
			+ stageSink->maxs[1] );
	}
	else {
		VectorClear2( sinkCenter );
	}
	if ( stageSource && stageSink ) {
		VectorSubtract2( sinkCenter, sourceCenter, reach );
		reachLengthSquared = VectorLengthSquared2( reach );
	}

	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			int index = BG_SailingRiverFieldIndex( field, x, y );
			sailingRiverCell_t *cell = &field->cells[index];
			float stage = cell->bedHeight;
			vec2_t center;

			cell->depth = 0.0f;
			VectorClear2( cell->momentum );
			cell->flags &= SAILING_RIVER_CELL_ACTIVE;
			if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			BG_SailingRiverFieldCellCenter( field, x, y, center );
			if ( !prefill ) {
				/* Dry start: flags below still need to be assigned, but no
				 * water exists until a boundary supplies it. */
				stage = cell->bedHeight;
			}
			else if ( stageSource && stageSink
				&& reachLengthSquared > 0.001f ) {
				vec2_t offset;
				float fraction;

				VectorSubtract2( center, sourceCenter, offset );
				fraction = Com_Clamp( 0.0f, 1.0f,
					( offset[0] * reach[0] + offset[1] * reach[1] )
						/ reachLengthSquared );
				stage = stageSource->surfaceHeight
					+ fraction * ( stageSink->surfaceHeight
						- stageSource->surfaceHeight );
			}
			else if ( stageSource ) {
				stage = stageSource->surfaceHeight;
			}
			else if ( stageSink ) {
				stage = stageSink->surfaceHeight;
			}
			cell->depth = Q_max( 0.0f, stage - cell->bedHeight );

			for ( i = 0; i < bgNumSailingRiverSources; i++ ) {
				const sailingRiverSource_t *source =
					&bgSailingRiverSources[i];
				if ( !Q_stricmp( source->riverName, river->name )
					&& BG_SailingRiverCellOverlapsBounds( field, x, y,
						source->mins, source->maxs ) ) {
					cell->flags |= SAILING_RIVER_CELL_SOURCE;
				}
			}
			for ( i = 0; i < bgNumSailingRiverSinks; i++ ) {
				const sailingRiverSink_t *sink = &bgSailingRiverSinks[i];
				if ( !Q_stricmp( sink->riverName, river->name )
					&& BG_SailingRiverCellOverlapsBounds( field, x, y,
						sink->mins, sink->maxs ) ) {
					cell->flags |= SAILING_RIVER_CELL_SINK;
				}
			}
		}
	}
	/* Friction is unknown at build time, so a normal-depth outlet stays
	 * transmissive until the first step supplies it. */
	BG_SailingRiverFieldApplyBoundaries( field, river, 0.0f, 0.0f );
	memcpy( field->scratch, field->cells,
		field->cellCount * sizeof( *field->cells ) );
	field->stepCount = 0;
	field->simulationTime = 0.0f;
}

float BG_SailingRiverFieldTimeStep( const sailingRiverField_t *field,
	float gravity, float cfl, float maximumTimeStep ) {
	float maximumSignalSpeed = 0.0f;
	int i;

	if ( !field || !field->cells || gravity <= 0.0f
		|| cfl <= 0.0f || maximumTimeStep <= 0.0f ) {
		return 0.0f;
	}
	for ( i = 0; i < field->cellCount; i++ ) {
		const sailingRiverCell_t *cell = &field->cells[i];
		float speed;
		float signalSpeed;

		if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE )
			|| cell->depth <= 0.001f ) {
			continue;
		}
		speed = sqrtf( VectorLengthSquared2( cell->momentum ) )
			/ cell->depth;
		signalSpeed = speed + sqrtf( gravity * cell->depth );
		maximumSignalSpeed = Q_max( maximumSignalSpeed, signalSpeed );
	}
	if ( maximumSignalSpeed <= 0.001f ) {
		return maximumTimeStep;
	}
	return Q_min( maximumTimeStep,
		cfl * field->cellSize / maximumSignalSpeed );
}

void BG_SailingRiverFieldApplyBoundaries( sailingRiverField_t *field,
	const sailingRiver_t *river, float timeStep, float friction ) {
	BG_SailingRiverFieldApplyBoundariesOverride( field, river, timeStep,
		-1.0f, friction );
}

static qboolean BG_SailingRiverSourceBoundaryCell(
	const sailingRiverField_t *field, const sailingRiverSource_t *source,
	int x, int y, int axis, int directionStep ) {
	const sailingRiverCell_t *cell;
	int upstreamX = x;
	int upstreamY = y;

	cell = &field->cells[BG_SailingRiverFieldIndex( field, x, y )];
	if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE )
		|| !BG_SailingRiverCellOverlapsBounds( field, x, y,
			source->mins, source->maxs ) ) {
		return qfalse;
	}
	if ( axis == 0 ) {
		upstreamX -= directionStep;
	}
	else {
		upstreamY -= directionStep;
	}
	if ( upstreamX < 0 || upstreamX >= field->width
		|| upstreamY < 0 || upstreamY >= field->height ) {
		return qtrue;
	}
	return !BG_SailingRiverCellOverlapsBounds( field,
		upstreamX, upstreamY, source->mins, source->maxs );
}

/*
 * Boundary depth for a prescribed inflow discharge.
 *
 * A subcritical inlet may only be given ONE condition. The interior still
 * sends a characteristic back upstream into the boundary, so prescribing both
 * a stage and a velocity over-determines it: the solver then computes an
 * ordinary Riemann flux across the inlet face and what actually enters is
 * whatever that flux yields, not the authored discharge. Measured on the test
 * reach, that leaked 1%, 14% and 29% of the authored CFS at bed frictions of
 * 0.02, 0.12 and 0.30 - the shortfall grew with roughness because a slower,
 * deeper interior weakens the head across that face. Discharge that depends on
 * bed roughness is not a discharge boundary at all.
 *
 * So prescribe the discharge and let the interior set the depth, through the
 * outgoing invariant u - 2*sqrt(g*h). Solving
 *
 *   q/h - 2*sqrt(g*h) = u_interior - 2*sqrt(g*h_interior)
 *
 * gives the one boundary depth compatible with what the interior is already
 * doing, so the resulting face flux is q by construction at any friction,
 * slope, or tailwater.
 *
 * f is monotonically decreasing with a single positive root, so Newton from
 * at or above critical depth converges from any starting interior state.
 * Dropping below critical means the interior cannot accept this discharge
 * subcritically - the inlet is choked and controls the flow itself - and
 * critical depth is then the correct answer rather than a fallback.
 */
static float BG_SailingRiverInflowDepth( float unitDischarge,
	float interiorDepth, float interiorVelocity ) {
	float criticalDepth;
	float invariant;
	float depth;
	int iteration;

	if ( unitDischarge <= 0.0f ) {
		return 0.0f;
	}
	criticalDepth = powf( unitDischarge * unitDischarge / SAILING_GRAVITY,
		1.0f / 3.0f );
	if ( interiorDepth <= 0.001f ) {
		return criticalDepth;
	}
	invariant = interiorVelocity
		- 2.0f * sqrtf( SAILING_GRAVITY * interiorDepth );
	depth = Q_max( interiorDepth, criticalDepth );
	for ( iteration = 0; iteration < 24; iteration++ ) {
		float value = unitDischarge / depth
			- 2.0f * sqrtf( SAILING_GRAVITY * depth ) - invariant;
		float derivative = -unitDischarge / ( depth * depth )
			- sqrtf( SAILING_GRAVITY / depth );
		float step;

		if ( fabsf( derivative ) < 0.000001f || Q_isnan( value ) ) {
			break;
		}
		step = value / derivative;
		depth -= step;
		if ( depth <= criticalDepth ) {
			return criticalDepth;
		}
		if ( fabsf( step ) < 0.0001f ) {
			break;
		}
	}
	return Q_max( depth, criticalDepth );
}

static void BG_SailingRiverApplyDischargeBoundary(
	sailingRiverField_t *field, const sailingRiverSource_t *source,
	float sourceDischarge ) {
	vec2_t direction;
	float directionLength;
	float referenceBed = 0.0f;
	float boundaryWidth;
	float unitDischarge;
	float interiorDepth = 0.0f;
	float interiorVelocity = 0.0f;
	float targetSurface;
	float crossSectionArea = 0.0f;
	float incomingSpeed;
	float normalComponent;
	int axis;
	int directionStep;
	int boundaryCellCount = 0;
	int interiorCount = 0;
	int x;
	int y;

	VectorCopy2( source->direction, direction );
	directionLength = sqrtf( VectorLengthSquared2( direction ) );
	if ( directionLength <= 0.001f || sourceDischarge <= 0.0f ) {
		return;
	}
	VectorScale2( direction, 1.0f / directionLength, direction );
	axis = fabsf( direction[0] ) >= fabsf( direction[1] ) ? 0 : 1;
	directionStep = direction[axis] >= 0.0f ? 1 : -1;
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			const sailingRiverCell_t *cell;

			if ( !BG_SailingRiverSourceBoundaryCell( field, source,
				x, y, axis, directionStep ) ) {
				continue;
			}
			cell = &field->cells[BG_SailingRiverFieldIndex( field, x, y )];
			referenceBed += cell->bedHeight;
			boundaryCellCount++;
		}
	}
	if ( !boundaryCellCount ) {
		return;
	}
	referenceBed /= boundaryCellCount;
	boundaryWidth = boundaryCellCount * field->cellSize;
	unitDischarge = sourceDischarge / boundaryWidth;
	/* Interior state one cell downstream, which carries the characteristic
	 * that the inlet has to stay consistent with. */
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			const sailingRiverCell_t *interior;
			int interiorIndex;
			int interiorX = x;
			int interiorY = y;

			if ( !BG_SailingRiverSourceBoundaryCell( field, source, x, y,
				axis, directionStep ) ) {
				continue;
			}
			if ( axis == 0 ) {
				interiorX += directionStep;
			}
			else {
				interiorY += directionStep;
			}
			interiorIndex = BG_SailingRiverFieldIndex( field, interiorX,
				interiorY );
			if ( interiorIndex < 0 ) {
				continue;
			}
			interior = &field->cells[interiorIndex];
			if ( !( interior->flags & SAILING_RIVER_CELL_ACTIVE )
				|| interior->depth <= 0.001f ) {
				continue;
			}
			interiorDepth += interior->depth;
			interiorVelocity += ( interior->momentum[0] * direction[0]
				+ interior->momentum[1] * direction[1] ) / interior->depth;
			interiorCount++;
		}
	}
	if ( interiorCount ) {
		interiorDepth /= interiorCount;
		interiorVelocity /= interiorCount;
	}
	/*
	 * The authored surfaceHeight is deliberately NOT applied here any more. It
	 * remains the initial/minimum stage the field is filled to at build time,
	 * but forcing it at runtime would re-impose a second condition on a
	 * subcritical inlet and reintroduce exactly the discharge leak this solve
	 * removes.
	 */
	targetSurface = referenceBed + BG_SailingRiverInflowDepth( unitDischarge,
		interiorDepth, interiorVelocity );
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			const sailingRiverCell_t *cell;

			if ( !BG_SailingRiverSourceBoundaryCell( field, source,
				x, y, axis, directionStep ) ) {
				continue;
			}
			cell = &field->cells[BG_SailingRiverFieldIndex( field, x, y )];
			crossSectionArea += Q_max( 0.0f,
				targetSurface - cell->bedHeight ) * field->cellSize;
		}
	}
	normalComponent = fabsf( direction[axis] );
	if ( crossSectionArea <= 0.001f || normalComponent <= 0.001f ) {
		return;
	}
	incomingSpeed = sourceDischarge
		/ ( crossSectionArea * normalComponent );
	incomingSpeed = Com_Clamp( 0.0f, 4096.0f, incomingSpeed );
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			sailingRiverCell_t *cell;

			if ( !BG_SailingRiverSourceBoundaryCell( field, source,
				x, y, axis, directionStep ) ) {
				continue;
			}
			cell = &field->cells[BG_SailingRiverFieldIndex( field, x, y )];
			cell->flags |= SAILING_RIVER_CELL_SOURCE;
			cell->depth = Q_max( 0.0f,
				targetSurface - cell->bedHeight );
			cell->momentum[0] = cell->depth * direction[0] * incomingSpeed;
			cell->momentum[1] = cell->depth * direction[1] * incomingSpeed;
		}
	}
}

/*
 * Normal-depth outlet.
 *
 * A fixed stage is the mirror of the over-determined inlet: it prescribes an
 * elevation on a subcritical outflow, which the interior also has a say in,
 * and it can supply or remove unlimited water to hold that number. Worse, it
 * holds the same elevation at every discharge, so a flood is dragged back down
 * to its dry-season tailwater and can never raise its own outlet.
 *
 * Instead solve uniform flow for the depth the arriving discharge would
 * naturally run at. With the Chezy drag the step uses, g*h*S = Cf*u^2 and
 * u = q/h give
 *
 *   h_normal = cbrt( Cf * q^2 / ( g * S ) )
 *
 * so the stage rises as q^(2/3) - the outlet floods when the river floods.
 * Velocity then comes from the incoming characteristic u + 2*sqrt(g*h) so the
 * outlet stays a single condition rather than two.
 *
 * Supercritical outflow carries both characteristics out of the domain and
 * admits no boundary condition at all, so that case is left transmissive.
 */
static void BG_SailingRiverApplyComputedStageSink( sailingRiverField_t *field,
	const sailingRiverSink_t *sink, float friction, float inflowDischarge,
	qboolean overfall ) {
	vec2_t direction;
	float directionLength;
	float interiorDepth = 0.0f;
	float interiorVelocity = 0.0f;
	float slope = 0.0f;
	float outletWidth;
	float unitDischarge;
	float outletDepth;
	float outflowVelocity;
	float interiorFroude;
	const int slopeSpan = 4;
	int interiorCount = 0;
	int slopeCount = 0;
	int sinkCells = 0;
	int axis;
	int step;
	int x;
	int y;

	VectorCopy2( sink->direction, direction );
	directionLength = sqrtf( VectorLengthSquared2( direction ) );
	if ( directionLength <= 0.001f ) {
		return;
	}
	/* Only a normal-depth outlet needs roughness; an overfall does not. */
	if ( !overfall && friction <= 0.0f ) {
		return;
	}
	VectorScale2( direction, 1.0f / directionLength, direction );
	axis = fabsf( direction[0] ) >= fabsf( direction[1] ) ? 0 : 1;
	/* Upstream is against the outflow direction. */
	step = direction[axis] >= 0.0f ? -1 : 1;
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			const sailingRiverCell_t *cell;
			const sailingRiverCell_t *interior;
			const sailingRiverCell_t *upstream;
			int interiorIndex;
			int upstreamIndex;
			int index = BG_SailingRiverFieldIndex( field, x, y );

			if ( index < 0 || !BG_SailingRiverCellOverlapsBounds( field, x, y,
				sink->mins, sink->maxs ) ) {
				continue;
			}
			cell = &field->cells[index];
			if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			sinkCells++;
			interiorIndex = axis == 0
				? BG_SailingRiverFieldIndex( field, x + step, y )
				: BG_SailingRiverFieldIndex( field, x, y + step );
			upstreamIndex = axis == 0
				? BG_SailingRiverFieldIndex( field, x + step * slopeSpan, y )
				: BG_SailingRiverFieldIndex( field, x, y + step * slopeSpan );
			if ( interiorIndex >= 0 ) {
				interior = &field->cells[interiorIndex];
				if ( ( interior->flags & SAILING_RIVER_CELL_ACTIVE )
					&& interior->depth > 0.001f ) {
					interiorDepth += interior->depth;
					interiorVelocity += ( interior->momentum[0] * direction[0]
						+ interior->momentum[1] * direction[1] )
						/ interior->depth;
					interiorCount++;
				}
			}
			if ( upstreamIndex >= 0 ) {
				upstream = &field->cells[upstreamIndex];
				if ( upstream->flags & SAILING_RIVER_CELL_ACTIVE ) {
					slope += ( upstream->bedHeight - cell->bedHeight )
						/ ( slopeSpan * field->cellSize );
					slopeCount++;
				}
			}
		}
	}
	if ( !interiorCount || ( !overfall && !slopeCount ) ) {
		return;
	}
	interiorDepth /= interiorCount;
	interiorVelocity /= interiorCount;
	slope /= slopeCount;
	/* A flat or adverse outlet has no normal depth; keep a small positive
	 * slope so the result stays finite and merely very deep. */
	slope = Q_max( slope, 0.0005f );
	/*
	 * Deliberately the river's inflow, NOT interiorDepth * interiorVelocity.
	 * At steady state an outlet passes the inflow by definition, and deriving
	 * it from the measured velocity instead deadlocks: a ponded reach has no
	 * outward velocity, so the outlet computes zero discharge and does
	 * nothing, so the reach never drains and never develops a velocity. That
	 * is the same self-reinforcing dam the provisional open sink suffers from,
	 * and it left this outlet completely inert on a filling reservoir.
	 */
	/*
	 * Width is the sink brush's extent ACROSS the flow, not its cell count:
	 * an outlet eight cells wide and nine long passes discharge through the
	 * eight, and using all seventy-two makes q nine times too small and the
	 * normal depth far too shallow. Same construction the discharge inlet
	 * uses for its boundary width.
	 */
	outletWidth = fabsf( direction[1] ) * ( sink->maxs[0] - sink->mins[0] )
		+ fabsf( direction[0] ) * ( sink->maxs[1] - sink->mins[1] );
	outletWidth = Q_max( outletWidth, field->cellSize );
	unitDischarge = inflowDischarge / outletWidth;
	if ( unitDischarge <= 0.0f || sinkCells <= 0 ) {
		return;
	}
	if ( overfall ) {
		outletDepth = powf( unitDischarge * unitDischarge / SAILING_GRAVITY,
			1.0f / 3.0f );
	}
	else {
		outletDepth = powf( friction * unitDischarge * unitDischarge
			/ ( SAILING_GRAVITY * slope ), 1.0f / 3.0f );
	}
	if ( outletDepth <= 0.001f ) {
		return;
	}
	interiorFroude = interiorVelocity
		/ sqrtf( SAILING_GRAVITY * interiorDepth );
	/* Supercritical approach carries both characteristics out of the domain
	 * and admits no boundary condition; the brink cannot control it. */
	if ( interiorFroude >= 1.0f ) {
		return;
	}
	if ( overfall ) {
		/* Critical flow by construction: q/hc is exactly sqrt(g*hc), so the
		 * brink sits at Froude one and passes whatever arrives without ever
		 * backing water up further than critical depth requires. */
		outflowVelocity = unitDischarge / outletDepth;
	}
	else {
		outflowVelocity = interiorVelocity
			+ 2.0f * sqrtf( SAILING_GRAVITY * interiorDepth )
			- 2.0f * sqrtf( SAILING_GRAVITY * outletDepth );
	}
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			sailingRiverCell_t *cell;
			int index = BG_SailingRiverFieldIndex( field, x, y );

			if ( index < 0 || !BG_SailingRiverCellOverlapsBounds( field, x, y,
				sink->mins, sink->maxs ) ) {
				continue;
			}
			cell = &field->cells[index];
			if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			cell->flags |= SAILING_RIVER_CELL_SINK;
			cell->depth = outletDepth;
			cell->momentum[0] = outletDepth * direction[0] * outflowVelocity;
			cell->momentum[1] = outletDepth * direction[1] * outflowVelocity;
		}
	}
}

void BG_SailingRiverFieldApplyBoundariesOverride( sailingRiverField_t *field,
	const sailingRiver_t *river, float timeStep, float dischargeOverride,
	float friction ) {
	int sourceIndex;
	int sinkIndex;

	if ( !field || !river || !field->cells ) {
		return;
	}
	for ( sourceIndex = 0; sourceIndex < bgNumSailingRiverSources;
		sourceIndex++ ) {
		const sailingRiverSource_t *source =
			&bgSailingRiverSources[sourceIndex];
		float sourceDischarge = source->discharge;
		int x;
		int y;

		if ( Q_stricmp( source->riverName, river->name ) ) {
			continue;
		}
		if ( source->type == SAILING_RIVER_SOURCE_DISCHARGE
			&& dischargeOverride >= 0.0f ) {
			sourceDischarge = dischargeOverride;
		}
		if ( source->type == SAILING_RIVER_SOURCE_DISCHARGE ) {
			BG_SailingRiverApplyDischargeBoundary( field, source,
				sourceDischarge );
			continue;
		}
		for ( y = 0; y < field->height; y++ ) {
			for ( x = 0; x < field->width; x++ ) {
				sailingRiverCell_t *cell = &field->cells[
					BG_SailingRiverFieldIndex( field, x, y )];
				float targetDepth;

				if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE )
					|| !BG_SailingRiverCellOverlapsBounds( field, x, y,
						source->mins, source->maxs ) ) {
					continue;
				}
				cell->flags |= SAILING_RIVER_CELL_SOURCE;
				targetDepth = Q_max( 0.0f,
					source->surfaceHeight - cell->bedHeight );
				cell->depth = targetDepth;
				cell->momentum[0] = cell->depth
					* source->direction[0] * source->speed;
				cell->momentum[1] = cell->depth
					* source->direction[1] * source->speed;
			}
		}
	}

	for ( sinkIndex = 0; sinkIndex < bgNumSailingRiverSinks; sinkIndex++ ) {
		const sailingRiverSink_t *sink = &bgSailingRiverSinks[sinkIndex];
		int x;
		int y;

		if ( Q_stricmp( sink->riverName, river->name ) ) {
			continue;
		}
		if ( sink->type == SAILING_RIVER_SINK_NORMAL_DEPTH
			|| sink->type == SAILING_RIVER_SINK_OVERFALL ) {
			BG_SailingRiverApplyComputedStageSink( field, sink, friction,
				BG_SailingRiverInflowDischarge( river, dischargeOverride ),
				sink->type == SAILING_RIVER_SINK_OVERFALL ? qtrue : qfalse );
			continue;
		}
		for ( y = 0; y < field->height; y++ ) {
			for ( x = 0; x < field->width; x++ ) {
				sailingRiverCell_t *cell = &field->cells[
					BG_SailingRiverFieldIndex( field, x, y )];
				float oldDepth;

				if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE )
					|| !BG_SailingRiverCellOverlapsBounds( field, x, y,
						sink->mins, sink->maxs ) ) {
					continue;
				}
				cell->flags |= SAILING_RIVER_CELL_SINK;
				oldDepth = cell->depth;
				if ( sink->type == SAILING_RIVER_SINK_FIXED_STAGE ) {
					float velocity[2] = { 0.0f, 0.0f };
					float outgoingSpeed;

					if ( oldDepth > 0.001f ) {
						velocity[0] = cell->momentum[0] / oldDepth;
						velocity[1] = cell->momentum[1] / oldDepth;
					}
					cell->depth = Q_max( 0.0f,
						sink->surfaceHeight - cell->bedHeight );
					/* A downstream stage boundary may remove or supply volume,
					 * but it must not reflect a numerical pulse upstream. */
					outgoingSpeed = velocity[0] * sink->direction[0]
						+ velocity[1] * sink->direction[1];
					if ( outgoingSpeed < 0.0f ) {
						velocity[0] -= outgoingSpeed * sink->direction[0];
						velocity[1] -= outgoingSpeed * sink->direction[1];
					}
					cell->momentum[0] = cell->depth * velocity[0];
					cell->momentum[1] = cell->depth * velocity[1];
				}
				else if ( timeStep > 0.0f && oldDepth > 0.001f ) {
					float outgoingSpeed = ( cell->momentum[0]
						* sink->direction[0] + cell->momentum[1]
						* sink->direction[1] ) / oldDepth;
					float retainedFraction = 1.0f - Com_Clamp( 0.0f, 1.0f,
						outgoingSpeed * timeStep / field->cellSize );
					cell->depth *= retainedFraction;
					VectorScale2( cell->momentum, retainedFraction,
						cell->momentum );
				}
			}
		}
	}
}

static void BG_SailingRiverHydrostaticFlux( const sailingRiverCell_t *left,
	const sailingRiverCell_t *right, int axis, float gravity,
	float leftFluxResult[3], float rightFluxResult[3] ) {
	float leftDepth = Q_max( 0.0f, left->depth );
	float rightDepth = Q_max( 0.0f, right->depth );
	float interfaceBed = Q_max( left->bedHeight, right->bedHeight );
	float reconstructedLeftDepth = Q_max( 0.0f,
		left->bedHeight + leftDepth - interfaceBed );
	float reconstructedRightDepth = Q_max( 0.0f,
		right->bedHeight + rightDepth - interfaceBed );
	float leftNormalVelocity = leftDepth > 0.001f
		? left->momentum[axis] / leftDepth : 0.0f;
	float rightNormalVelocity = rightDepth > 0.001f
		? right->momentum[axis] / rightDepth : 0.0f;
	float leftTangentVelocity = leftDepth > 0.001f
		? left->momentum[1 - axis] / leftDepth : 0.0f;
	float rightTangentVelocity = rightDepth > 0.001f
		? right->momentum[1 - axis] / rightDepth : 0.0f;
	float reconstructedLeftNormalMomentum = reconstructedLeftDepth
		* leftNormalVelocity;
	float reconstructedRightNormalMomentum = reconstructedRightDepth
		* rightNormalVelocity;
	float reconstructedLeftTangentMomentum = reconstructedLeftDepth
		* leftTangentVelocity;
	float reconstructedRightTangentMomentum = reconstructedRightDepth
		* rightTangentVelocity;
	float signalSpeed = Q_max(
		fabsf( leftNormalVelocity )
			+ sqrtf( gravity * reconstructedLeftDepth ),
		fabsf( rightNormalVelocity )
			+ sqrtf( gravity * reconstructedRightDepth ) );
	float leftFlux[3];
	float rightFlux[3];
	float flux[3];

	leftFlux[0] = reconstructedLeftNormalMomentum;
	leftFlux[1] = reconstructedLeftNormalMomentum * leftNormalVelocity
		+ 0.5f * gravity * reconstructedLeftDepth
			* reconstructedLeftDepth;
	leftFlux[2] = reconstructedLeftNormalMomentum * leftTangentVelocity;
	rightFlux[0] = reconstructedRightNormalMomentum;
	rightFlux[1] = reconstructedRightNormalMomentum * rightNormalVelocity
		+ 0.5f * gravity * reconstructedRightDepth
			* reconstructedRightDepth;
	rightFlux[2] = reconstructedRightNormalMomentum
		* rightTangentVelocity;
	flux[0] = 0.5f * ( leftFlux[0] + rightFlux[0]
		- signalSpeed
			* ( reconstructedRightDepth - reconstructedLeftDepth ) );
	flux[1] = 0.5f * ( leftFlux[1] + rightFlux[1]
		- signalSpeed * ( reconstructedRightNormalMomentum
			- reconstructedLeftNormalMomentum ) );
	flux[2] = 0.5f * ( leftFlux[2] + rightFlux[2]
		- signalSpeed * ( reconstructedRightTangentMomentum
			- reconstructedLeftTangentMomentum ) );

	/* Hydrostatic reconstruction supplies a different pressure correction
	 * to each side of a bed step.  A level, motionless surface therefore
	 * remains level instead of the bed-gradient term manufacturing jets. */
	leftFluxResult[0] = rightFluxResult[0] = flux[0];
	leftFluxResult[1] = flux[1] + 0.5f * gravity
		* ( leftDepth * leftDepth
			- reconstructedLeftDepth * reconstructedLeftDepth );
	rightFluxResult[1] = flux[1] + 0.5f * gravity
		* ( rightDepth * rightDepth
			- reconstructedRightDepth * reconstructedRightDepth );
	leftFluxResult[2] = rightFluxResult[2] = flux[2];
}

qboolean BG_SailingRiverFieldStep( sailingRiverField_t *field,
	float timeStep, float gravity, float friction ) {
	float scale;
	int x;
	int y;
	int i;
	int clampedCells = 0;
	int resetCells = 0;
	float depthChange = 0.0f;
	float depthTotal = 0.0f;
	int wetCells = 0;
	sailingRiverCell_t *swap;

	if ( !field || !field->cells || !field->scratch || timeStep <= 0.0f
		|| gravity <= 0.0f || friction < 0.0f ) {
		return qfalse;
	}
	memcpy( field->scratch, field->cells,
		field->cellCount * sizeof( *field->cells ) );
	scale = timeStep / field->cellSize;

	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x + 1 < field->width; x++ ) {
			int leftIndex = BG_SailingRiverFieldIndex( field, x, y );
			int rightIndex = leftIndex + 1;
			const sailingRiverCell_t *left = &field->cells[leftIndex];
			const sailingRiverCell_t *right = &field->cells[rightIndex];
			float leftFlux[3];
			float rightFlux[3];

			if ( !( left->flags & SAILING_RIVER_CELL_ACTIVE )
				&& !( right->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			if ( ( left->flags & SAILING_RIVER_CELL_ACTIVE )
				&& ( right->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				BG_SailingRiverHydrostaticFlux( left, right, 0, gravity,
					leftFlux, rightFlux );
				field->scratch[leftIndex].depth -= scale * leftFlux[0];
				field->scratch[rightIndex].depth += scale * rightFlux[0];
				field->scratch[leftIndex].momentum[0] -= scale * leftFlux[1];
				field->scratch[rightIndex].momentum[0] += scale * rightFlux[1];
				field->scratch[leftIndex].momentum[1] -= scale * leftFlux[2];
				field->scratch[rightIndex].momentum[1] += scale * rightFlux[2];
			}
			else if ( left->flags & SAILING_RIVER_CELL_ACTIVE ) {
				sailingRiverCell_t wall = *left;

				wall.momentum[0] = -wall.momentum[0];
				BG_SailingRiverHydrostaticFlux( left, &wall, 0, gravity,
					leftFlux, rightFlux );
				field->scratch[leftIndex].depth -= scale * leftFlux[0];
				field->scratch[leftIndex].momentum[0] -= scale * leftFlux[1];
				field->scratch[leftIndex].momentum[1] -= scale * leftFlux[2];
			}
			else {
				sailingRiverCell_t wall = *right;

				wall.momentum[0] = -wall.momentum[0];
				BG_SailingRiverHydrostaticFlux( &wall, right, 0, gravity,
					leftFlux, rightFlux );
				field->scratch[rightIndex].depth += scale * rightFlux[0];
				field->scratch[rightIndex].momentum[0] += scale * rightFlux[1];
				field->scratch[rightIndex].momentum[1] += scale * rightFlux[2];
			}
		}
	}
	for ( y = 0; y + 1 < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			int lowerIndex = BG_SailingRiverFieldIndex( field, x, y );
			int upperIndex = lowerIndex + field->width;
			const sailingRiverCell_t *lower = &field->cells[lowerIndex];
			const sailingRiverCell_t *upper = &field->cells[upperIndex];
			float lowerFlux[3];
			float upperFlux[3];

			if ( !( lower->flags & SAILING_RIVER_CELL_ACTIVE )
				&& !( upper->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			if ( ( lower->flags & SAILING_RIVER_CELL_ACTIVE )
				&& ( upper->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				BG_SailingRiverHydrostaticFlux( lower, upper, 1, gravity,
					lowerFlux, upperFlux );
				field->scratch[lowerIndex].depth -= scale * lowerFlux[0];
				field->scratch[upperIndex].depth += scale * upperFlux[0];
				field->scratch[lowerIndex].momentum[1] -= scale * lowerFlux[1];
				field->scratch[upperIndex].momentum[1] += scale * upperFlux[1];
				field->scratch[lowerIndex].momentum[0] -= scale * lowerFlux[2];
				field->scratch[upperIndex].momentum[0] += scale * upperFlux[2];
			}
			else if ( lower->flags & SAILING_RIVER_CELL_ACTIVE ) {
				sailingRiverCell_t wall = *lower;

				wall.momentum[1] = -wall.momentum[1];
				BG_SailingRiverHydrostaticFlux( lower, &wall, 1, gravity,
					lowerFlux, upperFlux );
				field->scratch[lowerIndex].depth -= scale * lowerFlux[0];
				field->scratch[lowerIndex].momentum[1] -= scale * lowerFlux[1];
				field->scratch[lowerIndex].momentum[0] -= scale * lowerFlux[2];
			}
			else {
				sailingRiverCell_t wall = *upper;

				wall.momentum[1] = -wall.momentum[1];
				BG_SailingRiverHydrostaticFlux( &wall, upper, 1, gravity,
					lowerFlux, upperFlux );
				field->scratch[upperIndex].depth += scale * upperFlux[0];
				field->scratch[upperIndex].momentum[1] += scale * upperFlux[1];
				field->scratch[upperIndex].momentum[0] += scale * upperFlux[2];
			}
		}
	}

	/* The four edges of the rectangular field are solid unless an authored
	 * source or sink inside the field changes volume there. */
	for ( y = 0; y < field->height; y++ ) {
		int leftIndex = BG_SailingRiverFieldIndex( field, 0, y );
		int rightIndex = BG_SailingRiverFieldIndex( field,
			field->width - 1, y );
		const sailingRiverCell_t *left = &field->cells[leftIndex];
		const sailingRiverCell_t *right = &field->cells[rightIndex];
		float insideFlux[3];
		float wallFlux[3];

		if ( left->flags & SAILING_RIVER_CELL_ACTIVE ) {
			sailingRiverCell_t wall = *left;

			wall.momentum[0] = -wall.momentum[0];
			BG_SailingRiverHydrostaticFlux( &wall, left, 0, gravity,
				wallFlux, insideFlux );
			field->scratch[leftIndex].depth += scale * insideFlux[0];
			field->scratch[leftIndex].momentum[0] += scale * insideFlux[1];
			field->scratch[leftIndex].momentum[1] += scale * insideFlux[2];
		}
		if ( right->flags & SAILING_RIVER_CELL_ACTIVE ) {
			sailingRiverCell_t wall = *right;

			wall.momentum[0] = -wall.momentum[0];
			BG_SailingRiverHydrostaticFlux( right, &wall, 0, gravity,
				insideFlux, wallFlux );
			field->scratch[rightIndex].depth -= scale * insideFlux[0];
			field->scratch[rightIndex].momentum[0] -= scale * insideFlux[1];
			field->scratch[rightIndex].momentum[1] -= scale * insideFlux[2];
		}
	}
	for ( x = 0; x < field->width; x++ ) {
		int lowerIndex = BG_SailingRiverFieldIndex( field, x, 0 );
		int upperIndex = BG_SailingRiverFieldIndex( field, x,
			field->height - 1 );
		const sailingRiverCell_t *lower = &field->cells[lowerIndex];
		const sailingRiverCell_t *upper = &field->cells[upperIndex];
		float insideFlux[3];
		float wallFlux[3];

		if ( lower->flags & SAILING_RIVER_CELL_ACTIVE ) {
			sailingRiverCell_t wall = *lower;

			wall.momentum[1] = -wall.momentum[1];
			BG_SailingRiverHydrostaticFlux( &wall, lower, 1, gravity,
				wallFlux, insideFlux );
			field->scratch[lowerIndex].depth += scale * insideFlux[0];
			field->scratch[lowerIndex].momentum[1] += scale * insideFlux[1];
			field->scratch[lowerIndex].momentum[0] += scale * insideFlux[2];
		}
		if ( upper->flags & SAILING_RIVER_CELL_ACTIVE ) {
			sailingRiverCell_t wall = *upper;

			wall.momentum[1] = -wall.momentum[1];
			BG_SailingRiverHydrostaticFlux( upper, &wall, 1, gravity,
				insideFlux, wallFlux );
			field->scratch[upperIndex].depth -= scale * insideFlux[0];
			field->scratch[upperIndex].momentum[1] -= scale * insideFlux[1];
			field->scratch[upperIndex].momentum[0] -= scale * insideFlux[2];
		}
	}

	/* Hydrostatic reconstruction above includes the bed force.  Applying a
	 * separate centered bed gradient here would double-count it and destroy
	 * the lake-at-rest equilibrium. */
	for ( y = 0; y < field->height; y++ ) {
		for ( x = 0; x < field->width; x++ ) {
			int index = BG_SailingRiverFieldIndex( field, x, y );
			const sailingRiverCell_t *cell = &field->cells[index];
			sailingRiverCell_t *next = &field->scratch[index];
			float speed;
			float damping;

			if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
				continue;
			}
			/* Separated from ordinary drying below purely so the diagnostic
			 * counter means something: a dry cell is normal, a non-finite one
			 * is a broken step. */
			if ( Q_isnan( next->depth ) || Q_isnan( next->momentum[0] )
				|| Q_isnan( next->momentum[1] ) ) {
				next->depth = 0.0f;
				VectorClear2( next->momentum );
				resetCells++;
				continue;
			}
			if ( next->depth <= 0.001f ) {
				next->depth = 0.0f;
				VectorClear2( next->momentum );
				continue;
			}
			speed = sqrtf( VectorLengthSquared2( next->momentum ) )
				/ next->depth;
			damping = 1.0f / ( 1.0f + friction * speed * timeStep
				/ Q_max( next->depth, 1.0f ) );
			VectorScale2( next->momentum, damping, next->momentum );
			speed = sqrtf( VectorLengthSquared2( next->momentum ) )
				/ next->depth;
			if ( speed > 4096.0f ) {
				VectorScale2( next->momentum, 4096.0f / speed,
					next->momentum );
				clampedCells++;
			}
			depthChange += fabsf( next->depth - cell->depth );
			depthTotal += next->depth;
			wetCells++;
		}
	}

	field->clampedCells = clampedCells;
	field->resetCells = resetCells;
	/* Rate rather than per-step change, so the measure does not depend on how
	 * large a step CFL happened to allow. */
	field->depthResidual = wetCells
		? depthChange / ( wetCells * timeStep ) : 0.0f;
	field->meanDepth = wetCells ? depthTotal / wetCells : 0.0f;
	{
		float previousVolume = field->volume;

		field->volume = depthTotal * field->cellSize * field->cellSize;
		field->volumeRate = ( field->volume - previousVolume ) / timeStep;
	}
	swap = field->cells;
	field->cells = field->scratch;
	field->scratch = swap;
	field->stepCount++;
	field->simulationTime += timeStep;
	for ( i = 0; i < field->cellCount; i++ ) {
		if ( !( field->cells[i].flags & SAILING_RIVER_CELL_ACTIVE ) ) {
			field->cells[i].depth = 0.0f;
			VectorClear2( field->cells[i].momentum );
		}
	}
	return qtrue;
}

float BG_SailingRiverInflowDischarge( const sailingRiver_t *river,
	float dischargeOverride ) {
	float inflow = 0.0f;
	int i;

	if ( !river ) {
		return 0.0f;
	}
	for ( i = 0; i < bgNumSailingRiverSources; i++ ) {
		const sailingRiverSource_t *source = &bgSailingRiverSources[i];

		if ( Q_stricmp( source->riverName, river->name ) ) {
			continue;
		}
		if ( source->type == SAILING_RIVER_SOURCE_DISCHARGE ) {
			inflow += dischargeOverride >= 0.0f
				? dischargeOverride : source->discharge;
		}
	}
	return inflow;
}

void BG_SailingRiverBakePath( const sailingRiver_t *river,
	const char *rawMapName, char *out, int outSize ) {
	if ( !out || outSize <= 0 ) {
		return;
	}
	out[0] = '\0';
	if ( !river ) {
		return;
	}
	/*
	 * Bakes live in maps/, beside the BSP they were captured from. That is
	 * where a map's other derived assets already sit, it keeps a mod directory
	 * holding several maps' rivers navigable, and FS_READ searches PK3s the
	 * same way there as anywhere else - so shipping one only means adding
	 * maps/*.river to the pack.
	 *
	 * An authored `bake` key that already names a directory is left alone, so a
	 * mapper who deliberately puts the asset elsewhere still gets what they
	 * asked for; only a bare filename is placed for them.
	 */
	if ( river->bakeFile[0] ) {
		if ( strchr( river->bakeFile, '/' )
			|| strchr( river->bakeFile, '\\' ) ) {
			Q_strncpyz( out, river->bakeFile, outSize );
		}
		else {
			Com_sprintf( out, outSize, "maps/%s", river->bakeFile );
		}
	}
	else if ( rawMapName && rawMapName[0] && river->name[0] ) {
		Com_sprintf( out, outSize, "maps/%s_%s.river",
			rawMapName, river->name );
	}
	else {
		return;
	}
	/* Authored keys are usually written with the extension already on. This
	 * only fills one in when the author left it off, so `bake foo` and
	 * `bake foo.river` name the same asset instead of two. */
	COM_DefaultExtension( out, outSize, ".river" );
}

qboolean BG_SailingRiverFieldConverged( const sailingRiverField_t *field,
	float inflowDischarge, float tolerance ) {
	if ( !field || inflowDischarge <= 0.0f || tolerance <= 0.0f ) {
		return qfalse;
	}
	/* A field that has never stepped has a zero rate for the wrong reason,
	 * and one that is still empty has not started conveying anything yet. */
	if ( field->stepCount <= 0 || field->volume <= 0.0f ) {
		return qfalse;
	}
	return fabsf( field->volumeRate ) < tolerance * inflowDischarge;
}

void BG_SailingRiverFieldFillBakeHeader( const sailingRiverField_t *field,
	sailingRiverBakeHeader_t *header ) {
	if ( !field || !header ) {
		return;
	}
	memset( header, 0, sizeof( *header ) );
	header->magic = SAILING_RIVER_BAKE_MAGIC;
	header->version = SAILING_RIVER_BAKE_VERSION;
	header->width = field->width;
	header->height = field->height;
	header->cellCount = field->cellCount;
	header->cellSize = field->cellSize;
	header->origin[0] = field->origin[0];
	header->origin[1] = field->origin[1];
	header->simulationTime = field->simulationTime;
	header->stepCount = field->stepCount;
}

qboolean BG_SailingRiverBakeHeaderValid( const sailingRiverBakeHeader_t *header,
	const sailingRiverField_t *field ) {
	if ( !header || !field ) {
		return qfalse;
	}
	if ( header->magic != SAILING_RIVER_BAKE_MAGIC
		|| header->version != SAILING_RIVER_BAKE_VERSION ) {
		return qfalse;
	}
	/* The grid is derived from the river brush and cellSize, so any change to
	 * either shows up here and the bake is correctly discarded. */
	if ( header->width != field->width || header->height != field->height
		|| header->cellCount != field->cellCount
		|| fabsf( header->cellSize - field->cellSize ) > 0.01f
		|| fabsf( header->origin[0] - field->origin[0] ) > 0.01f
		|| fabsf( header->origin[1] - field->origin[1] ) > 0.01f ) {
		return qfalse;
	}
	return qtrue;
}

qboolean BG_SailingRiverFieldSample( const sailingRiverField_t *field,
	float x, float y, sailingRiverSample_t *sample ) {
	float gridX;
	float gridY;
	float fractionX;
	float fractionY;
	float weightSum = 0.0f;
	int x0;
	int y0;
	int cornerX[4];
	int cornerY[4];
	float weight[4];
	int i;

	if ( !field || !field->cells || !sample
		|| x < field->origin[0]
		|| y < field->origin[1]
		|| x > field->origin[0] + field->width * field->cellSize
		|| y > field->origin[1] + field->height * field->cellSize ) {
		return qfalse;
	}
	memset( sample, 0, sizeof( *sample ) );
	gridX = ( x - field->origin[0] ) / field->cellSize - 0.5f;
	gridY = ( y - field->origin[1] ) / field->cellSize - 0.5f;
	x0 = (int)floorf( gridX );
	y0 = (int)floorf( gridY );
	fractionX = gridX - x0;
	fractionY = gridY - y0;
	cornerX[0] = cornerX[2] = x0;
	cornerX[1] = cornerX[3] = x0 + 1;
	cornerY[0] = cornerY[1] = y0;
	cornerY[2] = cornerY[3] = y0 + 1;
	weight[0] = ( 1.0f - fractionX ) * ( 1.0f - fractionY );
	weight[1] = fractionX * ( 1.0f - fractionY );
	weight[2] = ( 1.0f - fractionX ) * fractionY;
	weight[3] = fractionX * fractionY;
	for ( i = 0; i < 4; i++ ) {
		int cellX = Com_Clampi( 0, field->width - 1, cornerX[i] );
		int cellY = Com_Clampi( 0, field->height - 1, cornerY[i] );
		const sailingRiverCell_t *cell = &field->cells[
			BG_SailingRiverFieldIndex( field, cellX, cellY )];
		float velocityX = 0.0f;
		float velocityY = 0.0f;

		if ( !( cell->flags & SAILING_RIVER_CELL_ACTIVE ) ) {
			continue;
		}
		if ( cell->depth > 0.001f ) {
			velocityX = cell->momentum[0] / cell->depth;
			velocityY = cell->momentum[1] / cell->depth;
		}
		sample->bedHeight += weight[i] * cell->bedHeight;
		sample->ceilingHeight += weight[i] * cell->ceilingHeight;
		sample->surfaceHeight += weight[i]
			* ( cell->bedHeight + cell->depth );
		sample->depth += weight[i] * cell->depth;
		sample->velocity[0] += weight[i] * velocityX;
		sample->velocity[1] += weight[i] * velocityY;
		sample->flags |= cell->flags;
		weightSum += weight[i];
	}
	if ( weightSum <= 0.0f ) {
		return qfalse;
	}
	sample->bedHeight /= weightSum;
	sample->ceilingHeight /= weightSum;
	sample->surfaceHeight /= weightSum;
	sample->depth /= weightSum;
	VectorScale2( sample->velocity, 1.0f / weightSum, sample->velocity );
	return qtrue;
}

void BG_SailingSetRuntimeRiverField( int riverIndex,
	const sailingRiverField_t *field ) {
	if ( !field || !field->cells || riverIndex < 0
		|| !BG_SailingRiver( riverIndex ) ) {
		BG_SailingClearRuntimeRiverField();
		return;
	}
	bgSailingRuntimeRiverIndex = riverIndex;
	bgSailingRuntimeRiverField = field;
}

void BG_SailingClearRuntimeRiverField( void ) {
	bgSailingRuntimeRiverIndex = -1;
	bgSailingRuntimeRiverField = NULL;
}

/*
 * Descriptor behind the runtime field. Callers that have a sample must use
 * this rather than BG_SailingRiverForPoint: the sample is keyed on XY alone,
 * while the point lookup also tests Z against the river brush, so a swimmer
 * floating at a surface near the top of the volume samples valid water but
 * fails the point test and silently loses whatever the descriptor drives.
 */
const sailingRiver_t *BG_SailingRuntimeRiver( void ) {
	if ( !bgSailingRuntimeRiverField || bgSailingRuntimeRiverIndex < 0 ) {
		return NULL;
	}
	return BG_SailingRiver( bgSailingRuntimeRiverIndex );
}

qboolean BG_SailingRuntimeRiverSample( float x, float y,
	sailingRiverSample_t *sample ) {
	const sailingRiver_t *river;

	if ( !bgSailingRuntimeRiverField || bgSailingRuntimeRiverIndex < 0 ) {
		return qfalse;
	}
	river = BG_SailingRiver( bgSailingRuntimeRiverIndex );
	if ( !river || x < river->mins[0] || x > river->maxs[0]
		|| y < river->mins[1] || y > river->maxs[1] ) {
		return qfalse;
	}
	/* Being inside the runtime domain is meaningful even where the captured
	 * field is inactive: that is dry/solid space where fallback BSP water must
	 * be removed. */
	if ( !BG_SailingRiverFieldSample( bgSailingRuntimeRiverField,
		x, y, sample ) ) {
		if ( sample ) {
			memset( sample, 0, sizeof( *sample ) );
		}
	}
	return qtrue;
}

static float BG_SailingSmoothStep( float edge0, float edge1, float value ) {
	float t;

	if ( edge1 <= edge0 ) {
		return value >= edge1 ? 1.0f : 0.0f;
	}
	t = Com_Clamp( 0.0f, 1.0f, ( value - edge0 ) / ( edge1 - edge0 ) );
	return t * t * ( 3.0f - 2.0f * t );
}

static void BG_SailingShoreInfluenceInternal( const sailingOcean_t *ocean,
	float x, float y, float *waveScale, vec2_t waveScaleGradient,
	float *beachFoam, float *cliffFoam ) {
	int oceanIndex = -1;
	int i;

	if ( waveScale ) *waveScale = 1.0f;
	if ( waveScaleGradient ) {
		waveScaleGradient[0] = 0.0f;
		waveScaleGradient[1] = 0.0f;
	}
	if ( beachFoam ) *beachFoam = 0.0f;
	if ( cliffFoam ) *cliffFoam = 0.0f;
	if ( !ocean ) return;

	/* Open-water fast path: a single box test against the union of all shore
	 * footprints rejects the vast majority of vertices before the per-shore scan,
	 * so points nowhere near a shoreline cost almost nothing. */
	if ( x < bgSailingShoreBoundsMins[0] || x > bgSailingShoreBoundsMaxs[0]
		|| y < bgSailingShoreBoundsMins[1] || y > bgSailingShoreBoundsMaxs[1] ) {
		return;
	}

	for ( i = 0; i < bgNumSailingOceans; i++ ) {
		if ( ocean == &bgSailingOceans[i] ) {
			oceanIndex = i;
			break;
		}
	}
	if ( oceanIndex < 0 ) return;

	for ( i = 0; i < bgNumSailingShores; i++ ) {
		const sailingShore_t *shore = &bgSailingShores[i];
		float minDot, maxDot, span, progress;
		float localScale = 1.0f;

		if ( shore->oceanIndex != oceanIndex
			|| x < shore->mins[0] || x > shore->maxs[0]
			|| y < shore->mins[1] || y > shore->maxs[1] ) {
			continue;
		}
		minDot = shore->direction[0]
			* ( shore->direction[0] >= 0.0f ? shore->mins[0] : shore->maxs[0] )
			+ shore->direction[1]
			* ( shore->direction[1] >= 0.0f ? shore->mins[1] : shore->maxs[1] );
		maxDot = shore->direction[0]
			* ( shore->direction[0] >= 0.0f ? shore->maxs[0] : shore->mins[0] )
			+ shore->direction[1]
			* ( shore->direction[1] >= 0.0f ? shore->maxs[1] : shore->mins[1] );
		span = maxDot - minDot;
		if ( span <= 1.0f ) continue;
		progress = Com_Clamp( 0.0f, 1.0f,
			( x * shore->direction[0] + y * shore->direction[1] - minDot ) / span );

		if ( shore->type == SAILING_SHORE_BEACH ) {
			float foamIn = BG_SailingSmoothStep( 0.20f, 0.52f, progress );
			float foamOut = 1.0f - BG_SailingSmoothStep( 0.78f, 1.0f, progress );
			localScale = 1.0f - BG_SailingSmoothStep( 0.12f, 0.96f, progress );
			if ( beachFoam ) *beachFoam = fmaxf( *beachFoam, foamIn * foamOut );
			if ( waveScale && localScale < *waveScale ) {
				float t = Com_Clamp( 0.0f, 1.0f,
					( progress - 0.12f ) / ( 0.96f - 0.12f ) );
				*waveScale = localScale;
				if ( waveScaleGradient ) {
					float derivative = -6.0f * t * ( 1.0f - t )
						/ ( ( 0.96f - 0.12f ) * span );
					waveScaleGradient[0] = shore->direction[0] * derivative;
					waveScaleGradient[1] = shore->direction[1] * derivative;
				}
			}
		}
		else if ( shore->type == SAILING_SHORE_CLIFF ) {
			/*
			 * The brush is only a bounded capture volume. Its rectangular edge
			 * and single angle are not a cliff, especially for a closed sea
			 * stack. Preserve the ordinary authoritative wave here; Rend2 uses
			 * the cached solid-ground contour for visual run-up, foam and spray.
			 * Collision still prevents boats and players entering the rock.
			 */
			(void)progress;
		}
	}
}

void BG_SailingShoreInfluence( const sailingOcean_t *ocean, float x, float y,
	float *waveScale, float *beachFoam, float *cliffFoam ) {
	BG_SailingShoreInfluenceInternal( ocean, x, y, waveScale, NULL,
		beachFoam, cliffFoam );
}

void BG_SailingCurrentInfluence( const sailingOcean_t *ocean, float x, float y,
	float *surfaceOffset, float *waveScale, vec2_t surfaceGradient,
	vec2_t waveScaleGradient, float *foam ) {
	int oceanIndex = -1;
	int i;

	if ( surfaceOffset ) *surfaceOffset = 0.0f;
	if ( waveScale ) *waveScale = 1.0f;
	if ( surfaceGradient ) {
		surfaceGradient[0] = 0.0f;
		surfaceGradient[1] = 0.0f;
	}
	if ( waveScaleGradient ) {
		waveScaleGradient[0] = 0.0f;
		waveScaleGradient[1] = 0.0f;
	}
	if ( foam ) *foam = 0.0f;
	if ( !ocean
		|| x < bgSailingCurrentBoundsMins[0]
		|| x > bgSailingCurrentBoundsMaxs[0]
		|| y < bgSailingCurrentBoundsMins[1]
		|| y > bgSailingCurrentBoundsMaxs[1] ) {
		return;
	}
	for ( i = 0; i < bgNumSailingOceans; i++ ) {
		if ( ocean == &bgSailingOceans[i] ) {
			oceanIndex = i;
			break;
		}
	}
	if ( oceanIndex < 0 ) {
		return;
	}

	for ( i = 0; i < bgNumSailingCurrents; i++ ) {
		const sailingCurrent_t *current = &bgSailingCurrents[i];
		float dx;
		float dy;
		float radiusSquared;
		float distance;
		float coreRatio;
		float throatRadius;
		float profile;
		float profileDerivative;
		float rimStart;
		float taper;
		float taperDerivative;
		float weight;
		float weightDerivative;
		float localWaveScale;
		float localWaveDerivative;

		if ( current->oceanIndex != oceanIndex ) {
			continue;
		}
		dx = x - current->origin[0];
		dy = y - current->origin[1];
		radiusSquared = dx * dx + dy * dy;
		if ( radiusSquared >= current->radius * current->radius ) {
			continue;
		}
		distance = sqrtf( radiusSquared );
		/*
		 * A plunging throat inside a broad gentle dish:
		 *
		 *   r <= Rt:  f = 1 - r^2 / (3 Rt^2)
		 *   r >  Rt:  f = 2 Rt / (3 r)
		 *
		 * Both give 2/3 and a slope of -2/(3 Rt) at r = Rt, so the join is C1,
		 * and the slope is zero at r = 0, so the throat bottoms out without a
		 * shading cusp.
		 *
		 * The skirt is 1/r, not the ideal free vortex's 1/r^2. A 1/r^2 dish is
		 * down to an eighth of its depth by twice the throat radius and dead
		 * flat past that, which puts the entire depression in the middle and
		 * leaves no visible slope across the footprint. Turbulence, a wandering
		 * core and the large-scale convergence all broaden a real whirlpool's
		 * depression well beyond the inviscid steady-state figure.
		 *
		 * Rt is the throat, a fraction of the flow's coreRadius, so widening the
		 * core to spin the outer footprint harder does not also widen the throat
		 * into a bowl.
		 */
		throatRadius = fmaxf(
			current->coreRadius * SAILING_CURRENT_THROAT_FRACTION, 1.0f );
		if ( distance <= throatRadius ) {
			coreRatio = distance / throatRadius;
			profile = 1.0f - coreRatio * coreRatio / 3.0f;
			profileDerivative = -2.0f * distance
				/ ( 3.0f * throatRadius * throatRadius );
		}
		else {
			profile = 2.0f * throatRadius / ( 3.0f * distance );
			profileDerivative = -2.0f * throatRadius
				/ ( 3.0f * distance * distance );
		}
		/* The skirt is still measurably deep at the outer radius, so land it on
		 * open water with a zero-slope taper rather than letting it step. */
		rimStart = current->radius * SAILING_CURRENT_RIM_TAPER;
		if ( distance <= rimStart ) {
			taper = 1.0f;
			taperDerivative = 0.0f;
		}
		else {
			float taperSpan = current->radius - rimStart;
			float t = ( distance - rimStart ) / taperSpan;
			taper = 1.0f - t * t * ( 3.0f - 2.0f * t );
			taperDerivative = -6.0f * t * ( 1.0f - t ) / taperSpan;
		}
		weight = profile * taper;
		weightDerivative = profileDerivative * taper
			+ profile * taperDerivative;

		if ( surfaceOffset ) {
			*surfaceOffset -= current->sinkDepth * weight;
		}
		if ( surfaceGradient && distance > 0.001f ) {
			float depressionDerivative = -current->sinkDepth * weightDerivative;
			surfaceGradient[0] += depressionDerivative * dx / distance;
			surfaceGradient[1] += depressionDerivative * dy / distance;
		}
		localWaveScale = 1.0f
			- ( 1.0f - current->waveScale ) * weight;
		localWaveDerivative = -( 1.0f - current->waveScale )
			* weightDerivative;
		if ( waveScale ) {
			float previousScale = *waveScale;
			if ( waveScaleGradient && distance > 0.001f ) {
				float previousGradientX = waveScaleGradient[0];
				float previousGradientY = waveScaleGradient[1];
				waveScaleGradient[0] = previousGradientX * localWaveScale
					+ previousScale * localWaveDerivative * dx / distance;
				waveScaleGradient[1] = previousGradientY * localWaveScale
					+ previousScale * localWaveDerivative * dy / distance;
			}
			*waveScale = previousScale * localWaveScale;
		}
		if ( foam ) {
			/*
			 * Foam where the flow actually tears, rather than in a band pinned
			 * to coreRadius. The azimuthal shear rate |dv/dr - v/r| vanishes
			 * under solid-body rotation and peaks at 2 * spinSpeed / coreRadius
			 * just outside the core, so the bright band still lands at the core
			 * edge - but continuously, and scaled by how violent this vortex
			 * really is. The old fixed ring gave a languid eddy and a maelstrom
			 * identical foam.
			 */
			float outerSpan = fmaxf(
				current->radius - current->coreRadius, 0.001f );
			/*
			 * Evaluate the free-vortex term no closer than the core edge. Inside
			 * the core the flow is solid-body and carries no shear at all, so
			 * letting the 1/r^2 term run inward would climb without bound and
			 * saturate the whole throat.
			 */
			float shearRadius = fmaxf( distance, current->coreRadius );
			float t = Com_Clamp( 0.0f, 1.0f,
				( shearRadius - current->coreRadius ) / outerSpan );
			float edgeFade = 1.0f - t * t * ( 3.0f - 2.0f * t );
			float edgeFadeDerivative = -6.0f * t * ( 1.0f - t ) / outerSpan;
			float circulation = current->spinSpeed * current->coreRadius;
			float shear = fabsf(
				circulation * edgeFadeDerivative / shearRadius
				- 2.0f * circulation * edgeFade
					/ ( shearRadius * shearRadius ) );
			/* Ramp in across the core boundary instead of stepping. */
			float band = current->coreRadius * SAILING_CURRENT_FOAM_BAND;
			float shearFoam = Com_Clamp( 0.0f, 1.0f,
				shear * BG_SailingSmoothStep( current->coreRadius - band,
					current->coreRadius + band, distance )
				/ SAILING_CURRENT_FOAM_SHEAR );
			/*
			 * Broad surface raft. Shear is a ring source: it peaks at the core
			 * lip and falls off as 1/r^2, so it whitens the funnel wall and
			 * leaves the surrounding water clean. A real whirlpool's dominant
			 * signature from a low angle is a wide patch of turbulent
			 * whitewater over the whole convergence zone, so drive a second
			 * source from the vortex's own speed and let it fill the footprint,
			 * using the same rim fade the flow field uses.
			 */
			float surfaceFoam = edgeFade * Com_Clamp( 0.0f, 1.0f,
				fmaxf( current->spinSpeed, current->pullSpeed )
					/ SAILING_CURRENT_FOAM_SPEED );

			*foam = fmaxf( *foam, fmaxf( shearFoam, surfaceFoam ) );
		}
	}
}

void BG_SailingCurrentVelocityAt( const sailingOcean_t *ocean, float x, float y,
	int time, vec3_t currentVelocity ) {
	int oceanIndex = -1;
	int i;

	if ( !currentVelocity ) {
		return;
	}

	if ( !ocean ) {
		VectorClear( currentVelocity );
		return;
	}

	currentVelocity[0] = ocean->direction[0] * ocean->speed * ocean->currentScale;
	currentVelocity[1] = ocean->direction[1] * ocean->speed * ocean->currentScale;
	currentVelocity[2] = 0.0f;
	(void)time;

	if ( x < bgSailingCurrentBoundsMins[0]
		|| x > bgSailingCurrentBoundsMaxs[0]
		|| y < bgSailingCurrentBoundsMins[1]
		|| y > bgSailingCurrentBoundsMaxs[1] ) {
		return;
	}
	for ( i = 0; i < bgNumSailingOceans; i++ ) {
		if ( ocean == &bgSailingOceans[i] ) {
			oceanIndex = i;
			break;
		}
	}
	if ( oceanIndex < 0 ) {
		return;
	}

	for ( i = 0; i < bgNumSailingCurrents; i++ ) {
		const sailingCurrent_t *current = &bgSailingCurrents[i];
		float dx;
		float dy;
		float distanceSquared;
		float distance;
		float radialScale;
		float edgeFade;
		float tangentialSpeed;
		float inwardSpeed;
		float inverseDistance;

		if ( current->oceanIndex != oceanIndex ) {
			continue;
		}
		dx = x - current->origin[0];
		dy = y - current->origin[1];
		distanceSquared = dx * dx + dy * dy;
		if ( distanceSquared >= current->radius * current->radius
			|| distanceSquared <= 0.000001f ) {
			continue;
		}
		distance = sqrtf( distanceSquared );
		/* Crest's whirlpool flow field cuts out entirely inside its eye radius.
		 * A still center gives the funnel something to read against; ramping
		 * smoothly through zero instead leaves the throat looking like it is
		 * merely slow rather than the calm the surface shape implies. */
		if ( distance <= current->eyeRadius ) {
			continue;
		}
		inverseDistance = 1.0f / distance;
		if ( distance <= current->coreRadius ) {
			radialScale = ( distance - current->eyeRadius )
				/ ( current->coreRadius - current->eyeRadius );
			tangentialSpeed = current->spinSpeed * radialScale;
			inwardSpeed = current->pullSpeed * radialScale;
		}
		else {
			/*
			 * Both components fall as 1/r, so the spiral pitch is the same at
			 * every radius - the logarithmic spiral a real whirlpool forms.
			 * Angular momentum gives the tangential 1/r; mass conservation for
			 * a sink draining through a layer gives the radial one. Holding the
			 * inflow constant instead made the pitch degrade with radius until
			 * the outer footprint was a 45-degree spiral, which read as a drain
			 * rather than a whirlpool.
			 */
			float falloff;

			edgeFade = 1.0f - BG_SailingSmoothStep( current->coreRadius,
				current->radius, distance );
			falloff = current->coreRadius / distance * edgeFade;
			tangentialSpeed = current->spinSpeed * falloff;
			inwardSpeed = current->pullSpeed * falloff;
		}
		currentVelocity[0] += current->direction * -dy * inverseDistance
			* tangentialSpeed - dx * inverseDistance * inwardSpeed;
		currentVelocity[1] += current->direction * dx * inverseDistance
			* tangentialSpeed - dy * inverseDistance * inwardSpeed;
	}
}

/*
 * The flow that advects surface texture, as distinct from the flow that moves
 * hulls. Buoyant surface material does not follow the bulk flow down, so the
 * inflow is scaled back and the ambient ocean drift is excluded - the base
 * surface's own scrolling UVs already carry that. The result is a much tighter
 * spiral than the physics field, which is what makes a whirlpool read as
 * spinning rather than draining.
 */
void BG_SailingCurrentSurfaceFlowAt( const sailingOcean_t *ocean, float x,
	float y, vec2_t flow ) {
	vec3_t velocity;
	vec3_t drift;

	if ( !flow ) {
		return;
	}
	flow[0] = 0.0f;
	flow[1] = 0.0f;
	if ( !ocean ) {
		return;
	}

	BG_SailingCurrentVelocityAt( ocean, x, y, 0, velocity );
	drift[0] = ocean->direction[0] * ocean->speed * ocean->currentScale;
	drift[1] = ocean->direction[1] * ocean->speed * ocean->currentScale;
	velocity[0] -= drift[0];
	velocity[1] -= drift[1];

	/*
	 * The vortex contribution is tangential plus inward. Recovering the inward
	 * part needs a center to measure against, so take the single nearest
	 * current: overlapping vortexes are rare, and the bias only shapes how the
	 * texture slides, never where the water goes.
	 */
	{
		const sailingCurrent_t *nearest = NULL;
		float nearestDistanceSquared = 0.0f;
		int oceanIndex = -1;
		int i;

		for ( i = 0; i < bgNumSailingOceans; i++ ) {
			if ( ocean == &bgSailingOceans[i] ) {
				oceanIndex = i;
				break;
			}
		}
		for ( i = 0; oceanIndex >= 0 && i < bgNumSailingCurrents; i++ ) {
			const sailingCurrent_t *current = &bgSailingCurrents[i];
			float dx;
			float dy;
			float distanceSquared;

			if ( current->oceanIndex != oceanIndex ) {
				continue;
			}
			dx = x - current->origin[0];
			dy = y - current->origin[1];
			distanceSquared = dx * dx + dy * dy;
			if ( distanceSquared >= current->radius * current->radius ) {
				continue;
			}
			if ( !nearest || distanceSquared < nearestDistanceSquared ) {
				nearest = current;
				nearestDistanceSquared = distanceSquared;
			}
		}
		if ( nearest && nearestDistanceSquared > 0.000001f ) {
			float distance = sqrtf( nearestDistanceSquared );
			float outwardX = ( x - nearest->origin[0] ) / distance;
			float outwardY = ( y - nearest->origin[1] ) / distance;
			float radial = velocity[0] * outwardX + velocity[1] * outwardY;
			float removed = radial
				* ( 1.0f - SAILING_CURRENT_SURFACE_INFLOW );

			velocity[0] -= removed * outwardX;
			velocity[1] -= removed * outwardY;
		}
	}

	flow[0] = velocity[0];
	flow[1] = velocity[1];
}

/*
 * The two-phase advection schedule, shared by every consumer so the legacy
 * polyVert overlay and the enhanced renderer cannot drift apart in method.
 *
 * Callers must treat the result as constant across a surface: two vertices of
 * one primitive, or two adjacent fragments, have to agree on how long the layer
 * has been advecting or the crossfade tears into noise. Only `referenceTile`
 * varies between callers, because each advects something with its own feature
 * size.
 */
void BG_SailingCurrentAdvectionPhase( float referenceTile, float dragFraction,
	float peakSpeed, int time, vec2_t offsets, vec2_t weights ) {
	float seconds = (float)time * 0.001f;
	float shearLimit;
	float halfPeriod;
	float period;
	float weight;

	if ( !offsets || !weights ) {
		return;
	}
	shearLimit = fmaxf( SAILING_CURRENT_MIN_DRAG,
		referenceTile * fmaxf( dragFraction, 0.0f ) );
	halfPeriod = Com_Clamp( SAILING_CURRENT_MIN_HALF_PERIOD,
		SAILING_CURRENT_MAX_HALF_PERIOD,
		shearLimit / fmaxf( peakSpeed, 1.0f ) );
	period = halfPeriod * 2.0f;

	offsets[0] = fmodf( seconds, period );
	offsets[1] = fmodf( seconds + halfPeriod, period );
	weight = offsets[0] / halfPeriod;
	if ( weight > 1.0f ) {
		weight = 2.0f - weight;
	}
	/* A linear triangle reverses its temporal derivative abruptly at both
	 * ends of the crossfade. That cusp reads as a low-frame-rate hitch even
	 * when the UV travel itself is slow. Smooth the handoff so the outgoing
	 * and incoming copies both reach zero temporal weight velocity. */
	weight = weight * weight * ( 3.0f - 2.0f * weight );
	weights[0] = weight;
	weights[1] = 1.0f - weight;
	/* Keep the two samples centred on the unadvected field, as Crest's flow
	 * normal mapping does. Leaving both offsets positive bakes a permanent
	 * flow-dependent warp into the baseline mesh/detail. */
	offsets[0] -= halfPeriod;
	offsets[1] -= halfPeriod;
}

void BG_SailingWindVelocity( const sailingOcean_t *ocean, int time,
	vec3_t windVelocity ) {
	vec2_t wind;

	if ( !windVelocity ) {
		return;
	}
	/* One wind for the whole map: misc_dynamic_weather owns it, and game,
	 * cgame and sky all read the same deterministic time-based vector. The
	 * ocean argument stays so callers keep "no ocean, no wind". */
	if ( !ocean ) {
		VectorClear( windVelocity );
		return;
	}
	BG_DynamicWeatherWindVector( BG_DynamicWeather( 0 ), time, wind );
	windVelocity[0] = wind[0];
	windVelocity[1] = wind[1];
	windVelocity[2] = 0.0f;
}

void BG_SailingSetWaterspout( const sailingWaterspout_t *waterspout ) {
	if ( !waterspout || waterspout->radius <= 0.0f
		|| waterspout->coreRadius <= 0.0f
		|| waterspout->intensity <= 0.0f ) {
		BG_SailingClearWaterspout();
		return;
	}
	bgSailingWaterspout = *waterspout;
	bgSailingWaterspout.radius = Com_Clamp( 1.0f, 32768.0f,
		bgSailingWaterspout.radius );
	bgSailingWaterspout.coreRadius = Com_Clamp( 1.0f,
		bgSailingWaterspout.radius, bgSailingWaterspout.coreRadius );
	bgSailingWaterspout.height = Com_Clamp( 64.0f, 262144.0f,
		bgSailingWaterspout.height );
	bgSailingWaterspout.tangentialSpeed = Com_Clamp( -8192.0f, 8192.0f,
		bgSailingWaterspout.tangentialSpeed );
	bgSailingWaterspout.inflowSpeed = Com_Clamp( 0.0f, 4096.0f,
		bgSailingWaterspout.inflowSpeed );
	bgSailingWaterspout.liftSpeed = Com_Clamp( 0.0f, 4096.0f,
		bgSailingWaterspout.liftSpeed );
	bgSailingWaterspout.intensity = Com_Clamp( 0.0f, 1.0f,
		bgSailingWaterspout.intensity );
	bgSailingWaterspoutValid = qtrue;
}

void BG_SailingClearWaterspout( void ) {
	memset( &bgSailingWaterspout, 0, sizeof( bgSailingWaterspout ) );
	bgSailingWaterspoutValid = qfalse;
}

const sailingWaterspout_t *BG_SailingWaterspout( void ) {
	return bgSailingWaterspoutValid ? &bgSailingWaterspout : NULL;
}

static float BG_SailingWaterspoutSmoothStep( float edge0, float edge1,
	float value ) {
	float fraction;

	if ( edge1 <= edge0 ) {
		return value >= edge1 ? 1.0f : 0.0f;
	}
	fraction = Com_Clamp( 0.0f, 1.0f,
		( value - edge0 ) / ( edge1 - edge0 ) );
	return fraction * fraction * ( 3.0f - 2.0f * fraction );
}

float BG_SailingWaterspoutWindAt( const vec3_t point, int time,
	vec3_t windVelocity ) {
	const sailingWaterspout_t *waterspout = BG_SailingWaterspout();
	float dx, dy, distance, inverseDistance;
	float outerFade, heightFade, coreFraction;
	float tangential, inflow, lift, gust, strength;
	float maximumSpeed;
	float seconds;

	if ( windVelocity ) {
		VectorClear( windVelocity );
	}
	if ( !windVelocity || !point || !waterspout ) {
		return 0.0f;
	}

	dx = point[0] - waterspout->origin[0];
	dy = point[1] - waterspout->origin[1];
	distance = sqrtf( dx * dx + dy * dy );
	if ( distance >= waterspout->radius
		|| point[2] < waterspout->origin[2] - waterspout->coreRadius * 0.5f
		|| point[2] > waterspout->origin[2] + waterspout->height ) {
		return 0.0f;
	}

	outerFade = 1.0f - BG_SailingWaterspoutSmoothStep(
		waterspout->radius * 0.72f, waterspout->radius, distance );
	heightFade = 1.0f - 0.35f * BG_SailingWaterspoutSmoothStep(
		waterspout->origin[2] + waterspout->height * 0.65f,
		waterspout->origin[2] + waterspout->height, point[2] );
	coreFraction = Com_Clamp( 0.0f, 1.0f,
		distance / waterspout->coreRadius );

	/* Rankine-like rotation: solid-body motion inside the radius of maximum
	 * wind, then a reciprocal falloff outside it. The calm mathematical center
	 * keeps both physics and visuals finite. */
	if ( distance < waterspout->coreRadius ) {
		tangential = waterspout->tangentialSpeed * coreFraction;
	} else {
		tangential = waterspout->tangentialSpeed
			* waterspout->coreRadius / distance;
	}
	inflow = waterspout->inflowSpeed
		* BG_SailingWaterspoutSmoothStep( 0.0f, 0.35f, coreFraction );
	if ( distance > waterspout->coreRadius ) {
		inflow *= waterspout->coreRadius / distance;
	}
	lift = waterspout->liftSpeed
		* ( 1.0f - BG_SailingWaterspoutSmoothStep(
			waterspout->coreRadius * 0.35f,
			waterspout->coreRadius * 2.25f, distance ) );

	seconds = (float)time * 0.001f;
	gust = 0.90f + 0.10f * sinf( seconds * 2.17f
		+ (float)( waterspout->seed & 1023u ) * 0.017f
		+ distance * 0.006f );
	strength = waterspout->intensity * outerFade * heightFade * gust;
	tangential *= strength;
	inflow *= strength;
	lift *= waterspout->intensity * outerFade * gust;

	if ( distance > 0.001f ) {
		inverseDistance = 1.0f / distance;
		windVelocity[0] = ( -dy * inverseDistance * tangential
			- dx * inverseDistance * inflow );
		windVelocity[1] = ( dx * inverseDistance * tangential
			- dy * inverseDistance * inflow );
	}
	windVelocity[2] = lift;
	maximumSpeed = fmaxf( fabsf( waterspout->tangentialSpeed ),
		fmaxf( waterspout->inflowSpeed, waterspout->liftSpeed ) );
	if ( maximumSpeed <= 0.0f ) {
		return 0.0f;
	}
	return Com_Clamp( 0.0f, 1.0f,
		VectorLength( windVelocity ) / maximumSpeed );
}

void BG_SailingWindVelocityAt( const sailingOcean_t *ocean,
	const vec3_t point, int time, vec3_t windVelocity ) {
	vec3_t localWind;

	BG_SailingWindVelocity( ocean, time, windVelocity );
	if ( point ) {
		BG_SailingWaterspoutWindAt( point, time, localWind );
		VectorAdd( windVelocity, localWind, windVelocity );
	}
}

float BG_SailingResolveOceanSpeed( float speedAuthored, float wavelength ) {
	if ( speedAuthored == SAILING_OCEAN_SPEED_AUTO ) {
		/* Deep-water phase speed c = sqrt( g * lambda / 2pi ). */
		return sqrtf( SAILING_WAVE_GRAVITY
			* Com_Clamp( 64.0f, 32768.0f, wavelength )
			/ (float)( 2.0 * M_PI ) );
	}
	return Com_Clamp( -4096.0f, 4096.0f, speedAuthored );
}

qboolean BG_SailingSetOceanWeather( int index, float currentScale,
	float amplitude, float steepness, float wavelength, float slosh,
	float speedAuthored, float rainIntensity, float waveDirectionX,
	float waveDirectionY ) {
	sailingOcean_t *ocean;
	float waveDirectionLength;

	if ( index < 0 || index >= bgNumSailingOceans ) {
		return qfalse;
	}
	ocean = &bgSailingOceans[index];
	waveDirectionLength = sqrtf( waveDirectionX * waveDirectionX
		+ waveDirectionY * waveDirectionY );
	if ( waveDirectionLength > 0.001f ) {
		ocean->direction[0] = waveDirectionX / waveDirectionLength;
		ocean->direction[1] = waveDirectionY / waveDirectionLength;
	}
	ocean->rainIntensity = Com_Clamp( 0.0f, 1.0f, rainIntensity );
	ocean->currentScale = Com_Clamp( -2.0f, 2.0f, currentScale );
	ocean->amplitude = Com_Clamp( 0.0f, SAILING_MAX_AMPLITUDE, amplitude );
	ocean->steepness = Com_Clamp( 0.0f, 1.0f, steepness );
	ocean->wavelength = Com_Clamp( 64.0f, 32768.0f, wavelength );
	ocean->slosh = Com_Clamp( 0.0f, 1.0f, slosh );
	ocean->speedAuthored = Com_Clamp( -4096.0f, 4096.0f, speedAuthored );
	/* Re-resolved on every change so an auto speed follows a wavelength-only
	 * target_weatherchange. */
	ocean->speed = BG_SailingResolveOceanSpeed( ocean->speedAuthored,
		ocean->wavelength );
	BG_SailingInvalidateHullCache();
	return qtrue;
}

/*
 * CS_SAILING_OCEANS state, 19 fields:
 *   mins[3] maxs[3] baseHeight amplitude wavelength speedAuthored steepness
 *   slosh waveDirection[2] currentScale rainIntensity waveSeed
 *   gerstnerWaveCount horizontalDisplacementScale
 * Bounds and height ride here so clients never depend on their local BSP
 * entity lump (a server .ent override would otherwise desync physics). Floats
 * use %.9g, which round-trips a float exactly: predicted waves must be built
 * from bit-identical inputs. Wind is not here; it is CS_DYNAMIC_WEATHER's.
 */
void BG_SailingBuildOceanWeatherString( const sailingOcean_t *ocean,
	char *buffer, int bufferSize ) {
	if ( !buffer || bufferSize <= 0 ) {
		return;
	}
	if ( !ocean ) {
		buffer[0] = '\0';
		return;
	}
	Com_sprintf( buffer, bufferSize,
		"%.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g %.9g"
		" %.9g %.9g %.9g %.9g %u %i %.9g",
		ocean->mins[0], ocean->mins[1], ocean->mins[2],
		ocean->maxs[0], ocean->maxs[1], ocean->maxs[2], ocean->baseHeight,
		ocean->amplitude, ocean->wavelength, ocean->speedAuthored,
		ocean->steepness, ocean->slosh,
		ocean->direction[0], ocean->direction[1],
		ocean->currentScale, ocean->rainIntensity,
		ocean->waveSeed, ocean->gerstnerWaveCount,
		ocean->horizontalDisplacementScale );
}

qboolean BG_SailingApplyOceanWeatherString( int index, const char *state ) {
	vec3_t mins, maxs;
	float baseHeight;
	float amplitude, wavelength, speedAuthored, steepness, slosh;
	float waveDirectionX, waveDirectionY, currentScale, rainIntensity;
	float horizontalDisplacementScale;
	unsigned int waveSeed;
	int gerstnerWaveCount;
	sailingOcean_t *ocean;
	int parsed;

	if ( !state || !state[0] || !BG_SailingOcean( index ) ) {
		return qfalse;
	}
	parsed = sscanf( state,
		"%f %f %f %f %f %f %f %f %f %f %f %f %f %f %f %f %u %i %f",
		&mins[0], &mins[1], &mins[2], &maxs[0], &maxs[1], &maxs[2],
		&baseHeight, &amplitude, &wavelength, &speedAuthored, &steepness,
		&slosh, &waveDirectionX, &waveDirectionY, &currentScale,
		&rainIntensity, &waveSeed, &gerstnerWaveCount,
		&horizontalDisplacementScale );
	if ( parsed != 19
		|| mins[0] >= maxs[0] || mins[1] >= maxs[1] || mins[2] > maxs[2] ) {
		return qfalse;
	}
	if ( !BG_SailingSetOceanWeather( index, currentScale, amplitude,
		steepness, wavelength, slosh, speedAuthored, rainIntensity,
		waveDirectionX, waveDirectionY ) ) {
		return qfalse;
	}
	ocean = &bgSailingOceans[index];
	/* Already normalized by the server; renormalizing could move it an ulp. */
	if ( waveDirectionX * waveDirectionX + waveDirectionY * waveDirectionY
		> 0.000001f ) {
		ocean->direction[0] = waveDirectionX;
		ocean->direction[1] = waveDirectionY;
	}
	VectorCopy( mins, ocean->mins );
	VectorCopy( maxs, ocean->maxs );
	ocean->baseHeight = baseHeight;
	ocean->waveSeed = waveSeed ? waveSeed : 85619173u;
	ocean->gerstnerWaveCount = (int)Com_Clamp( 2.0f,
		(float)SAILING_GERSTNER_MAX_WAVES, (float)gerstnerWaveCount );
	ocean->horizontalDisplacementScale =
		Com_Clamp( 0.0f, 1.0f, horizontalDisplacementScale );
	BG_SailingInvalidateHullCache();
	return qtrue;
}

void BG_SailingSailAcceleration( const vec3_t apparentWind, float sailAngle,
	float sailArea, vec3_t acceleration, float *angleOfAttack,
	float *liftCoefficient, float *dragCoefficient ) {
	vec3_t airflow;
	vec3_t sailDirection;
	vec3_t sailNormal;
	vec3_t liftDirection;
	float radians;
	float airflowSpeed;
	float sinAlpha;
	float cosAlpha;
	float chordComponent;
	float orientedSinAlpha;
	float cl;
	float cd;
	float pressure;
	float accelerationLength;

	if ( acceleration ) {
		VectorClear( acceleration );
	}
	if ( angleOfAttack ) *angleOfAttack = 0.0f;
	if ( liftCoefficient ) *liftCoefficient = 0.0f;
	if ( dragCoefficient ) *dragCoefficient = 0.0f;
	if ( !apparentWind || !acceleration || sailArea <= 0.0f ) {
		return;
	}

	/* A compact symmetric lift/drag model. Airflow is the direction the air
	 * travels relative to the boat. The sail chord is treated as undirected so
	 * either tack uses the same coefficients; signed angle of attack chooses the
	 * lift side. This preserves broadside pressure downwind while allowing a
	 * shallow angle of attack to generate useful lift on a reach. */
	VectorSet( airflow, apparentWind[0], apparentWind[1], 0.0f );
	airflowSpeed = VectorNormalize( airflow );
	if ( airflowSpeed <= 0.001f ) {
		return;
	}
	radians = sailAngle * (float)M_PI / 180.0f;
	VectorSet( sailDirection, cosf( radians ), sinf( radians ), 0.0f );
	VectorSet( sailNormal, -sinf( radians ), cosf( radians ), 0.0f );
	sinAlpha = Com_Clamp( -1.0f, 1.0f, DotProduct( airflow, sailNormal ) );
	chordComponent = Com_Clamp( -1.0f, 1.0f,
		DotProduct( airflow, sailDirection ) );
	cosAlpha = fabsf( chordComponent );
	/* Keep lift invariant when the same undirected sail line is represented by
	 * its opposite endpoint. Both chord and normal reverse in that equivalent
	 * representation, so their product—not an absolute chord term—selects the
	 * physical lift side. */
	cl = 1.2f * 2.0f * sinAlpha * chordComponent;
	orientedSinAlpha = sinAlpha * ( chordComponent < 0.0f ? -1.0f : 1.0f );
	cd = 0.08f + sinAlpha * sinAlpha;
	pressure = airflowSpeed * airflowSpeed * sailArea
		* SAILING_SAIL_FORCE_SCALE;

	/* Drag follows the relative airflow. Signed Cl selects one of its two
	 * perpendicular directions without depending on which chord endpoint was
	 * chosen above. */
	VectorScale( airflow, pressure * cd, acceleration );
	VectorSet( liftDirection, -airflow[1], airflow[0], 0.0f );
	VectorMA( acceleration, pressure * cl, liftDirection, acceleration );
	accelerationLength = VectorLength( acceleration );
	if ( accelerationLength > 240.0f ) {
		VectorScale( acceleration, 240.0f / accelerationLength, acceleration );
	}

	if ( angleOfAttack ) {
		*angleOfAttack = atan2f( orientedSinAlpha, cosAlpha )
			* 180.0f / (float)M_PI;
	}
	if ( liftCoefficient ) *liftCoefficient = cl;
	if ( dragCoefficient ) *dragCoefficient = cd;
}

int BG_SailingEncodeBoatControls( float trim, float rudder ) {
	const float trimQuantum = SAILING_SAIL_TRIM_MAX / 15.0f;
	int trimCode;
	int rudderCode;

	trim = Com_Clamp( -SAILING_SAIL_TRIM_MAX, SAILING_SAIL_TRIM_MAX, trim );
	trim /= trimQuantum;
	trimCode = (int)( trim + ( trim >= 0.0f ? 0.5f : -0.5f ) );
	trimCode = (int)Com_Clamp( -15.0f, 15.0f, (float)trimCode );
	if ( trimCode < 0 ) {
		trimCode += 32;
	}

	rudder = Com_Clamp( -1.0f, 1.0f, rudder ) * 3.0f;
	rudderCode = (int)( rudder + ( rudder >= 0.0f ? 0.5f : -0.5f ) );
	rudderCode = (int)Com_Clamp( -3.0f, 3.0f, (float)rudderCode );
	if ( rudderCode < 0 ) {
		rudderCode += 8;
	}
	return ( rudderCode << 5 ) | ( trimCode & 31 );
}

float BG_SailingDecodeTrim( int encodedTrim ) {
	const float trimQuantum = SAILING_SAIL_TRIM_MAX / 15.0f;
	int trimCode = encodedTrim & 31;

	if ( trimCode & 16 ) {
		trimCode -= 32;
	}
	trimCode = (int)Com_Clamp( -15.0f, 15.0f, (float)trimCode );
	return (float)trimCode * trimQuantum;
}

float BG_SailingDecodeRudder( int encodedControls ) {
	int rudderCode = ( encodedControls >> 5 ) & 7;

	if ( rudderCode & 4 ) {
		rudderCode -= 8;
	}
	rudderCode = (int)Com_Clamp( -3.0f, 3.0f, (float)rudderCode );
	return (float)rudderCode / 3.0f;
}

static unsigned int BG_SailingWaveHash( unsigned int value ) {
	value ^= value >> 16;
	value *= 0x7feb352du;
	value ^= value >> 15;
	value *= 0x846ca68bu;
	value ^= value >> 16;
	return value;
}

static float BG_SailingWaveRandom( unsigned int seed, unsigned int index,
	unsigned int salt ) {
	unsigned int bits = BG_SailingWaveHash(
		seed ^ index * 0x9e3779b9u ^ salt );
	return (float)( bits & 0x00ffffffu ) / 16777215.0f;
}

static unsigned int BG_SailingGerstnerRevisionFloat( unsigned int revision,
	float value ) {
	unsigned int bits;

	memcpy( &bits, &value, sizeof( bits ) );
	return ( revision ^ bits ) * 16777619u;
}

static unsigned int BG_SailingGerstnerRevision(
	const sailingOcean_t *ocean ) {
	unsigned int revision = 2166136261u;

	revision = ( revision ^ ocean->waveSeed ) * 16777619u;
	revision = ( revision ^ (unsigned int)ocean->gerstnerWaveCount )
		* 16777619u;
	revision = BG_SailingGerstnerRevisionFloat( revision, ocean->amplitude );
	revision = BG_SailingGerstnerRevisionFloat( revision, ocean->wavelength );
	revision = BG_SailingGerstnerRevisionFloat( revision, ocean->speed );
	revision = BG_SailingGerstnerRevisionFloat( revision, ocean->slosh );
	revision = BG_SailingGerstnerRevisionFloat( revision, ocean->direction[0] );
	revision = BG_SailingGerstnerRevisionFloat( revision, ocean->direction[1] );
	return revision ? revision : 1u;
}

static sailingGerstnerState_t *BG_SailingGerstnerStateForOcean(
	const sailingOcean_t *ocean ) {
	int i;

	for ( i = 0; i < bgNumSailingOceans; i++ ) {
		if ( ocean == &bgSailingOceans[i] ) {
			return &bgSailingGerstnerStates[i];
		}
	}
	return NULL;
}

static void BG_SailingInitializeGerstner( const sailingOcean_t *ocean,
	sailingGerstnerState_t *state, unsigned int revision ) {
	static const float wavelengthRatio[SAILING_GERSTNER_MAX_WAVES] = {
		1.00f, 1.31f, 0.79f, 1.72f, 0.61f, 2.18f,
		0.48f, 1.47f, 0.70f, 2.65f, 0.39f, 1.93f
	};
	static const float amplitudeWeight[SAILING_GERSTNER_MAX_WAVES] = {
		1.00f, 0.76f, 0.64f, 0.48f, 0.39f, 0.31f,
		0.25f, 0.21f, 0.17f, 0.14f, 0.11f, 0.09f
	};
	static const float directionOffset[SAILING_GERSTNER_MAX_WAVES] = {
		0.0f, 13.0f, -17.0f, 27.0f, -31.0f, 39.0f,
		-43.0f, 8.0f, -10.0f, 51.0f, -55.0f, 22.0f
	};
	float amplitudeWeightSum = 0.0f;
	float baseWaveNumber = (float)( 2.0 * M_PI ) / ocean->wavelength;
	float baseAngularFrequency = baseWaveNumber * ocean->speed;
	float spreadScale = 0.35f + 0.65f * ocean->slosh;
	float amplitudeNormalization;
	float horizontalDerivativeSum = 0.0f;
	int i;

	memset( state, 0, sizeof( *state ) );
	state->waveCount = ocean->gerstnerWaveCount;
	for ( i = 0; i < state->waveCount; i++ ) {
		/*
		 * slosh is the authored blend from one coherent dominant swell into
		 * the directional spectrum. The primary is always present; allowing
		 * every secondary component full energy at slosh zero made long,
		 * high-amplitude weather profiles grow several unintended major
		 * crests.
		 */
		float effectiveWeight = amplitudeWeight[i]
			* ( i == 0 ? 1.0f : ocean->slosh );
		amplitudeWeightSum += effectiveWeight;
	}
	/*
	 * Preserve the map-facing amplitude contract: at zero slosh the primary
	 * Gerstner amplitude is exactly the authored amplitude. As slosh introduces
	 * secondary components they divide, rather than multiply, that same crest
	 * envelope budget.
	 */
	amplitudeNormalization = amplitudeWeightSum > 0.000001f
		? ocean->amplitude / amplitudeWeightSum : 0.0f;
	for ( i = 0; i < state->waveCount; i++ ) {
		sailingGerstnerWave_t *wave = &state->wave[i];
		float effectiveWeight = amplitudeWeight[i]
			* ( i == 0 ? 1.0f : ocean->slosh );
		float wavelengthJitter = i == 0 ? 1.0f
			: 0.92f + 0.16f
				* BG_SailingWaveRandom( ocean->waveSeed, (unsigned int)i,
					0x68bc21ebu );
		float wavelength = ocean->wavelength * wavelengthRatio[i]
			* wavelengthJitter;
		float angleJitter = i == 0 ? 0.0f
			: ( BG_SailingWaveRandom( ocean->waveSeed,
				(unsigned int)i, 0x02e5be93u ) * 2.0f - 1.0f ) * 3.0f;
		float angle = ( directionOffset[i] * spreadScale + angleJitter )
			* (float)M_PI / 180.0f;
		float angleCos = cosf( angle );
		float angleSin = sinf( angle );

		wave->direction[0] = ocean->direction[0] * angleCos
			- ocean->direction[1] * angleSin;
		wave->direction[1] = ocean->direction[0] * angleSin
			+ ocean->direction[1] * angleCos;
		wave->amplitude = amplitudeNormalization * effectiveWeight;
		wave->waveNumber = (float)( 2.0 * M_PI ) / wavelength;
		/* Deep-water dispersion relative to the authored dominant phase speed:
		 * shorter components move more slowly as crests but oscillate faster. */
		wave->angularFrequency = baseAngularFrequency
			* sqrtf( wave->waveNumber / baseWaveNumber );
		wave->phase = i == 0 ? 0.0f
			: BG_SailingWaveRandom( ocean->waveSeed,
				(unsigned int)i, 0x967a889bu ) * (float)( 2.0 * M_PI );
		horizontalDerivativeSum += wave->amplitude * wave->waveNumber;
	}
	/*
	 * Approach, but do not reach, the Gerstner folding limit at authored
	 * steepness 1. Lower steepness is the map-facing control for a broader,
	 * rounder swell.
	 */
	state->horizontalNormalization = horizontalDerivativeSum > 0.000001f
		? 0.9f / horizontalDerivativeSum : 0.0f;
	state->revision = revision;
	state->initialized = qtrue;
}

static qboolean BG_SailingEvaluateGerstnerRaw( const sailingOcean_t *ocean,
	float x, float y, int time, float *waveOffset, float *slopeX,
	float *slopeY, vec3_t waveVelocity ) {
	sailingGerstnerState_t *state =
		BG_SailingGerstnerStateForOcean( ocean );
	unsigned int revision;
	float seconds = (float)time * 0.001f;
	float choppiness;
	float sourceX = x;
	float sourceY = y;
	float j00 = 1.0f;
	float j01 = 0.0f;
	float j10 = 0.0f;
	float j11 = 1.0f;
	float sourceSlopeX = 0.0f;
	float sourceSlopeY = 0.0f;
	float rawHeight = 0.0f;
	float determinant;
	int iteration;
	int i;

	if ( !state ) {
		return qfalse;
	}
	revision = BG_SailingGerstnerRevision( ocean );
	if ( !state->initialized || state->revision != revision ) {
		BG_SailingInitializeGerstner( ocean, state, revision );
	}
	choppiness = ocean->steepness * ocean->horizontalDisplacementScale
		* state->horizontalNormalization;

	for ( iteration = 0;
		iteration < ( choppiness > 0.0001f
			? SAILING_GERSTNER_INVERSION_STEPS : 0 );
		iteration++ ) {
		float displacementX = 0.0f;
		float displacementY = 0.0f;
		float solveJ00 = 1.0f;
		float solveJ01 = 0.0f;
		float solveJ10 = 0.0f;
		float solveJ11 = 1.0f;
		float residualX;
		float residualY;
		float inverseDeterminant;
		float correctionX;
		float correctionY;

		for ( i = 0; i < state->waveCount; i++ ) {
			const sailingGerstnerWave_t *wave = &state->wave[i];
			float phase = wave->waveNumber
				* ( sourceX * wave->direction[0]
					+ sourceY * wave->direction[1] )
				- wave->angularFrequency * seconds + wave->phase;
			float horizontal = choppiness * wave->amplitude * cosf( phase );
			float horizontalDerivative = -choppiness * wave->amplitude
				* wave->waveNumber * sinf( phase );

			displacementX += wave->direction[0] * horizontal;
			displacementY += wave->direction[1] * horizontal;
			solveJ00 += horizontalDerivative * wave->direction[0]
				* wave->direction[0];
			solveJ01 += horizontalDerivative * wave->direction[0]
				* wave->direction[1];
			solveJ10 += horizontalDerivative * wave->direction[1]
				* wave->direction[0];
			solveJ11 += horizontalDerivative * wave->direction[1]
				* wave->direction[1];
		}
		residualX = sourceX + displacementX - x;
		residualY = sourceY + displacementY - y;
		inverseDeterminant = solveJ00 * solveJ11
			- solveJ01 * solveJ10;
		if ( fabsf( inverseDeterminant ) < 0.05f ) {
			inverseDeterminant =
				inverseDeterminant < 0.0f ? -0.05f : 0.05f;
		}
		correctionX = ( solveJ11 * residualX
			- solveJ01 * residualY ) / inverseDeterminant;
		correctionY = ( -solveJ10 * residualX
			+ solveJ00 * residualY ) / inverseDeterminant;
		sourceX -= correctionX;
		sourceY -= correctionY;
		if ( correctionX * correctionX + correctionY * correctionY
			< 0.0001f ) {
			break;
		}
	}

	if ( waveVelocity ) {
		VectorClear( waveVelocity );
	}
	for ( i = 0; i < state->waveCount; i++ ) {
		const sailingGerstnerWave_t *wave = &state->wave[i];
		float phase = wave->waveNumber
			* ( sourceX * wave->direction[0]
				+ sourceY * wave->direction[1] )
			- wave->angularFrequency * seconds + wave->phase;
		float phaseSin = sinf( phase );
		float phaseCos = cosf( phase );
		float slopeFactor = wave->amplitude * wave->waveNumber * phaseCos;
		float horizontalDerivative = -choppiness * wave->amplitude
			* wave->waveNumber * phaseSin;

		rawHeight += wave->amplitude * phaseSin;
		sourceSlopeX += slopeFactor * wave->direction[0];
		sourceSlopeY += slopeFactor * wave->direction[1];
		j00 += horizontalDerivative * wave->direction[0]
			* wave->direction[0];
		j01 += horizontalDerivative * wave->direction[0]
			* wave->direction[1];
		j10 += horizontalDerivative * wave->direction[1]
			* wave->direction[0];
		j11 += horizontalDerivative * wave->direction[1]
			* wave->direction[1];
		if ( waveVelocity ) {
			float horizontalVelocity = choppiness * wave->amplitude
				* wave->angularFrequency * phaseSin;
			waveVelocity[0] += wave->direction[0] * horizontalVelocity;
			waveVelocity[1] += wave->direction[1] * horizontalVelocity;
			waveVelocity[2] -= wave->amplitude
				* wave->angularFrequency * phaseCos;
		}
	}

	/* Convert derivatives in the undisplaced parameter plane to the Eulerian
	 * height gradient at the requested world XY. */
	determinant = j00 * j11 - j01 * j10;
	if ( fabsf( determinant ) < 0.05f ) {
		determinant = determinant < 0.0f ? -0.05f : 0.05f;
	}
	*waveOffset = rawHeight;
	*slopeX = ( j11 * sourceSlopeX - j10 * sourceSlopeY ) / determinant;
	*slopeY = ( -j01 * sourceSlopeX + j00 * sourceSlopeY ) / determinant;
	return qtrue;
}

qboolean BG_SailingEvaluateGerstnerWave( const sailingOcean_t *ocean,
	float x, float y, int time, float *height, vec3_t normal,
	vec3_t waterVelocity ) {
	float waveOffset;
	float slopeX;
	float slopeY;
	vec3_t waveVelocity;

	if ( !ocean ) {
		if ( height ) {
			*height = 0.0f;
		}
		if ( normal ) {
			VectorSet( normal, 0.0f, 0.0f, 1.0f );
		}
		if ( waterVelocity ) {
			VectorClear( waterVelocity );
		}
		return qfalse;
	}

	VectorClear( waveVelocity );
	if ( !BG_SailingEvaluateGerstnerRaw( ocean, x, y, time, &waveOffset,
		&slopeX, &slopeY, waveVelocity ) ) {
		return qfalse;
	}
	if ( height ) {
		*height = ocean->baseHeight + waveOffset;
	}
	if ( normal ) {
		VectorSet( normal, -slopeX, -slopeY, 1.0f );
		VectorNormalize( normal );
	}
	if ( waterVelocity ) {
		VectorCopy( waveVelocity, waterVelocity );
	}
	return qtrue;
}

qboolean BG_SailingGetGerstnerRenderWaves( const sailingOcean_t *ocean,
	sailingGerstnerRenderWave_t *waves, int maxWaves, int *waveCount,
	float *choppiness ) {
	sailingGerstnerState_t *state =
		BG_SailingGerstnerStateForOcean( ocean );
	unsigned int revision;
	int count;
	int i;

	if ( waveCount ) {
		*waveCount = 0;
	}
	if ( choppiness ) {
		*choppiness = 0.0f;
	}
	if ( !ocean || !state || !waves || maxWaves <= 0 ) {
		return qfalse;
	}

	revision = BG_SailingGerstnerRevision( ocean );
	if ( !state->initialized || state->revision != revision ) {
		BG_SailingInitializeGerstner( ocean, state, revision );
	}

	count = state->waveCount < maxWaves ? state->waveCount : maxWaves;
	for ( i = 0; i < count; i++ ) {
		waves[i].direction[0] = state->wave[i].direction[0];
		waves[i].direction[1] = state->wave[i].direction[1];
		waves[i].amplitude = state->wave[i].amplitude;
		waves[i].waveNumber = state->wave[i].waveNumber;
		waves[i].angularFrequency = state->wave[i].angularFrequency;
		waves[i].phase = state->wave[i].phase;
	}
	if ( waveCount ) {
		*waveCount = count;
	}
	if ( choppiness ) {
		*choppiness = ocean->steepness
			* ocean->horizontalDisplacementScale
			* state->horizontalNormalization;
	}
	return qtrue;
}

/*
=============================================================================

THE HYBRID SEA

The surface physics floats in is the one the DinurdoJK client draws for a map
ocean (its Hybrid ocean): Blue Water's FFT wave spectrum (Bill Yang, MIT;
ocean/src/spectrum.rs and sea_state.rs, ported here step for step) plus one
analytic swell, mixed by the map's slosh. Both sides build the same waves from
the same ocean and weather state, so prediction, the server and the drawn
water agree.

The spectrum's two long tiles are each reduced to SAILING_SEA_WAVES_PER_CASCADE
waves with the same total energy (systematic importance sampling over the
lattice modes, seeded); the client transforms exactly these. Its fine tile
(waves under ~6 u) is drawn only and left out here. Like the drawn surface,
each long tile is read as three shifted copies blended on a triangle grid
(Heitz & Neyret stochastic tiling, Blue Water's waves.wgsl `scatter`), and
choppy horizontal motion is inverted with the same fixed-point steps as the
client's surface probe.

Cost: building is a few milliseconds, once per ocean/weather change. A height
query sums at most 2 tiles x 3 copies x 128 waves, a few times for the
choppy inversion: a fixed few tens of microseconds, every query alike.

=============================================================================
*/

#define SS_GRAVITY 9.81
#define SS_PI 3.14159265358979323846
/* The finer tile takes over a band once its lattice has this many modes across
 * the cut radius. */
#define SS_FINE_MODES_AT_CUT 8.0
/* Phillips' saturation range, blended in between these multiples of the peak. */
#define SS_SATURATION 0.02
#define SS_SATURATION_ONSET 1.5
#define SS_SATURATION_FULL 3.0
/* Stokes' highest wave, H / lambda; at 1 / (pi * it) a crest cusps. */
#define SS_STOKES_LIMIT 0.1412
#define SS_PEAK_WAVELENGTHS_PER_TILE 8.0
#define SS_MAX_TILE_GROWTH 4.0
/* Blue Water's default sea: seed, spreading, and the sea its height, wind and
 * spectrum shape are calibrated at. */
#define SS_SEED 1941.0
#define SS_SHARPNESS 0.8
#define SS_DEFAULT_HEIGHT 1.8
#define SS_DEFAULT_WIND 9.0
#define SS_DEFAULT_WAVELENGTH 32.0
/* Canonical lattice modes a physics tile's band can hold (about 3300 used). */
#define SS_BAND_MODES 8192
#define SS_INVERSION_STEPS 4
#define SS_SCATTER_CELL 0.93f
#define SS_SCATTER_SEED 211u
#define SS_SCATTER_FLOOR 0.02f

static const double ssTileBase[SAILING_SEA_CASCADES] = { 1024.0, 181.0, 31.0 };

typedef struct ssKey_s {
	float amplitude;
	float wavelength;
	float speed;
	float steepness;
	float slosh;
	float direction[2];
	float horizontalDisplacementScale;
	int gerstnerWaveCount;
	int hasWeather;
	float windSpeed;
} ssKey_t;

typedef struct ssTimeState_s {
	int time;
	qboolean valid;
	/* a e^{-iwt} per wave. */
	float re[SAILING_SEA_MAX_WAVES];
	float im[SAILING_SEA_MAX_WAVES];
	/* The swell's w * t, reduced. */
	float swellTurn;
} ssTimeState_t;

typedef struct ssMode_s {
	int nx;
	int nz;
	float re;
	float im;
	float m;
	qboolean forward;
} ssMode_t;

static sailingSea_t bgSailingSeas[MAX_SAILING_OCEANS];
static ssKey_t bgSailingSeaKeys[MAX_SAILING_OCEANS];
static qboolean bgSailingSeaBuilt[MAX_SAILING_OCEANS];
static ssTimeState_t bgSailingSeaTimes[MAX_SAILING_OCEANS];
/* Build scratch: each physics tile's canonical modes, in lattice order. */
static ssMode_t ssModes[SAILING_SEA_PHYSICS_CASCADES][SS_BAND_MODES];
static int ssModeCount[SAILING_SEA_PHYSICS_CASCADES];

static double SS_Clamp( double value, double low, double high ) {
	return value < low ? low : value > high ? high : value;
}

/* Donelan, Hamilton & Hui (1985) wind-sea energy per unit w, without level. */
static double SS_Equilibrium( double omega, double peak, double gamma, double age ) {
	double sigma;
	double ratio;
	double r;

	if ( !( omega > 0.0 ) ) {
		return 0.0;
	}
	age = SS_Clamp( age, 0.83, 5.0 );
	sigma = 0.08 * ( 1.0 + 4.0 / ( age * age * age ) );
	r = exp( -( ( omega - peak ) * ( omega - peak ) )
		/ ( 2.0 * sigma * sigma * peak * peak ) );
	ratio = peak / omega;
	return SS_GRAVITY * SS_GRAVITY / ( omega * omega * omega * omega * peak )
		* exp( -( ratio * ratio * ratio * ratio ) ) * pow( gamma, r );
}

/* Energy per unit w of the drawn sea, held to the saturation range above the
 * peak. */
static double SS_SeaSpectrum( double omega, double peak, double gamma,
	double level, double age ) {
	double free = level * SS_Equilibrium( omega, peak, gamma, age );
	double t = SS_Clamp( ( omega / peak - SS_SATURATION_ONSET )
		/ ( SS_SATURATION_FULL - SS_SATURATION_ONSET ), 0.0, 1.0 );
	double cap = SS_SATURATION * SS_GRAVITY * SS_GRAVITY / pow( omega, 5.0 );

	return free + ( ( free < cap ? free : cap ) - free ) * t * t * ( 3.0 - 2.0 * t );
}

static double SS_SpectrumTotal( double level, double peak, double gamma,
	double age ) {
	double sum = 0.0;
	int i;

	for ( i = 0; i <= 600; i++ ) {
		double omega = peak * 0.3 * pow( 100.0, (double)i / 600.0 );
		double weight = ( i == 0 || i == 600 ) ? 0.5 : 1.0;
		sum += SS_SeaSpectrum( omega, peak, gamma, level, age ) * omega * weight;
	}
	return sum * log( 100.0 ) / 600.0;
}

/* The level at which the spectrum holds `variance` (m^2). */
static double SS_SpectrumLevel( double peak, double gamma, double variance,
	double age ) {
	double low = 0.0;
	double high = 1e-3;
	int i;

	while ( SS_SpectrumTotal( high, peak, gamma, age ) < variance && high < 1e30 ) {
		high *= 2.0;
	}
	for ( i = 0; i < 60; i++ ) {
		double middle = ( low + high ) / 2.0;
		if ( SS_SpectrumTotal( middle, peak, gamma, age ) < variance ) {
			low = middle;
		}
		else {
			high = middle;
		}
	}
	return ( low + high ) / 2.0;
}

/* Spreading exponent s of cos^{2s}(theta/2) (Hasselmann et al. 1980). */
static double SS_SpreadingExponent( double omega, double peak, double windSpeed ) {
	double ratio = omega / peak;
	double age = SS_Clamp( windSpeed * peak / SS_GRAVITY, 0.3, 3.0 );
	double s = ratio < 1.05 ? 6.97 * pow( ratio, 4.06 )
		: 9.77 * pow( ratio, -2.33 - 1.45 * ( age - 1.17 ) );

	s *= SS_SHARPNESS;
	return s > 0.0 ? s : 0.0;
}

/* ln Gamma(x) for x > 0 (Lanczos, g = 7). */
static double SS_LogGamma( double x ) {
	static const double c[9] = {
		0.99999999999980993, 676.5203681218851, -1259.1392167224028,
		771.32342877765313, -176.61502916214059, 12.507343278686905,
		-0.13857109526572012, 9.9843695780195716e-6, 1.5056327351493116e-7
	};
	double sum;
	double t;
	int i;

	if ( x < 0.5 ) {
		return log( SS_PI / sin( SS_PI * x ) ) - SS_LogGamma( 1.0 - x );
	}
	x -= 1.0;
	sum = c[0];
	for ( i = 1; i < 9; i++ ) {
		sum += c[i] / ( x + (double)i );
	}
	t = x + 7.5;
	return 0.5 * log( 2.0 * SS_PI ) + ( x + 0.5 ) * log( t ) - t + log( sum );
}

/* Normalisation of |cos(theta/2)|^{2s} over a full turn. */
static double SS_SpreadingNormalization( double s ) {
	return exp( ( 2.0 * s - 1.0 ) * log( 2.0 ) + 2.0 * SS_LogGamma( s + 1.0 )
		- SS_LogGamma( 2.0 * s + 1.0 ) ) / SS_PI;
}

/* PCG output permutation (Jarzynski & Olano 2020). */
static unsigned int SS_Pcg( unsigned int v ) {
	unsigned int state = v * 747796405u + 2891336453u;
	unsigned int word = ( ( state >> ( ( state >> 28u ) + 4u ) ) ^ state ) * 277803737u;
	return ( word >> 22u ) ^ word;
}

static unsigned int SS_ToUint32( double value ) {
	double wrapped = fmod( value, 4294967296.0 );

	if ( wrapped < 0.0 ) {
		wrapped += 4294967296.0;
	}
	return (unsigned int)wrapped;
}

static unsigned int SS_ModeHash( double seed, unsigned int cascade, int nx, int nz ) {
	double whole = seed < 0.0 ? ceil( seed ) : floor( seed );
	unsigned int high = SS_ToUint32( floor( whole / 4294967296.0 ) );
	unsigned int low = SS_ToUint32( whole );

	return SS_Pcg( (unsigned int)nz + SS_Pcg( (unsigned int)nx
		+ SS_Pcg( cascade + SS_Pcg( low + SS_Pcg( high ) ) ) ) );
}

static double SS_Unit( unsigned int bits ) {
	return ( (double)bits + 0.5 ) / 4294967296.0;
}

/* Blue Water's spectrum shape (gamma) at its calibrated default sea. */
static double SS_DefaultGamma( void ) {
	double fetch = ( SS_DEFAULT_HEIGHT / 4.0 ) * ( SS_DEFAULT_HEIGHT / 4.0 )
		* SS_GRAVITY * SS_GRAVITY / pow( SS_DEFAULT_WIND, 4.0 ) / 1.6e-7;
	double frequency = 3.5 * SS_GRAVITY / SS_DEFAULT_WIND * pow( fetch, -0.33 );
	double wavelength = SS_GRAVITY / ( 2.0 * SS_PI * frequency * frequency );
	double age;

	if ( !( wavelength > 0.0 ) ) {
		wavelength = SS_DEFAULT_WAVELENGTH;
	}
	age = SS_DEFAULT_WIND / sqrt( SS_GRAVITY * wavelength / ( 2.0 * SS_PI ) );
	return 1.7 + 6.0 * log10( SS_Clamp( age, 1.0, 5.0 ) );
}

/* sin and cos together: quadrant reduction plus Cephes' single-precision
 * polynomials (about 1e-7). */
static void SS_SinCos( float x, float *s, float *c ) {
	/* Round to the nearest quadrant without a branch or library call: adding
	 * 1.5 * 2^23 leaves the integer in the low mantissa bits. */
	volatile float shifted = x * 0.63661977236758134f + 12582912.0f;
	float quadrantF = shifted - 12582912.0f;
	int quadrant = (int)quadrantF;
	float r = ( ( x - quadrantF * 1.5703125f )
		- quadrantF * 4.837512969970703125e-4f )
		- quadrantF * 7.549789948768648e-8f;
	float r2 = r * r;
	float sinR = r + r * r2 * ( -1.6666654611e-1f
		+ r2 * ( 8.3321608736e-3f + r2 * -1.9515295891e-4f ) );
	float cosR = 1.0f - 0.5f * r2 + r2 * r2 * ( 4.166664568298827e-2f
		+ r2 * ( -1.388731625493765e-3f + r2 * 2.443315711809948e-5f ) );
	/* Odd quadrants swap sin and cos (cos negated); quadrants 2 and 3 negate. */
	float odd = (float)( quadrant & 1 );
	float sign = 1.0f - 2.0f * (float)( ( quadrant >> 1 ) & 1 );

	*s = ( sinR + odd * ( cosR - sinR ) ) * sign;
	*c = ( cosR - odd * ( sinR + cosR ) ) * sign;
}

/* The map ocean as Hybrid draws it (DinurdoJK AuthoredOcean::apply_to_hybrid
 * and native_waves.rs BlueWaterWaves::sanitize). */
static void SS_Parameters( const sailingOcean_t *ocean, const ssKey_t *key,
	sailingSea_t *sea ) {
	static const float weight[SAILING_GERSTNER_MAX_WAVES] = {
		1.00f, 0.76f, 0.64f, 0.48f, 0.39f, 0.31f,
		0.25f, 0.21f, 0.17f, 0.14f, 0.11f, 0.09f
	};
	float sum = 0.0f;
	float squares = 0.0f;
	float sigma;
	float direction;
	int count = key->gerstnerWaveCount < 2 ? 2
		: key->gerstnerWaveCount > SAILING_GERSTNER_MAX_WAVES
			? SAILING_GERSTNER_MAX_WAVES : key->gerstnerWaveCount;
	int i;

	(void)ocean;
	/* The full directional set's RMS height (jaPRO's Gerstner amplitude
	 * envelope at slosh 1) sets the spectrum's significant height 4 sigma. */
	for ( i = 0; i < count; i++ ) {
		sum += weight[i];
		squares += weight[i] * weight[i];
	}
	sigma = key->amplitude * sqrtf( squares * 0.5f ) / sum;
	sea->significantHeight = (float)SS_Clamp(
		4.0f * sigma / SAILING_SEA_UNITS_PER_METRE, 0.0, 60.0 );

	direction = atan2f( key->direction[1], key->direction[0] ) * 180.0f / (float)M_PI;
	direction = fmodf( direction + 180.0f, 360.0f );
	if ( direction < 0.0f ) {
		direction += 360.0f;
	}
	sea->directionDegrees = (float)SS_Clamp( direction - 180.0f, -180.0, 180.0 );
	sea->peakWavelength = (float)SS_Clamp(
		key->wavelength / SAILING_SEA_UNITS_PER_METRE, 2.0, 512.0 );
	/* jaPRO's default steepness 0.65 is Blue Water's breaking chop. */
	sea->choppiness = (float)SS_Clamp( (float)( 1.0 / ( SS_PI * SS_STOKES_LIMIT ) )
		* key->steepness / 0.65f, 0.0, 4.0 );
	sea->windSpeed = (float)SS_Clamp( key->hasWeather
		? key->windSpeed / SAILING_SEA_UNITS_PER_METRE : (float)SS_DEFAULT_WIND,
		0.1, 40.0 );
	sea->fftBlend = key->slosh;
	sea->swellAmplitude = key->amplitude;
	sea->swellWavelength = key->wavelength;
	sea->swellSpeed = key->speed;
	sea->swellSteepness = key->steepness * key->horizontalDisplacementScale;
}

/* Blue Water's build_spectrum over the three tiles (the third only for the
 * height normalisation), keeping the two physics tiles' canonical modes. */
static void SS_BuildSpectrum( sailingSea_t *sea ) {
	double height = sea->significantHeight;
	double lambda = sea->peakWavelength;
	double peak = sqrt( SS_GRAVITY * 2.0 * SS_PI / lambda );
	double gamma = SS_DefaultGamma();
	double age = sea->windSpeed * peak / SS_GRAVITY;
	double level;
	double windRadians = (double)sea->directionDegrees * ( SS_PI / 180.0 );
	double sinWind = sin( windRadians );
	double cosWind = cos( windRadians );
	double growth;
	double size[SAILING_SEA_CASCADES];
	double bandLo[SAILING_SEA_CASCADES];
	double bandHi[SAILING_SEA_CASCADES];
	double drawn = 0.0;
	double scale;
	int n = SAILING_SEA_RESOLUTION;
	int half = n / 2;
	int c;
	int i;

	growth = SS_Clamp( SS_PEAK_WAVELENGTHS_PER_TILE * lambda / ssTileBase[0],
		1.0, SS_MAX_TILE_GROWTH );
	for ( c = 0; c < SAILING_SEA_CASCADES; c++ ) {
		size[c] = ssTileBase[c] * growth;
		sea->tileSize[c] = (float)size[c];
	}
	for ( c = 0; c < SAILING_SEA_CASCADES; c++ ) {
		double nyquist = SS_PI * (double)n / size[c];
		bandLo[c] = 0.0;
		bandHi[c] = nyquist;
	}
	for ( c = 1; c < SAILING_SEA_CASCADES; c++ ) {
		double cut = SS_FINE_MODES_AT_CUT * 2.0 * SS_PI / size[c];
		double coarseHalf = SS_PI * (double)n / size[c - 1] / 2.0;
		if ( coarseHalf < cut ) {
			cut = coarseHalf;
		}
		bandHi[c - 1] = cut;
		bandLo[c] = cut;
	}

	ssModeCount[0] = ssModeCount[1] = 0;
	if ( !( height > 0.0 ) || !( lambda > 0.0 ) ) {
		return;
	}
	level = SS_SpectrumLevel( peak, gamma, ( height / 4.0 ) * ( height / 4.0 ), age );

	for ( c = 0; c < SAILING_SEA_CASCADES; c++ ) {
		double dk = 2.0 * SS_PI / size[c];
		int z;
		int x;

		for ( z = 0; z < n; z++ ) {
			int nz = z < half ? z : z - n;
			for ( x = 0; x < n; x++ ) {
				int nx = x < half ? x : x - n;
				double kx, kz, k, omega, s, cosTheta, toward, away;
				double density, pair, radius, angle, amplitude, re, im;
				unsigned int bits;
				qboolean forward;

				/* One draw per +/- pair, from its canonical member. */
				if ( nz < 0 || ( nz == 0 && nx <= 0 ) ) {
					continue;
				}
				kx = (double)nx * dk;
				kz = (double)nz * dk;
				k = sqrt( kx * kx + kz * kz );
				if ( k < bandLo[c] || k >= bandHi[c] || nx == -half ) {
					continue;
				}
				omega = sqrt( SS_GRAVITY * k );
				s = SS_SpreadingExponent( omega, peak, sea->windSpeed );
				cosTheta = ( kx * cosWind + kz * sinWind ) / k;
				toward = pow( sqrt( fmax( ( 1.0 + cosTheta ) / 2.0, 0.0 ) ), 2.0 * s );
				away = pow( sqrt( fmax( ( 1.0 - cosTheta ) / 2.0, 0.0 ) ), 2.0 * s );
				density = SS_SeaSpectrum( omega, peak, gamma, level, age )
					* SS_GRAVITY / ( 2.0 * omega ) * SS_SpreadingNormalization( s )
					/ k * dk * dk;
				pair = density * ( toward + away );
				if ( !( pair > 0.0 ) ) {
					continue;
				}
				bits = SS_ModeHash( SS_SEED, (unsigned int)c, nx, nz );
				radius = sqrt( -2.0 * log( SS_Unit( SS_Pcg( bits + 1u ) ) ) );
				angle = 2.0 * SS_PI * SS_Unit( SS_Pcg( bits + 2u ) );
				amplitude = sqrt( pair / 4.0 ) * radius;
				re = amplitude * cos( angle );
				im = amplitude * sin( angle );
				forward = SS_Unit( SS_Pcg( bits + 3u ) ) * ( toward + away ) < toward
					? qtrue : qfalse;
				drawn += 2.0 * ( re * re + im * im );
				if ( c < SAILING_SEA_PHYSICS_CASCADES && ssModeCount[c] < SS_BAND_MODES ) {
					ssMode_t *mode = &ssModes[c][ssModeCount[c]++];
					mode->nx = nx;
					mode->nz = nz;
					mode->re = (float)re;
					mode->im = (float)im;
					mode->m = (float)floor( sqrt( SS_GRAVITY * k )
						* SAILING_SEA_FOLD_PERIOD / ( 2.0 * SS_PI ) + 0.5 );
					mode->forward = forward;
				}
			}
		}
	}

	/* Normalise to exactly the requested significant height. */
	scale = drawn > 0.0 ? height / ( 4.0 * sqrt( drawn ) ) : 0.0;
	for ( c = 0; c < SAILING_SEA_PHYSICS_CASCADES; c++ ) {
		for ( i = 0; i < ssModeCount[c]; i++ ) {
			ssModes[c][i].re = (float)( (double)ssModes[c][i].re * scale );
			ssModes[c][i].im = (float)( (double)ssModes[c][i].im * scale );
		}
	}
}

/* Each physics tile rebuilt from SAILING_SEA_WAVES_PER_CASCADE waves:
 * evenly spaced picks through the modes' cumulative energy (a seeded start),
 * each pick an equal share, merged where a strong mode is picked more than
 * once. Kept modes keep their phase, direction and wavelength. */
static void SS_SparseWaves( sailingSea_t *sea ) {
	int c;

	sea->waveCount = 0;
	for ( c = 0; c < SAILING_SEA_PHYSICS_CASCADES; c++ ) {
		double total = 0.0;
		double step;
		double next;
		double cumulative = 0.0;
		double dk = 2.0 * SS_PI / (double)sea->tileSize[c];
		int count = SAILING_SEA_WAVES_PER_CASCADE;
		int i;

		for ( i = 0; i < ssModeCount[c]; i++ ) {
			const ssMode_t *mode = &ssModes[c][i];
			total += (double)mode->re * mode->re + (double)mode->im * mode->im;
		}
		if ( total <= 0.0 ) {
			continue;
		}
		/* A tile with no more modes than waves keeps all of them. */
		if ( count >= ssModeCount[c] ) {
			count = 0;
			step = 0.0;
		}
		else {
			step = total / (double)count;
		}
		next = fmod( SS_SEED * 0.618033988749895 + 0.5
			+ (double)c * 0.754877666246693, 1.0 ) * step;
		for ( i = 0; i < ssModeCount[c]; i++ ) {
			const ssMode_t *mode = &ssModes[c][i];
			double energy = (double)mode->re * mode->re
				+ (double)mode->im * mode->im;
			int picks = 0;
			double gain;
			sailingSeaWave_t *wave;

			if ( energy <= 0.0 ) {
				continue;
			}
			if ( count ) {
				cumulative += energy;
				while ( next < cumulative ) {
					picks++;
					next += step;
				}
				gain = picks ? sqrt( (double)picks * step / energy ) : 0.0;
			}
			else {
				picks = 1;
				gain = 1.0;
			}
			if ( !picks || sea->waveCount >= SAILING_SEA_MAX_WAVES ) {
				continue;
			}
			wave = &sea->wave[sea->waveCount++];
			sea->cascadeWaves[c]++;
			wave->cascade = c;
			/* The +m texel is the canonical one for a forward-travelling draw,
			 * else its twin; it holds (re, im) either way. */
			wave->nx = mode->forward ? mode->nx : -mode->nx;
			wave->nz = mode->forward ? mode->nz : -mode->nz;
			wave->re = (float)( (double)mode->re * gain );
			wave->im = (float)( (double)mode->im * gain );
			wave->m = mode->m;
			wave->kx = (float)( (double)wave->nx * dk );
			wave->kz = (float)( (double)wave->nz * dk );
			wave->k = sqrtf( wave->kx * wave->kx + wave->kz * wave->kz );
		}
	}
}

static int SS_OceanIndex( const sailingOcean_t *ocean ) {
	int i;

	for ( i = 0; i < bgNumSailingOceans; i++ ) {
		if ( ocean == &bgSailingOceans[i] ) {
			return i;
		}
	}
	return -1;
}

const sailingSea_t *BG_SailingSea( const sailingOcean_t *ocean ) {
	const dynamicWeather_t *weather;
	sailingSea_t *sea;
	ssKey_t key;
	int index = ocean ? SS_OceanIndex( ocean ) : -1;

	if ( index < 0 ) {
		return NULL;
	}
	weather = BG_DynamicWeather( 0 );
	memset( &key, 0, sizeof( key ) );
	key.amplitude = ocean->amplitude;
	key.wavelength = ocean->wavelength;
	key.speed = ocean->speed;
	key.steepness = ocean->steepness;
	key.slosh = ocean->slosh;
	key.direction[0] = ocean->direction[0];
	key.direction[1] = ocean->direction[1];
	key.horizontalDisplacementScale = ocean->horizontalDisplacementScale;
	key.gerstnerWaveCount = ocean->gerstnerWaveCount;
	key.hasWeather = weather ? 1 : 0;
	key.windSpeed = weather ? weather->windSpeed : 0.0f;

	sea = &bgSailingSeas[index];
	if ( bgSailingSeaBuilt[index]
		&& !memcmp( &key, &bgSailingSeaKeys[index], sizeof( key ) ) ) {
		return sea;
	}
	memset( sea, 0, sizeof( *sea ) );
	SS_Parameters( ocean, &key, sea );
	SS_BuildSpectrum( sea );
	SS_SparseWaves( sea );
	sea->valid = qtrue;
	bgSailingSeaKeys[index] = key;
	bgSailingSeaBuilt[index] = qtrue;
	bgSailingSeaTimes[index].valid = qfalse;
	return sea;
}

/* Each wave's a e^{-iwt} at `time`, and the swell's w t, once per time. */
static const ssTimeState_t *SS_TimeState( int index, const sailingSea_t *sea,
	int time ) {
	ssTimeState_t *state = &bgSailingSeaTimes[index];
	double seconds;
	double phase;
	int i;

	if ( state->valid && state->time == time ) {
		return state;
	}
	seconds = (double)time * 0.001;
	phase = fmod( seconds, SAILING_SEA_FOLD_PERIOD ) / SAILING_SEA_FOLD_PERIOD;
	if ( phase < 0.0 ) {
		phase += 1.0;
	}
	for ( i = 0; i < sea->waveCount; i++ ) {
		const sailingSeaWave_t *wave = &sea->wave[i];
		double turns = (double)wave->m * phase;
		double turn = ( turns - floor( turns ) ) * 2.0 * SS_PI;
		float c = (float)cos( turn );
		float s = (float)sin( turn );
		state->re[i] = wave->re * c + wave->im * s;
		state->im[i] = wave->im * c - wave->re * s;
	}
	if ( sea->swellWavelength > 0.0f ) {
		double turn = 2.0 * SS_PI * (double)sea->swellSpeed
			/ (double)sea->swellWavelength * seconds;
		state->swellTurn = (float)fmod( turn, 2.0 * SS_PI );
	}
	else {
		state->swellTurn = 0.0f;
	}
	state->time = time;
	state->valid = qtrue;
	return state;
}

typedef struct ssSample_s {
	/* Displacement of grid point g (m): horizontal and height. */
	float dx;
	float dz;
	float h;
	/* dh/dg. */
	float gx;
	float gz;
	/* dD/dg (symmetric). */
	float jxx;
	float jxz;
	float jzz;
	/* dD/dt. */
	float vx;
	float vz;
	float vh;
} ssSample_t;

static unsigned int SS_NoiseBits( int x, int y, unsigned int seed ) {
	unsigned int h0 = ( (unsigned int)x * 0x27d4eb2du )
		^ ( (unsigned int)y * 0x165667b1u ) ^ seed;
	unsigned int h1 = ( h0 ^ ( h0 >> 15u ) ) * 0x85ebca6bu;
	unsigned int h2 = ( h1 ^ ( h1 >> 13u ) ) * 0xc2b2ae35u;
	return h2 ^ ( h2 >> 16u );
}

/* Blue Water's `scatter`: where tile `cascade`'s three copies are read at grid
 * point (x, z) (as offsets in metres) and their weights (sum of squares 1). */
static void SS_Scatter( float x, float z, float tile, int cascade,
	float offset[3][2], float weight[3] ) {
	float px = ( x * 0.8f - z * 0.6f ) / ( tile * SS_SCATTER_CELL );
	float pz = ( x * 0.6f + z * 0.8f ) / ( tile * SS_SCATTER_CELL );
	float skewedX = px - pz * 0.57735027f;
	float skewedZ = pz * 1.15470054f;
	float baseX = floorf( skewedX );
	float baseZ = floorf( skewedZ );
	float fx = skewedX - baseX;
	float fz = skewedZ - baseZ;
	float w = 1.0f - fx - fz;
	float bary[3];
	float vertex[3][2];
	float raised[3];
	float largest;
	float length;
	unsigned int seed = ( SS_SCATTER_SEED + (unsigned int)cascade ) * 0x9e3779b9u;
	int i;

	if ( w > 0.0f ) {
		bary[0] = w; bary[1] = fz; bary[2] = fx;
		vertex[0][0] = baseX; vertex[0][1] = baseZ;
		vertex[1][0] = baseX; vertex[1][1] = baseZ + 1.0f;
		vertex[2][0] = baseX + 1.0f; vertex[2][1] = baseZ;
	}
	else {
		bary[0] = -w; bary[1] = 1.0f - fz; bary[2] = 1.0f - fx;
		vertex[0][0] = baseX + 1.0f; vertex[0][1] = baseZ + 1.0f;
		vertex[1][0] = baseX + 1.0f; vertex[1][1] = baseZ;
		vertex[2][0] = baseX; vertex[2][1] = baseZ + 1.0f;
	}
	largest = 0.0f;
	for ( i = 0; i < 3; i++ ) {
		raised[i] = bary[i] * bary[i] * bary[i];
		if ( raised[i] > largest ) {
			largest = raised[i];
		}
	}
	length = 0.0f;
	for ( i = 0; i < 3; i++ ) {
		if ( raised[i] < largest * SS_SCATTER_FLOOR ) {
			raised[i] = 0.0f;
		}
		length += raised[i] * raised[i];
	}
	length = sqrtf( length );
	for ( i = 0; i < 3; i++ ) {
		int vx = (int)vertex[i][0];
		int vz = (int)vertex[i][1];
		weight[i] = length > 0.0f ? raised[i] / length : ( i == 0 ? 1.0f : 0.0f );
		offset[i][0] = (float)SS_NoiseBits( vx, vz, seed )
			* ( 1.0f / 4294967296.0f ) * tile;
		offset[i][1] = (float)SS_NoiseBits( vx, vz, seed ^ 0x5bd1e995u )
			* ( 1.0f / 4294967296.0f ) * tile;
	}
}

/* One copy of one physics tile at (x, z), weighted. */
static void SS_AddTile( const sailingSea_t *sea, const ssTimeState_t *state,
	int cascade, float x, float z, float weight, ssSample_t *out ) {
	float lambda = sea->choppiness;
	int first = cascade ? sea->cascadeWaves[0] : 0;
	int last = first + sea->cascadeWaves[cascade];
	int i;

	for ( i = first; i < last; i++ ) {
		const sailingSeaWave_t *wave = &sea->wave[i];
		float s;
		float c;
		float realPart;
		float imagPart;
		float omega;
		float hx;
		float hz;

		SS_SinCos( wave->kx * x + wave->kz * z, &s, &c );
		/* a_t e^{i theta} = realPart + i imagPart. */
		realPart = ( state->re[i] * c - state->im[i] * s ) * 2.0f * weight;
		imagPart = ( state->re[i] * s + state->im[i] * c ) * 2.0f * weight;
		omega = (float)( 2.0 * SS_PI / SAILING_SEA_FOLD_PERIOD ) * wave->m;
		hx = wave->k > 0.0f ? lambda * wave->kx / wave->k : 0.0f;
		hz = wave->k > 0.0f ? lambda * wave->kz / wave->k : 0.0f;
		out->h += realPart;
		out->dx -= hx * imagPart;
		out->dz -= hz * imagPart;
		out->gx -= wave->kx * imagPart;
		out->gz -= wave->kz * imagPart;
		out->jxx -= hx * wave->kx * realPart;
		out->jxz -= hx * wave->kz * realPart;
		out->jzz -= hz * wave->kz * realPart;
		out->vh += omega * imagPart;
		out->vx += hx * omega * realPart;
		out->vz += hz * omega * realPart;
	}
}

/* The Hybrid surface's displacement of grid point (x, z): the physics tiles'
 * copies at the FFT weight, plus the analytic swell (DinurdoJK
 * hybrid_swell_uniform and the native mesh's jka_native_raw_offset). */
static void SS_Sample( const sailingSea_t *sea, const ssTimeState_t *state,
	float x, float z, ssSample_t *out ) {
	float blend = sea->fftBlend;
	int c;

	memset( out, 0, sizeof( *out ) );
	if ( blend > 0.0f ) {
		for ( c = 0; c < SAILING_SEA_PHYSICS_CASCADES; c++ ) {
			float offset[3][2];
			float weight[3];
			int copy;

			SS_Scatter( x, z, sea->tileSize[c], c, offset, weight );
			for ( copy = 0; copy < 3; copy++ ) {
				if ( weight[copy] > 0.0f ) {
					SS_AddTile( sea, state, c, x + offset[copy][0],
						z + offset[copy][1], weight[copy] * blend, out );
				}
			}
		}
	}
	if ( blend < 1.0f && sea->swellWavelength > 0.0f && sea->swellAmplitude > 0.0f ) {
		float radians = sea->directionDegrees * (float)M_PI / 180.0f;
		float dirX = cosf( radians );
		float dirZ = -sinf( radians );
		float k = 2.0f * (float)M_PI * SAILING_SEA_UNITS_PER_METRE / sea->swellWavelength;
		float omega = 2.0f * (float)M_PI * sea->swellSpeed / sea->swellWavelength;
		float amplitude = sea->swellAmplitude / SAILING_SEA_UNITS_PER_METRE
			* ( 1.0f - blend );
		float denominator = sea->swellAmplitude * 2.0f * (float)M_PI
			/ sea->swellWavelength;
		float horizontal = 0.9f * sea->swellSteepness
			/ ( denominator > 0.00001f ? denominator : 0.00001f );
		float s;
		float cphase;
		float along;

		SS_SinCos( ( x * dirX + z * dirZ ) * k - state->swellTurn, &s, &cphase );
		along = amplitude * horizontal * cphase;
		out->h += amplitude * s;
		out->dx += dirX * along;
		out->dz += dirZ * along;
		out->gx += amplitude * k * cphase * dirX;
		out->gz += amplitude * k * cphase * dirZ;
		out->jxx -= amplitude * horizontal * k * s * dirX * dirX;
		out->jxz -= amplitude * horizontal * k * s * dirX * dirZ;
		out->jzz -= amplitude * horizontal * k * s * dirZ * dirZ;
		out->vh -= amplitude * omega * cphase;
		out->vx += dirX * amplitude * horizontal * omega * s;
		out->vz += dirZ * amplitude * horizontal * omega * s;
	}
}

/* Height offset (map units), Eulerian slope and water velocity of the Hybrid
 * sea at map (x, y): the grid point the choppy surface moves onto (x, y) is
 * found as the client's surface probe finds it. */
static qboolean BG_SailingEvaluateSeaRaw( const sailingOcean_t *ocean,
	float x, float y, int time, float *waveOffset, float *slopeX,
	float *slopeY, vec3_t waveVelocity ) {
	const sailingSea_t *sea = BG_SailingSea( ocean );
	const ssTimeState_t *state;
	ssSample_t sample;
	float targetX = x / SAILING_SEA_UNITS_PER_METRE;
	float targetZ = -y / SAILING_SEA_UNITS_PER_METRE;
	float gridX = targetX;
	float gridZ = targetZ;
	float jxx;
	float jzz;
	float determinant;
	float worldSlopeX;
	float worldSlopeZ;
	int step;

	if ( !sea || !sea->valid ) {
		return qfalse;
	}
	state = SS_TimeState( SS_OceanIndex( ocean ), sea, time );
	/* Newton steps on grid + D(grid) = target, from the Jacobian each sample
	 * carries; the same root the client probe's fixed-point steps reach. */
	SS_Sample( sea, state, gridX, gridZ, &sample );
	for ( step = 0; step < SS_INVERSION_STEPS; step++ ) {
		float residualX = gridX + sample.dx - targetX;
		float residualZ = gridZ + sample.dz - targetZ;
		float correctionX;
		float correctionZ;

		jxx = 1.0f + sample.jxx;
		jzz = 1.0f + sample.jzz;
		determinant = jxx * jzz - sample.jxz * sample.jxz;
		if ( fabsf( determinant ) < 0.05f ) {
			determinant = determinant < 0.0f ? -0.05f : 0.05f;
		}
		correctionX = ( jzz * residualX - sample.jxz * residualZ ) / determinant;
		correctionZ = ( -sample.jxz * residualX + jxx * residualZ ) / determinant;
		/* Within 5 cm: under 2 u of height even on the steepest faces. */
		if ( residualX * residualX + residualZ * residualZ < 0.0025f ) {
			break;
		}
		gridX -= correctionX;
		gridZ -= correctionZ;
		SS_Sample( sea, state, gridX, gridZ, &sample );
	}

	jxx = 1.0f + sample.jxx;
	jzz = 1.0f + sample.jzz;
	determinant = jxx * jzz - sample.jxz * sample.jxz;
	if ( fabsf( determinant ) < 0.05f ) {
		determinant = determinant < 0.0f ? -0.05f : 0.05f;
	}
	worldSlopeX = ( jzz * sample.gx - sample.jxz * sample.gz ) / determinant;
	worldSlopeZ = ( -sample.jxz * sample.gx + jxx * sample.gz ) / determinant;

	*waveOffset = sample.h * SAILING_SEA_UNITS_PER_METRE;
	*slopeX = worldSlopeX;
	*slopeY = -worldSlopeZ;
	if ( waveVelocity ) {
		waveVelocity[0] = sample.vx * SAILING_SEA_UNITS_PER_METRE;
		waveVelocity[1] = -sample.vz * SAILING_SEA_UNITS_PER_METRE;
		waveVelocity[2] = sample.vh * SAILING_SEA_UNITS_PER_METRE;
	}
	return qtrue;
}

void BG_SailingEvaluateWave( const sailingOcean_t *ocean, float x, float y, int time,
	float *height, vec3_t normal, vec3_t waterVelocity ) {
	float waveOffset;
	float waveScale;
	vec2_t waveScaleGradient;
	float currentSurfaceOffset;
	float currentWaveScale;
	vec2_t currentSurfaceGradient;
	vec2_t currentWaveScaleGradient;
	float slopeX;
	float slopeY;
	vec3_t waveVelocity;

	if ( !ocean ) {
		if ( height ) {
			*height = 0.0f;
		}
		if ( normal ) {
			VectorSet( normal, 0.0f, 0.0f, 1.0f );
		}
		if ( waterVelocity ) {
			VectorClear( waterVelocity );
		}
		return;
	}

	VectorClear( waveVelocity );
	if ( !BG_SailingEvaluateSeaRaw( ocean, x, y, time, &waveOffset,
		&slopeX, &slopeY, waveVelocity ) ) {
		waveOffset = 0.0f;
		slopeX = 0.0f;
		slopeY = 0.0f;
	}

	BG_SailingShoreInfluenceInternal( ocean, x, y, &waveScale,
		waveScaleGradient, NULL, NULL );
	BG_SailingCurrentInfluence( ocean, x, y, &currentSurfaceOffset,
		&currentWaveScale, currentSurfaceGradient, currentWaveScaleGradient,
		NULL );
	waveScaleGradient[0] = waveScaleGradient[0] * currentWaveScale
		+ waveScale * currentWaveScaleGradient[0];
	waveScaleGradient[1] = waveScaleGradient[1] * currentWaveScale
		+ waveScale * currentWaveScaleGradient[1];
	waveScale *= currentWaveScale;
	slopeX = slopeX * waveScale + waveOffset * waveScaleGradient[0];
	slopeY = slopeY * waveScale + waveOffset * waveScaleGradient[1];
	slopeX += currentSurfaceGradient[0];
	slopeY += currentSurfaceGradient[1];
	if ( height ) {
		*height = ocean->baseHeight + waveOffset * waveScale
			+ currentSurfaceOffset;
	}

	if ( normal ) {
		VectorSet( normal, -slopeX, -slopeY, 1.0f );
		VectorNormalize( normal );
	}
	if ( waterVelocity ) {
		VectorCopy( waveVelocity, waterVelocity );
		VectorScale( waterVelocity, waveScale, waterVelocity );
	}
}
void BG_SailingEvaluateBoatHull( const sailingOcean_t *ocean, const vec3_t origin,
	float yaw, float hullLength, float hullWidth, int time, sailingBoatHull_t *hull ) {
	vec3_t angles;
	vec3_t forward;
	vec3_t right;
	vec3_t centerNormal;
	vec3_t sampleNormal;
	vec3_t sampleVelocity;
	float endDistance;
	float endpointPitch;
	float centerPitch;
	float slopeSumX;
	float slopeSumY;
	float velocitySumZ;
	int i;

	if ( !hull ) {
		return;
	}

	memset( hull, 0, sizeof( *hull ) );
	if ( !ocean || !origin ) {
		return;
	}
	for ( i = 0; i < SAILING_HULL_CACHE_SIZE; i++ ) {
		const sailingHullCache_t *cached = &bgSailingHullCache[i];
		if ( cached->valid && cached->ocean == ocean
			&& cached->origin[0] == origin[0]
			&& cached->origin[1] == origin[1]
			&& cached->origin[2] == origin[2]
			&& cached->yaw == yaw
			&& cached->hullLength == hullLength
			&& cached->hullWidth == hullWidth
			&& cached->time == time ) {
			*hull = cached->hull;
			return;
		}
	}

	VectorSet( angles, 0.0f, yaw, 0.0f );
	AngleVectors( angles, forward, right, NULL );
	endDistance = hullLength * 0.45f;
	VectorCopy( origin, hull->centerPoint );
	VectorMA( origin, endDistance, forward, hull->bowPoint );
	VectorMA( origin, -endDistance, forward, hull->sternPoint );
	VectorMA( origin, -hullWidth * 0.45f, right, hull->leftPoint );
	VectorMA( origin, hullWidth * 0.45f, right, hull->rightPoint );

	BG_SailingEvaluateWave( ocean, hull->centerPoint[0], hull->centerPoint[1], time,
		&hull->centerHeight, centerNormal, hull->centerWaterVelocity );
	slopeSumX = -centerNormal[0] / centerNormal[2];
	slopeSumY = -centerNormal[1] / centerNormal[2];
	velocitySumZ = hull->centerWaterVelocity[2];
	BG_SailingEvaluateWave( ocean, hull->bowPoint[0], hull->bowPoint[1], time,
		&hull->bowHeight, sampleNormal, sampleVelocity );
	slopeSumX += -sampleNormal[0] / sampleNormal[2];
	slopeSumY += -sampleNormal[1] / sampleNormal[2];
	velocitySumZ += sampleVelocity[2];
	BG_SailingEvaluateWave( ocean, hull->sternPoint[0], hull->sternPoint[1], time,
		&hull->sternHeight, sampleNormal, sampleVelocity );
	slopeSumX += -sampleNormal[0] / sampleNormal[2];
	slopeSumY += -sampleNormal[1] / sampleNormal[2];
	velocitySumZ += sampleVelocity[2];
	BG_SailingEvaluateWave( ocean, hull->leftPoint[0], hull->leftPoint[1], time,
		&hull->leftHeight, sampleNormal, sampleVelocity );
	slopeSumX += -sampleNormal[0] / sampleNormal[2];
	slopeSumY += -sampleNormal[1] / sampleNormal[2];
	velocitySumZ += sampleVelocity[2];
	BG_SailingEvaluateWave( ocean, hull->rightPoint[0], hull->rightPoint[1], time,
		&hull->rightHeight, sampleNormal, sampleVelocity );
	slopeSumX += -sampleNormal[0] / sampleNormal[2];
	slopeSumY += -sampleNormal[1] / sampleNormal[2];
	velocitySumZ += sampleVelocity[2];
	hull->averageSlope[0] = slopeSumX * 0.2f;
	hull->averageSlope[1] = slopeSumY * 0.2f;
	hull->averageVerticalVelocity = velocitySumZ * 0.2f;

	/* Bow/stern gives the waterline across the full hull. Blend it with the
	 * already-sampled center tangent so a long hull near one wavelength cannot
	 * alias two similar endpoint heights into a falsely level pitch. */
	endpointPitch = -atan2f( hull->bowHeight - hull->sternHeight,
		hullLength * 0.9f ) * ( 180.0f / (float)M_PI );
	centerPitch = atan2f( DotProduct( centerNormal, forward ), centerNormal[2] )
		* ( 180.0f / (float)M_PI );
	hull->pitch = endpointPitch * 0.5f + centerPitch * 0.5f;
	hull->roll = -atan2f( hull->rightHeight - hull->leftHeight,
		hullWidth * 0.9f ) * ( 180.0f / (float)M_PI );

	{
		sailingHullCache_t *cached =
			&bgSailingHullCache[bgSailingHullCacheNext];
		cached->valid = qtrue;
		cached->ocean = ocean;
		VectorCopy( origin, cached->origin );
		cached->yaw = yaw;
		cached->hullLength = hullLength;
		cached->hullWidth = hullWidth;
		cached->time = time;
		cached->hull = *hull;
		bgSailingHullCacheNext = ( bgSailingHullCacheNext + 1 )
			% SAILING_HULL_CACHE_SIZE;
	}
}

void BG_SailingEvaluateBoatLoad( const sailingOcean_t *ocean,
	const sailingBoatHull_t *hull, const vec3_t origin, const vec3_t velocity,
	const vec3_t angles, float hullLength, float hullWidth, float floatHeight,
	float lateralDrag, float sailArea, float sailTrim, int time,
	sailingBoatLoad_t *load ) {
	vec3_t forward;
	vec3_t right;
	vec3_t up;
	vec3_t center;
	vec3_t currentVelocity;
	vec3_t waterVelocity;
	vec3_t relativeVelocity;
	vec3_t windVelocity;
	vec3_t apparentWind;
	vec3_t sailAcceleration;
	vec3_t futurePoint;
	vec3_t futureWaterVelocity;
	vec3_t waveAcceleration;
	float waterHeight[SAILING_DAMAGE_HULL_SAMPLES];
	float lateralSpeed;
	float sailAngle;
	int i;

	if ( !load ) {
		return;
	}
	memset( load, 0, sizeof( *load ) );
	if ( !ocean || !hull || !origin || !velocity || !angles ) {
		return;
	}

	AngleVectors( angles, forward, right, up );
	VectorMA( origin, -floatHeight, up, center );
	VectorCopy( center, load->waterlinePoint[0] );
	VectorMA( center, hullLength * 0.45f, forward,
		load->waterlinePoint[1] );
	VectorMA( center, -hullLength * 0.45f, forward,
		load->waterlinePoint[2] );
	VectorMA( center, -hullWidth * 0.45f, right,
		load->waterlinePoint[3] );
	VectorMA( center, hullWidth * 0.45f, right,
		load->waterlinePoint[4] );
	waterHeight[0] = hull->centerHeight;
	waterHeight[1] = hull->bowHeight;
	waterHeight[2] = hull->sternHeight;
	waterHeight[3] = hull->leftHeight;
	waterHeight[4] = hull->rightHeight;
	for ( i = 0; i < SAILING_DAMAGE_HULL_SAMPLES; i++ ) {
		load->hullDepth[i] = waterHeight[i]
			- load->waterlinePoint[i][2];
	}

	/* The keel load is the acceleration required to reject sideways motion
	 * relative to the moving water. It remains large when sail and keel forces
	 * cancel, which is exactly when the hull and rig are still under strain. */
	BG_SailingCurrentVelocityAt( ocean, hull->centerPoint[0],
		hull->centerPoint[1], time, currentVelocity );
	VectorAdd( hull->centerWaterVelocity, currentVelocity, waterVelocity );
	VectorSubtract( velocity, waterVelocity, relativeVelocity );
	lateralSpeed = DotProduct( relativeVelocity, right );
	load->hullLateralAcceleration = -lateralDrag * lateralSpeed;

	VectorClear( sailAcceleration );
	if ( sailArea > 0.0f ) {
		BG_SailingWindVelocityAt( ocean, hull->centerPoint, time, windVelocity );
		VectorSubtract( windVelocity, velocity, apparentWind );
		sailAngle = angles[YAW] + sailTrim - 90.0f;
		BG_SailingSailAcceleration( apparentWind, sailAngle, sailArea,
			sailAcceleration, NULL, NULL, NULL );
	}
	load->sailLateralAcceleration = DotProduct( sailAcceleration, right );

	/* A rapidly changing orbital water velocity is the part of a beam-on wave
	 * that shoves the whole hull sideways. Sample 50 ms ahead along the boat's
	 * path, matching the existing movement/debug feed-forward interval. */
	VectorMA( hull->centerPoint, 0.05f, velocity, futurePoint );
	BG_SailingEvaluateWave( ocean, futurePoint[0], futurePoint[1], time + 50,
		NULL, NULL, futureWaterVelocity );
	BG_SailingCurrentVelocityAt( ocean, futurePoint[0], futurePoint[1],
		time + 50, currentVelocity );
	VectorAdd( futureWaterVelocity, currentVelocity, futureWaterVelocity );
	VectorSubtract( futureWaterVelocity, waterVelocity,
		waveAcceleration );
	VectorScale( waveAcceleration, 20.0f, waveAcceleration );
	load->waveLateralAcceleration = DotProduct( waveAcceleration, right );

	load->lateralLoad = fabsf( load->sailLateralAcceleration )
		+ fabsf( load->hullLateralAcceleration )
		+ 0.5f * fabsf( load->waveLateralAcceleration );
}

float BG_SailingImpactDamageForRate( float impactRate, float threshold ) {
	float severity;

	if ( threshold <= 0.0f ) {
		threshold = SAILING_SLAM_RATE_THRESHOLD;
	}
	if ( impactRate < threshold ) {
		return 0.0f;
	}
	severity = ( impactRate - threshold ) / threshold;
	return Com_Clamp( 0.0f, 225.0f, 5.0f + 60.0f * severity * severity );
}

float BG_SailingUpdateHullStress( float stress, float lateralLoad, float dt ) {
	float overload;

	if ( dt <= 0.0f ) {
		return Com_Clamp( 0.0f, 1.0f, stress );
	}
	overload = ( lateralLoad - SAILING_LATERAL_SAFE_LOAD )
		/ SAILING_LATERAL_SAFE_LOAD;
	if ( overload > 0.0f ) {
		stress += Com_Clamp( 0.0f, 2.0f, overload ) * 0.40f * dt;
	} else {
		stress -= 0.24f * dt;
	}
	return Com_Clamp( 0.0f, 1.0f, stress );
}

float BG_SailingHullStressDamageRate( float stress, float lateralLoad ) {
	float overload;
	float damageFraction;

	if ( stress <= SAILING_STRESS_DAMAGE_START
		|| lateralLoad <= SAILING_LATERAL_SAFE_LOAD ) {
		return 0.0f;
	}
	overload = Com_Clamp( 0.0f, 2.0f,
		( lateralLoad - SAILING_LATERAL_SAFE_LOAD )
			/ SAILING_LATERAL_SAFE_LOAD );
	damageFraction = ( stress - SAILING_STRESS_DAMAGE_START )
		/ ( 1.0f - SAILING_STRESS_DAMAGE_START );
	return 8.0f * damageFraction * overload;
}
