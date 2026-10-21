@tool
extends Node3D

@onready var ocean: MeshInstance3D = $Ocean

@onready var foam_sub_near: FoamSubviewport = $FoamSubViewport
@onready var foam_sub_far: FoamSubviewport = $FoamSubViewportFar

@onready var top_depth_camera: Camera3D = $TopDepth/Camera3D
@onready var bottom_depth_camera: Camera3D = $BottomDepth/Camera3D

@onready var ocean_material: ShaderMaterial = $Ocean.get_surface_override_material(0)

@export var near_foam: bool = true:
	set(value):
		near_foam = value
		set_foam_enabled(value, foam_sub_near)

@export var far_foam: bool = true:
	set(value):
		far_foam = value
		set_foam_enabled(value, foam_sub_far)

func _ready() -> void:
	set_foam_enabled(near_foam, foam_sub_near)
	set_foam_enabled(far_foam, foam_sub_far)

func _process(delta) -> void:
	var capture_center := calc_capture_center()
	var depth_coverage := foam_sub_far.capture_coverage
	update_depth_cameras(capture_center)

	foam_sub_near.set_depth_coverage(depth_coverage)
	foam_sub_far.set_depth_coverage(depth_coverage)

	if(near_foam || far_foam):
		ocean_material.set_shader_parameter("foam_center", capture_center)

	if(near_foam):
		foam_sub_near.update(delta, capture_center)
		if foam_sub_near.foam_texture != null:
			ocean_material.set_shader_parameter("foam_map_near", foam_sub_near.foam_texture)
			ocean_material.set_shader_parameter("foam_coverage", foam_sub_near.capture_coverage)

	if(far_foam):
		foam_sub_far.update(delta, capture_center)
		if foam_sub_far.foam_texture != null:
			ocean_material.set_shader_parameter("foam_map_far", foam_sub_far.foam_texture)
			ocean_material.set_shader_parameter("foam_coverage_far", foam_sub_far.capture_coverage)
	

func set_foam_enabled(value: bool, subviewport: FoamSubviewport) -> void:
	if is_node_ready():
		ocean_material.set_shader_parameter("far_foam_enabled", far_foam)
		ocean_material.set_shader_parameter("near_foam_enabled", near_foam)
		var update_foam_maps: Callable = func():
			ocean_material.set_shader_parameter("foam_map_near", foam_sub_near.foam_texture)
			ocean_material.set_shader_parameter("foam_map_far", foam_sub_far.foam_texture)
			
		subviewport.set_enabled(value, update_foam_maps)

func update_depth_cameras(center: Vector2) -> void:
	top_depth_camera.size = foam_sub_far.capture_coverage
	top_depth_camera.global_position.x = center.x
	top_depth_camera.global_position.z = center.y

	bottom_depth_camera.size = foam_sub_far.capture_coverage
	bottom_depth_camera.global_position.x = center.x
	bottom_depth_camera.global_position.z = center.y

	var top_mesh: QuadMesh = top_depth_camera.get_node("MeshInstance3D").mesh
	var bottom_mesh: QuadMesh = bottom_depth_camera.get_node("MeshInstance3D").mesh
	top_mesh.size = Vector2.ONE * foam_sub_far.capture_coverage
	bottom_mesh.size = top_mesh.size

func get_camera() -> Camera3D:
	var camera: Camera3D
	if Engine.is_editor_hint():
		camera = EditorInterface.get_editor_viewport_3d(0).get_camera_3d()
	else:
		camera = get_viewport().get_camera_3d()

	return camera

func calc_capture_center() -> Vector2:
	var camera := get_camera()
	if !is_instance_valid(camera):
		return Vector2.ZERO

	var camera_xz := Vector2(camera.global_position.x, camera.global_position.z)
	var capture_center = camera_xz.snapped(Vector2.ONE)

	return capture_center