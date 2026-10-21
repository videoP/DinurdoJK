shader_type spatial;
render_mode depth_draw_always, world_vertex_coords;
uniform vec3 color: source_color;
uniform float sampling_distance = 0.01;
uniform sampler2D foam_map_near: repeat_disable;
uniform sampler2D foam_map_far: repeat_disable;
uniform sampler2D depth_texture: hint_depth_texture, repeat_disable, filter_nearest;
uniform sampler2D screen_texture: hint_screen_texture, repeat_disable;
uniform float absorption_strength = 0.5;
uniform vec3 color_absorption = vec3(1.3, 0.4, 0.6);
uniform sampler2D i_noise_texture;
uniform sampler2D noise: repeat_enable;
uniform vec2 foam_center;
uniform float foam_coverage;
uniform float foam_coverage_far;
uniform sampler2D sea_foam: repeat_enable;
varying float camera_dist;
varying vec2 foam_uv_near;
varying vec2 foam_uv_far;
varying vec2 world_xz;

uniform bool far_foam_enabled = true;
uniform bool near_foam_enabled = true;

vec3 gerstner(vec2 pos, vec2 direction, float amplitude, float frequency, float speed, float steepness) {
   	direction = normalize(direction);
	float noise_value = texture(noise, pos * 0.2 + TIME * direction * 0.07).r;
	float phase = (TIME * speed + dot(direction, pos) * frequency);
	vec3 displacement;
	displacement.y = sin(phase) * amplitude + noise_value * 0.05 - 0.05/2.0;
	displacement.xz = direction * cos(phase) * steepness;
	
	return displacement;
}

vec3 wave(vec2 pos) {
	vec3 displacement = gerstner(pos, vec2(0,1), 0.4, 1, 1.3, 0.2);
	displacement += gerstner(pos, vec2(1,1), 0.15, 1.3, 1.6, 0.12);
	displacement += gerstner(pos, vec2(0.5,1), 0.12, 2.0, 2.0, 0.17);
	displacement += gerstner(pos, vec2(-0.4,1), 0.01, 3.0, 3.0, 0.1);
	displacement += gerstner(pos, vec2(-0.2,0.7), 0.025, 4.0, 4.0, 0.08);
	
	return displacement;
}

vec2 create_mixed_foam(float foam_old, float foam_fresh, vec2 foam) {
	float mixed_foam_j = mix(foam_old, foam_fresh, foam.x);
	mixed_foam_j = smoothstep(0.4, 1, mixed_foam_j);
	foam.x = clamp(foam.x * mixed_foam_j, 0, 1);
	
	foam.y *= mix(foam_old, foam_fresh, foam.y);
	foam.y = smoothstep(0.4, 1, foam.y);
	
	return foam;
}

float get_through_water(vec2 uv, float depth, mat4 i_proj_mat, float vertex_z) {
	#if(CURRENT_RENDERER == RENDERER_COMPATIBILITY)
		vec3 ndc = vec3(uv, depth) * 2.0 - 1.0;
	#else
		vec3 ndc = vec3(uv  * 2.0 - 1.0, depth);
	#endif
	
	vec4 view_pos = i_proj_mat * vec4(ndc, 1);
	float through_water = -(view_pos/view_pos.w).z + vertex_z;
	
	return through_water;
}

void vertex() {
	foam_uv_near = (VERTEX.xz - foam_center) / foam_coverage + 0.5;
	foam_uv_far = (VERTEX.xz - foam_center) / foam_coverage_far + 0.5;
	world_xz = VERTEX.xz;
	
	vec3 displacement = wave(VERTEX.xz);
	vec3 wave_pos = VERTEX + displacement;
	camera_dist = distance(VERTEX.xz, CAMERA_POSITION_WORLD.xz);
	float sampling_fac = smoothstep(5, 40, camera_dist);
	float sample_dist =  mix(sampling_distance, sampling_distance * 20.0, sampling_fac);
	
	vec3 fwd_pos = VERTEX + vec3(0,0,-1)  * sample_dist;
	vec3 left_pos = VERTEX + vec3(-1,0,0) * sample_dist;
	
	vec3 wave_fwd = fwd_pos + wave(fwd_pos.xz);
	vec3 wave_left = left_pos + wave(left_pos.xz);
	
	vec3 fwd_vec = wave_fwd - wave_pos;
	vec3 left_vec = wave_left - wave_pos;
	
	VERTEX += displacement;
	vec3 wave_normal = normalize(cross(fwd_vec, left_vec));
	vec3 flat_normal = normalize(vec3(0, 1, 0));
	float flat_fac = smoothstep(40, 80.0, camera_dist);	
	
	NORMAL = mix(wave_normal, flat_normal, 0);
}


