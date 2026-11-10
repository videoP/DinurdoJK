/* jaPRO sailing state for the jaPRO (TaystJK) prediction backend.
 *
 * Mirrors the jaPRO `sailing` cgame (D:/Code/jaPRO codemp/cgame/cg_spawn.c):
 * the static map data a swimmer's Pmove needs comes from the client's own BSP
 * entity lump (misc_sailing_ocean/shore/current), and the per-ocean runtime
 * state from CS_SAILING_OCEANS. The SP_* bodies below are cg_spawn.c's, with
 * trap->R_ModelBounds replaced by bounds the caller resolved from the BSP and
 * the renderer callbacks dropped. Callers hold the native lock (NativeGuard). */
#include "qcommon/q_shared.h"
#include "game/bg_sailing.h"
#include "game/bg_weather.h"

#define JKA_SAILING_MAX_SPAWN_VARS 64

static int sailSpawnCount;
static const char *sailSpawnKeys[JKA_SAILING_MAX_SPAWN_VARS];
static const char *sailSpawnValues[JKA_SAILING_MAX_SPAWN_VARS];
static qboolean sailHasModelBounds;
static vec3_t sailModelMins;
static vec3_t sailModelMaxs;

/* CG_SpawnString and friends: case-insensitive key, default when absent. */
static qboolean Sail_SpawnString( const char *key, const char *defaultString,
	const char **out ) {
	int i;

	for ( i = 0; i < sailSpawnCount; i++ ) {
		if ( !Q_stricmp( key, sailSpawnKeys[i] ) ) {
			*out = sailSpawnValues[i];
			return qtrue;
		}
	}
	*out = defaultString;
	return qfalse;
}

static qboolean Sail_SpawnFloat( const char *key, const char *defaultString,
	float *out ) {
	const char *s;
	qboolean present = Sail_SpawnString( key, defaultString, &s );

	*out = atof( s );
	return present;
}

static qboolean Sail_SpawnInt( const char *key, const char *defaultString,
	int *out ) {
	const char *s;
	qboolean present = Sail_SpawnString( key, defaultString, &s );

	*out = atoi( s );
	return present;
}

static qboolean Sail_SpawnVector( const char *key, const char *defaultString,
	float *out ) {
	const char *s;
	qboolean present = Sail_SpawnString( key, defaultString, &s );

	if ( sscanf( s, "%f %f %f", &out[0], &out[1], &out[2] ) != 3 ) {
		sscanf( defaultString, "%f %f %f", &out[0], &out[1], &out[2] );
	}
	return present;
}

