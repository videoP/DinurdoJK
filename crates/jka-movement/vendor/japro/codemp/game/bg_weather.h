/*
===========================================================================
Shared dynamic-weather definitions used by game and cgame.

misc_dynamic_weather is the map's weather authority. Its first addon is a
volumetric cloud layer; wind lives here too, so that the ocean, the particle
weather system and the clouds can eventually agree on one source rather than
each carrying their own.
===========================================================================
*/

#pragma once

#include "qcommon/q_shared.h"

/* One weather volume per map is the common case. The second slot exists so a
 * map can cross-fade between two authored states without a new entity form. */
#define MAX_DYNAMIC_WEATHER 2

/*
 * Cloud layer geometry is authored in map units as absolute world Z, not in
 * metres on a real-Earth sphere the way the NUBIS reference does. The horizon
 * falloff that curvature would otherwise provide comes from a separate, much
 * larger fake radius, so the layer heights stay something a mapper can reason
 * about against ±32768 world bounds.
 */
#define DYNAMIC_WEATHER_MIN_LAYER_HEIGHT 64.0f
#define DYNAMIC_WEATHER_MAX_LAYER_HEIGHT 262144.0f
/*
 * Fake planet radius. This is the knob that decides how far the cloud layer
 * reaches before curving below the horizon, and it is worth understanding
 * because it costs performance as directly as it changes the look: the horizon
 * distance is the longest ray the shader ever has to march.
 *
 *     horizonDistance = sqrt(cloudTop^2 + 2 * curvatureRadius * cloudTop)
 *
 * At an Earth-like 3,000,000 and a 14,000-unit layer top that is roughly
 * 290,000 units - nearly nine times the width of a full-size JKA map, and a
 * deck of cloud that reads as flat and endless because the curve is far too
 * gentle to see. 300,000 brings it to about 93,000, comfortably past the map
 * edge but close enough that the layer visibly bends down to meet the horizon,
 * and it shortens the worst-case march by a factor of three.
 */
#define DYNAMIC_WEATHER_DEFAULT_CURVATURE 300000.0f

typedef struct dynamicWeather_s {
	/*
	 * Dynamic state. Everything in this block rides the configstring and may be
	 * animated at runtime by target_weatherchange.
	 */
	vec2_t windDirection;
	/* Map units per second, matching misc_sailing_ocean's windSpeed. */
	float windSpeed;
	float windGust;
	float windShift;

	/* 0 clear sky, 1 fully overcast. The single biggest look knob. */
	float cloudCoverage;
	/* 0 flat stratus, 0.5 stratocumulus, 1 towering cumulus. Selects the
	 * height-density gradient the layer is shaped against. */
	float cloudType;
	/* Scales the accumulated extinction, so the same shape can read as thin
	 * haze or as a solid deck. */
	float cloudDensity;
	/* Fraction of windSpeed the cloud layer is advected at. Clouds aloft move
	 * with a different airmass than surface wind, and authors want that split. */
	float cloudSpeedScale;
	/* Absolute world Z of the cloud shell. These ride the configstring so a
	 * target_weatherchange can lower or raise the layer at runtime. */
	float cloudBottom;
	float cloudTop;

	/*
	 * Static configuration. Parsed from the BSP entity on both sides and never
	 * transmitted. Defined once here so client and server cannot drift.
	 */
	float curvatureRadius;
	/* World-unit tiling period of the base shape and detail noise. */
	float shapeScale;
	float detailScale;
	/* Erosion depth applied by the detail volume at cloud edges. */
	float detailStrength;
	/* Tint applied to lit cloud, multiplied into the sun colour. */
	vec3_t cloudColor;

	qboolean cloudsEnabled;
} dynamicWeather_t;

void BG_DynamicWeatherDefaults( dynamicWeather_t *weather );
int BG_DynamicWeatherAdd( const dynamicWeather_t *weather );
int BG_DynamicWeatherCount( void );
const dynamicWeather_t *BG_DynamicWeather( int index );
void BG_DynamicWeatherReset( void );

/* Applies and clamps only the dynamic block, leaving static configuration
 * untouched. This is the single entry point runtime changes go through. */
qboolean BG_DynamicWeatherSetState( int index, float windDirectionX,
	float windDirectionY, float windSpeed, float windGust, float windShift,
	float cloudCoverage, float cloudType, float cloudDensity,
	float cloudSpeedScale, float cloudBottom, float cloudTop );

void BG_DynamicWeatherBuildString( const dynamicWeather_t *weather,
	char *buffer, int bufferSize );
qboolean BG_DynamicWeatherApplyString( int index, const char *state );

/* Resolved wind for a point in time, gust and shift folded in. Shared so the
 * renderer's cloud advection and any future consumer see the same vector. */
void BG_DynamicWeatherWindVector( const dynamicWeather_t *weather, int time,
	vec2_t outWind );
