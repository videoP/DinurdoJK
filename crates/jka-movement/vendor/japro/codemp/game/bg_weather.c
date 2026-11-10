/*
===========================================================================
Shared dynamic-weather state for misc_dynamic_weather.

Game and cgame both parse the same BSP keys and both call in here, so every
default and every clamp is defined once, in this file. The sailing ocean was
bitten twice by client and server independently guessing at defaults; keeping
them here is the fix rather than the convention.
===========================================================================
*/

#include "bg_weather.h"

static dynamicWeather_t bgDynamicWeather[MAX_DYNAMIC_WEATHER];
static int bgDynamicWeatherCount;

void BG_DynamicWeatherDefaults( dynamicWeather_t *weather ) {
	if ( !weather ) {
		return;
	}
	memset( weather, 0, sizeof( *weather ) );

	weather->windDirection[0] = 1.0f;
	weather->windDirection[1] = 0.0f;
	weather->windSpeed = 180.0f;
	weather->windGust = 0.2f;
	weather->windShift = 0.0f;

	weather->cloudCoverage = 0.45f;
	weather->cloudType = 0.5f;
	weather->cloudDensity = 1.0f;
	/* Clouds ride a faster airmass than the surface. Greater than 1 so a map
	 * tuned for believable sea state still gets visible sky movement. */
	weather->cloudSpeedScale = 2.0f;

	weather->cloudBottom = 6000.0f;
	weather->cloudTop = 14000.0f;
	weather->curvatureRadius = DYNAMIC_WEATHER_DEFAULT_CURVATURE;
	weather->shapeScale = 32768.0f;
	weather->detailScale = 4096.0f;
	weather->detailStrength = 0.35f;
	weather->cloudColor[0] = 1.0f;
	weather->cloudColor[1] = 1.0f;
	weather->cloudColor[2] = 1.0f;

	weather->cloudsEnabled = qtrue;
}

static void BG_DynamicWeatherClamp( dynamicWeather_t *weather ) {
	float length;

	weather->windSpeed = Com_Clamp( 0.0f, 8192.0f, weather->windSpeed );
	weather->windGust = Com_Clamp( 0.0f, 1.0f, weather->windGust );
	weather->windShift = Com_Clamp( 0.0f, 180.0f, weather->windShift );

	length = sqrtf( weather->windDirection[0] * weather->windDirection[0]
		+ weather->windDirection[1] * weather->windDirection[1] );
	if ( length > 0.0001f ) {
		weather->windDirection[0] /= length;
		weather->windDirection[1] /= length;
	}
	else {
		weather->windDirection[0] = 1.0f;
		weather->windDirection[1] = 0.0f;
	}

	weather->cloudCoverage = Com_Clamp( 0.0f, 1.0f, weather->cloudCoverage );
	weather->cloudType = Com_Clamp( 0.0f, 1.0f, weather->cloudType );
	weather->cloudDensity = Com_Clamp( 0.0f, 8.0f, weather->cloudDensity );
	weather->cloudSpeedScale =
		Com_Clamp( 0.0f, 32.0f, weather->cloudSpeedScale );

	weather->cloudBottom = Com_Clamp( -DYNAMIC_WEATHER_MAX_LAYER_HEIGHT,
		DYNAMIC_WEATHER_MAX_LAYER_HEIGHT, weather->cloudBottom );
	weather->cloudTop = Com_Clamp( -DYNAMIC_WEATHER_MAX_LAYER_HEIGHT,
		DYNAMIC_WEATHER_MAX_LAYER_HEIGHT, weather->cloudTop );
	/* An inverted or degenerate layer would make the raymarch's height
	 * fraction divide by zero, so enforce a floor rather than trusting the
	 * entity. */
	if ( weather->cloudTop
		< weather->cloudBottom + DYNAMIC_WEATHER_MIN_LAYER_HEIGHT ) {
		weather->cloudTop =
			weather->cloudBottom + DYNAMIC_WEATHER_MIN_LAYER_HEIGHT;
	}

	/* The floor is low enough to author a small, obviously curved world for
	 * effect; the ceiling keeps the squared radius well inside float range. */
	weather->curvatureRadius = Com_Clamp( 16384.0f, 1.0e9f,
		weather->curvatureRadius );
	weather->shapeScale = Com_Clamp( 256.0f, 1048576.0f,
		weather->shapeScale );
	weather->detailScale = Com_Clamp( 64.0f, 262144.0f,
		weather->detailScale );
	weather->detailStrength = Com_Clamp( 0.0f, 1.0f,
		weather->detailStrength );
	weather->cloudColor[0] = Com_Clamp( 0.0f, 4.0f, weather->cloudColor[0] );
	weather->cloudColor[1] = Com_Clamp( 0.0f, 4.0f, weather->cloudColor[1] );
	weather->cloudColor[2] = Com_Clamp( 0.0f, 4.0f, weather->cloudColor[2] );
}

void BG_DynamicWeatherReset( void ) {
	memset( bgDynamicWeather, 0, sizeof( bgDynamicWeather ) );
	bgDynamicWeatherCount = 0;
}