static int Sail_misc_sailing_ocean( void ) {
	sailingOcean_t ocean;
	const char *shader;
	vec3_t origin;
	float waveAngle;
	int waveSeed;
	int gerstnerWaveCount;

	if ( BG_SailingOceanCount() >= MAX_SAILING_OCEANS || !sailHasModelBounds ) {
		return -1;
	}

	BG_SailingOceanDefaults( &ocean );
	Sail_SpawnVector( "origin", "0 0 0", origin );
	VectorAdd( sailModelMins, origin, ocean.mins );
	VectorAdd( sailModelMaxs, origin, ocean.maxs );
	ocean.baseHeight = ocean.maxs[2];

	Sail_SpawnFloat( "waterHeight", va( "%f", ocean.baseHeight ), &ocean.baseHeight );
	Sail_SpawnFloat( "amplitude", "128", &ocean.amplitude );
	Sail_SpawnFloat( "wavelength", "1400", &ocean.wavelength );
	Sail_SpawnFloat( "speed", "180", &ocean.speedAuthored );
	Sail_SpawnFloat( "steepness", "0.65", &ocean.steepness );
	Sail_SpawnFloat( "slosh", "0", &ocean.slosh );
	Sail_SpawnFloat( "currentScale", "0.25", &ocean.currentScale );
	Sail_SpawnFloat( "textureDriftScale", "0.25", &ocean.textureDriftScale );
	Sail_SpawnFloat( "swimControlScale", "1", &ocean.swimControlScale );
	Sail_SpawnFloat( "buoyancy", "12", &ocean.buoyancy );
	Sail_SpawnFloat( "buoyancyDamping", "7", &ocean.buoyancyDamping );
	Sail_SpawnFloat( "buoyancyDepth", "48", &ocean.buoyancyDepth );
	Sail_SpawnFloat( "diveDrag", "0.2", &ocean.diveDrag );
	Sail_SpawnFloat( "gridSpacing", "96", &ocean.gridSpacing );
	Sail_SpawnFloat( "renderDistance", "4096", &ocean.renderDistance );
	Sail_SpawnFloat( "textureScale", "256", &ocean.textureScale );
	Sail_SpawnVector( "fogColor", "0.45 0.6 0.75", ocean.fogColor );
	Sail_SpawnFloat( "fogDistance", "300", &ocean.fogDistance );
	Sail_SpawnInt( "waveSeed", "85619173", &waveSeed );
	ocean.waveSeed = (unsigned int)waveSeed;
	Sail_SpawnInt( "gerstnerWaveCount", "8", &gerstnerWaveCount );
	ocean.gerstnerWaveCount = gerstnerWaveCount;
	/* The server publishes the resolved rollout gate. */
	ocean.horizontalDisplacementScale = 0.0f;
	Sail_SpawnFloat( "waveAngle", "0", &waveAngle );
	waveAngle *= (float)M_PI / 180.0f;
	ocean.direction[0] = cosf( waveAngle );
	ocean.direction[1] = sinf( waveAngle );
	Sail_SpawnFloat( "rainIntensity", "0", &ocean.rainIntensity );
	Sail_SpawnString( "baseShader", "", &shader );
	Q_strncpyz( ocean.baseShader, shader, sizeof( ocean.baseShader ) );
	Sail_SpawnString( "meshShader", "textures/sailing/ocean_mesh", &shader );
	Q_strncpyz( ocean.meshShader, shader, sizeof( ocean.meshShader ) );

	return BG_SailingAddOcean( &ocean );
}

static int Sail_misc_sailing_shore( void ) {
	sailingShore_t shore;
	const char *shoreType;
	vec3_t origin;
	float angle;

	if ( BG_SailingShoreCount() >= MAX_SAILING_SHORES || !sailHasModelBounds ) {
		return -1;
	}

	BG_SailingShoreDefaults( &shore );
	Sail_SpawnVector( "origin", "0 0 0", origin );
	VectorAdd( sailModelMins, origin, shore.mins );
	VectorAdd( sailModelMaxs, origin, shore.maxs );
	Sail_SpawnInt( "ocean", "0", &shore.oceanIndex );
	Sail_SpawnFloat( "angle", "0", &angle );
	angle *= (float)M_PI / 180.0f;
	shore.direction[0] = cosf( angle );
	shore.direction[1] = sinf( angle );
	Sail_SpawnString( "shoreType", "beach", &shoreType );
	if ( !Q_stricmp( shoreType, "cliff" ) ) {
		shore.type = SAILING_SHORE_CLIFF;
	}
	return BG_SailingAddShore( &shore );
}

static int Sail_misc_sailing_current( void ) {
	sailingCurrent_t current;
	const char *type;
	vec3_t origin;
	float width;
	float height;

	if ( BG_SailingCurrentCount() >= MAX_SAILING_CURRENTS || !sailHasModelBounds ) {
		return -1;
	}

	BG_SailingCurrentDefaults( &current );
	Sail_SpawnVector( "origin", "0 0 0", origin );
	current.origin[0] = origin[0] + ( sailModelMins[0] + sailModelMaxs[0] ) * 0.5f;
	current.origin[1] = origin[1] + ( sailModelMins[1] + sailModelMaxs[1] ) * 0.5f;
	current.origin[2] = origin[2] + ( sailModelMins[2] + sailModelMaxs[2] ) * 0.5f;
	width = sailModelMaxs[0] - sailModelMins[0];
	height = sailModelMaxs[1] - sailModelMins[1];
	current.radius = fminf( width, height ) * 0.5f;
	if ( current.radius < 16.0f ) {
		return -1;
	}
	Sail_SpawnString( "type", "vortex", &type );
	if ( Q_stricmp( type, "vortex" ) ) {
		return -1;
	}
	current.type = SAILING_CURRENT_VORTEX;
	Sail_SpawnInt( "ocean", "0", &current.oceanIndex );
	current.coreRadius = current.radius * SAILING_CURRENT_CORE_FRACTION;
	Sail_SpawnFloat( "coreRadius", va( "%f", current.coreRadius ),
		&current.coreRadius );
	current.eyeRadius = current.coreRadius * 0.15f;
	Sail_SpawnFloat( "eyeRadius", va( "%f", current.eyeRadius ),
		&current.eyeRadius );
	Sail_SpawnFloat( "spinSpeed", "480", &current.spinSpeed );
	Sail_SpawnFloat( "pullSpeed", "120", &current.pullSpeed );
	Sail_SpawnFloat( "sinkDepth", "-1", &current.sinkDepth );
	Sail_SpawnFloat( "waveScale", "0.25", &current.waveScale );
	Sail_SpawnFloat( "direction", "1", &current.direction );

	return BG_SailingAddCurrent( &current );
}