void fragment() {
	vec4 world_pos = INV_VIEW_MATRIX * vec4(VERTEX, 1);
	
	float h_blend = smoothstep(-0.7, 1, world_pos.y);
	vec3 mixed_color = mix(color*1.2, vec3(0,0.08,0.02), h_blend);
	vec4 foam_near = texture(foam_map_near, foam_uv_near);
	vec4 foam_far = texture(foam_map_far, foam_uv_far);
	vec2 world_uv = world_xz / 15.0;
	float foam_fresh = texture(sea_foam, world_uv * 2.0).r;
	float foam_old = texture(sea_foam, world_uv * 24.0).r;

	//Near foam
	vec2 foam_near_mixed = create_mixed_foam(foam_old, foam_fresh, vec2(foam_near.r, foam_near.g));
	
	// Far foam
	vec2 foam_far_mixed = create_mixed_foam(foam_old, foam_fresh, vec2(foam_far.r, foam_far.g));
	
	// Foam edge fading
	float near_radius = foam_coverage * 0.5;
	float far_radius = foam_coverage_far * 0.5;
	float fade_near = near_foam_enabled ? smoothstep(near_radius * 0.7, near_radius, camera_dist) : 1.0;
	float fade_far = far_foam_enabled ? smoothstep(far_radius * 0.9, far_radius, camera_dist) : 0.0;
	vec2 foam = mix(foam_near_mixed, foam_far_mixed, fade_near);
	foam = mix(foam, vec2(0), fade_far);
	
	// Add foam to albedo
	mixed_color = mix(mixed_color, vec3(0.6,1,0.9), foam.x);
	mixed_color = mix(mixed_color, vec3(0.6,1,0.9), foam.y);
	
	//Refraction
	vec3 normal = normalize(NORMAL);
	vec2 dist_strength = vec2(0.025, 0.01) * 1.25;
	vec2 planned_distortion = vec2(normal.x, -normal.y) * dist_strength;
	float distance_factor = clamp(4.0 / -VERTEX.z, 0, 1);

	vec2 d_uv = SCREEN_UV + planned_distortion * distance_factor;
	float d_depth = texture(depth_texture, d_uv).r;
	
	float through_water = get_through_water(d_uv, d_depth, INV_PROJECTION_MATRIX, VERTEX.z);
	float blend = smoothstep(0,0.5, through_water) * 0.8;
	
	d_uv = SCREEN_UV + planned_distortion * distance_factor * blend;
	d_depth = texture(depth_texture, d_uv).r;
	through_water = max(get_through_water(d_uv, d_depth, INV_PROJECTION_MATRIX, VERTEX.z), 0.0);
	
	if(d_depth > 0.0) {
		vec3 tex = texture(screen_texture, d_uv).rgb;
		
		//Depth fade
		tex *= exp(-through_water * absorption_strength * color_absorption);
		EMISSION = tex;
	}
	
	//Intersection
	float i_noise = texture(i_noise_texture, UV * 20.0 + TIME * 0.03).r;
	float i_width = 0.05 + sin(TIME * 0.5) * 0.02 + i_noise * 0.04;
	if(through_water > 0.0 && through_water < i_width) {
		float fade_in = smoothstep(0, 0.1 * i_width, through_water);
		float fade_out = 1.0 - smoothstep(0.8 * i_width, i_width, through_water);
		
		mixed_color = mix(mixed_color, vec3(0.4,0.7,0.6), fade_in * fade_out);
	}
	
	float horizon_fade =1.0 - exp(-camera_dist * 0.4);
	//mixed_color = mix(mixed_color, vec3(0.03, 0.03, 0.03), horizon_fade);
	//float mixed_rough = mix(0, 0.99, horizon_fade);
	
	ALBEDO = mixed_color;
	RIM = 2.0;
	ROUGHNESS = 0.01;
}