int BG_DynamicWeatherCount( void ) {
	return bgDynamicWeatherCount;
}

const dynamicWeather_t *BG_DynamicWeather( int index ) {
	if ( index < 0 || index >= bgDynamicWeatherCount ) {
		return NULL;
	}
	return &bgDynamicWeather[index];
}

int BG_DynamicWeatherAdd( const dynamicWeather_t *weather ) {
	int index;

	if ( !weather || bgDynamicWeatherCount >= MAX_DYNAMIC_WEATHER ) {
		return -1;
	}
	index = bgDynamicWeatherCount++;
	bgDynamicWeather[index] = *weather;
	BG_DynamicWeatherClamp( &bgDynamicWeather[index] );
	return index;
}

qboolean BG_DynamicWeatherSetState( int index, float windDirectionX,
	float windDirectionY, float windSpeed, float windGust, float windShift,
	float cloudCoverage, float cloudType, float cloudDensity,
	float cloudSpeedScale, float cloudBottom, float cloudTop ) {
	dynamicWeather_t *weather;

	if ( index < 0 || index >= bgDynamicWeatherCount ) {
		return qfalse;
	}
	weather = &bgDynamicWeather[index];
	weather->windDirection[0] = windDirectionX;
	weather->windDirection[1] = windDirectionY;
	weather->windSpeed = windSpeed;
	weather->windGust = windGust;
	weather->windShift = windShift;
	weather->cloudCoverage = cloudCoverage;
	weather->cloudType = cloudType;
	weather->cloudDensity = cloudDensity;
	weather->cloudSpeedScale = cloudSpeedScale;
	weather->cloudBottom = cloudBottom;
	weather->cloudTop = cloudTop;
	BG_DynamicWeatherClamp( weather );
	return qtrue;
}

void BG_DynamicWeatherWindVector( const dynamicWeather_t *weather, int time,
	vec2_t outWind ) {
	float seconds;
	float gust;
	float shift;
	float shiftRadians;
	float shiftCos;
	float shiftSin;
	float speed;

	if ( !outWind ) {
		return;
	}
	if ( !weather ) {
		outWind[0] = 0.0f;
		outWind[1] = 0.0f;
		return;
	}

	/* Deliberately the same continuous, time-based formulation the sailing
	 * ocean uses, so that once the ocean reads its wind from here the sea state
	 * does not visibly change. */
	seconds = (float)time * 0.001f;
	gust = sinf( seconds * 0.83f ) * 0.65f
		+ sinf( seconds * 1.71f + 1.9f ) * 0.35f;
	speed = weather->windSpeed * ( 1.0f + weather->windGust * gust );
	if ( speed < 0.0f ) {
		speed = 0.0f;
	}

	shift = sinf( seconds * 0.13f ) * 0.7f
		+ sinf( seconds * 0.047f + 2.4f ) * 0.3f;
	shiftRadians = weather->windShift * shift * (float)M_PI / 180.0f;
	shiftCos = cosf( shiftRadians );
	shiftSin = sinf( shiftRadians );
	outWind[0] = ( weather->windDirection[0] * shiftCos
		- weather->windDirection[1] * shiftSin ) * speed;
	outWind[1] = ( weather->windDirection[0] * shiftSin
		+ weather->windDirection[1] * shiftCos ) * speed;
}

void BG_DynamicWeatherBuildString( const dynamicWeather_t *weather,
	char *buffer, int bufferSize ) {
	if ( !buffer || bufferSize <= 0 ) {
		return;
	}
	if ( !weather ) {
		buffer[0] = '\0';
		return;
	}
	/* Positional, no keys, matching the ocean string's shape. */
	Com_sprintf( buffer, bufferSize,
		"%.6f %.6f %.3f %.3f %.3f %.4f %.4f %.4f %.4f %.3f %.3f",
		weather->windDirection[0], weather->windDirection[1],
		weather->windSpeed, weather->windGust, weather->windShift,
		weather->cloudCoverage, weather->cloudType, weather->cloudDensity,
		weather->cloudSpeedScale, weather->cloudBottom, weather->cloudTop );
}

qboolean BG_DynamicWeatherApplyString( int index, const char *state ) {
	float windDirectionX, windDirectionY, windSpeed, windGust, windShift;
	float cloudCoverage, cloudType, cloudDensity, cloudSpeedScale;
	float cloudBottom, cloudTop;
	int parsed;

	if ( !state || !state[0] || !BG_DynamicWeather( index ) ) {
		return qfalse;
	}

	parsed = sscanf( state, "%f %f %f %f %f %f %f %f %f %f %f",
		&windDirectionX, &windDirectionY, &windSpeed, &windGust, &windShift,
		&cloudCoverage, &cloudType, &cloudDensity, &cloudSpeedScale,
		&cloudBottom, &cloudTop );
	if ( parsed != 11 ) {
		return qfalse;
	}
	return BG_DynamicWeatherSetState( index, windDirectionX, windDirectionY,
		windSpeed, windGust, windShift, cloudCoverage, cloudType,
		cloudDensity, cloudSpeedScale, cloudBottom, cloudTop );
}