/* cg_spawn.c SP_misc_dynamic_weather: the map's wind (and clouds), whose
 * dynamic block CS_DYNAMIC_WEATHER then overwrites. The sea's spectrum reads
 * its wind speed. */
static int Sail_misc_dynamic_weather( void ) {
	dynamicWeather_t weather;
	vec3_t origin;
	float windAngle;
	float cloudHeight;
	float curvatureRatio;
	int clouds;

	if ( BG_DynamicWeatherCount() >= MAX_DYNAMIC_WEATHER ) {
		return -1;
	}

	BG_DynamicWeatherDefaults( &weather );

	Sail_SpawnFloat( "windAngle", "0", &windAngle );
	windAngle *= (float)M_PI / 180.0f;
	weather.windDirection[0] = cosf( windAngle );
	weather.windDirection[1] = sinf( windAngle );
	Sail_SpawnFloat( "windSpeed", va( "%f", weather.windSpeed ), &weather.windSpeed );
	Sail_SpawnFloat( "windGust", va( "%f", weather.windGust ), &weather.windGust );
	Sail_SpawnFloat( "windShift", va( "%f", weather.windShift ), &weather.windShift );
	Sail_SpawnFloat( "cloudCoverage", va( "%f", weather.cloudCoverage ),
		&weather.cloudCoverage );
	Sail_SpawnFloat( "cloudType", va( "%f", weather.cloudType ), &weather.cloudType );
	Sail_SpawnFloat( "cloudDensity", va( "%f", weather.cloudDensity ),
		&weather.cloudDensity );
	Sail_SpawnFloat( "cloudSpeedScale", va( "%f", weather.cloudSpeedScale ),
		&weather.cloudSpeedScale );

	if ( !Sail_SpawnFloat( "height", "0", &cloudHeight ) || cloudHeight <= 0.0f ) {
		return -1;
	}
	Sail_SpawnVector( "origin", "0 0 0", origin );
	weather.cloudBottom = origin[2];
	weather.cloudTop = weather.cloudBottom + cloudHeight;
	if ( !Sail_SpawnFloat( "curvatureRatio", "0", &curvatureRatio )
		|| curvatureRatio <= 0.0f ) {
		return -1;
	}
	weather.curvatureRadius = cloudHeight * curvatureRatio;
	Sail_SpawnFloat( "shapeScale", va( "%f", weather.shapeScale ),
		&weather.shapeScale );
	Sail_SpawnFloat( "detailScale", va( "%f", weather.detailScale ),
		&weather.detailScale );
	Sail_SpawnFloat( "detailStrength", va( "%f", weather.detailStrength ),
		&weather.detailStrength );
	Sail_SpawnVector( "cloudColor", "1 1 1", weather.cloudColor );
	Sail_SpawnInt( "clouds", "1", &clouds );
	weather.cloudsEnabled = clouds ? qtrue : qfalse;

	return BG_DynamicWeatherAdd( &weather );
}

/* CG_SailingReset's BG half: drop every ocean/shore/current/river/waterspout. */
void jka_sailing_reset( void ) {
	BG_SailingClearOceans();
	BG_DynamicWeatherReset();
}

/* One BSP entity, in entity-lump order (indices must match the server's spawn
 * order). modelMins/modelMaxs are the entity's inline brush model bounds, or
 * NULL. Returns the BG index, -1 when ignored or rejected, -2 for an entity
 * that is not a sailing marker. */
int jka_sailing_spawn( const char *const *keys, const char *const *values,
	int count, const float *modelMins, const float *modelMaxs ) {
	const char *classname;
	int result = -2;

	if ( count < 0 || count > JKA_SAILING_MAX_SPAWN_VARS ) {
		return -1;
	}
	sailSpawnCount = count;
	memcpy( (void *)sailSpawnKeys, keys, sizeof( keys[0] ) * count );
	memcpy( (void *)sailSpawnValues, values, sizeof( values[0] ) * count );
	sailHasModelBounds = modelMins && modelMaxs;
	if ( sailHasModelBounds ) {
		VectorCopy( modelMins, sailModelMins );
		VectorCopy( modelMaxs, sailModelMaxs );
	}
	Sail_SpawnString( "classname", "", &classname );
	if ( !Q_stricmp( classname, "misc_sailing_ocean" ) ) {
		result = Sail_misc_sailing_ocean();
	}
	else if ( !Q_stricmp( classname, "misc_sailing_shore" ) ) {
		result = Sail_misc_sailing_shore();
	}
	else if ( !Q_stricmp( classname, "misc_sailing_current" ) ) {
		result = Sail_misc_sailing_current();
	}
	else if ( !Q_stricmp( classname, "misc_dynamic_weather" ) ) {
		result = Sail_misc_dynamic_weather();
	}
	sailSpawnCount = 0;
	return result;
}

/* CS_SAILING_OCEANS + index. Nonzero when applied. */
int jka_sailing_apply_ocean_state( int index, const char *state ) {
	return BG_SailingApplyOceanWeatherString( index, state ) ? 1 : 0;
}

/* CS_DYNAMIC_WEATHER + index. Nonzero when applied. */
int jka_sailing_apply_weather_state( int index, const char *state ) {
	return BG_DynamicWeatherApplyString( index, state ) ? 1 : 0;
}

/* Ocean `index`'s sea exactly as physics floats in it (BG_SailingSea), for the
 * client's Hybrid ocean to draw. `params` (13 floats): significant height (m),
 * direction (deg), peak wavelength (m), choppiness, wind speed (m/s), the three
 * tile sizes (m), FFT blend, swell amplitude (u), wavelength (u), speed (u/s)
 * and steepness. `waves` (6 floats each): cascade, nx, nz, re, im, m. Returns
 * the wave count, or -1 without that ocean. */
int jka_sailing_sea( int index, float *params, float *waves, int maxWaves ) {
	const sailingSea_t *sea = BG_SailingSea( BG_SailingOcean( index ) );
	int count;
	int i;

	if ( !sea || !sea->valid || !params ) {
		return -1;
	}
	params[0] = sea->significantHeight;
	params[1] = sea->directionDegrees;
	params[2] = sea->peakWavelength;
	params[3] = sea->choppiness;
	params[4] = sea->windSpeed;
	params[5] = sea->tileSize[0];
	params[6] = sea->tileSize[1];
	params[7] = sea->tileSize[2];
	params[8] = sea->fftBlend;
	params[9] = sea->swellAmplitude;
	params[10] = sea->swellWavelength;
	params[11] = sea->swellSpeed;
	params[12] = sea->swellSteepness;
	count = sea->waveCount < maxWaves ? sea->waveCount : maxWaves;
	for ( i = 0; waves && i < count; i++ ) {
		const sailingSeaWave_t *wave = &sea->wave[i];
		waves[i * 6 + 0] = (float)wave->cascade;
		waves[i * 6 + 1] = (float)wave->nx;
		waves[i * 6 + 2] = (float)wave->nz;
		waves[i * 6 + 3] = wave->re;
		waves[i * 6 + 4] = wave->im;
		waves[i * 6 + 5] = wave->m;
	}
	return count;
}

/* The shared analytic surface Pmove swims in, for diagnostics. Nonzero when
 * (x, y) is inside an ocean volume. */
int jka_sailing_wave_height( float x, float y, int time, float *height ) {
	const sailingOcean_t *ocean = BG_SailingOceanForPoint( x, y );

	if ( !ocean ) {
		return 0;
	}
	BG_SailingEvaluateWave( ocean, x, y, time, height, NULL, NULL );
	return 1;
}
