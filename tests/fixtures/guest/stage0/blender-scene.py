import math
import sys

import bpy
from mathutils import Vector


arguments = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
output_path = arguments[0] if arguments else "//stage0-render.png"

bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.delete(use_global=False)

scene = bpy.context.scene
scene.render.engine = "CYCLES"
scene.cycles.device = "CPU"
scene.cycles.samples = 16
scene.render.resolution_x = 640
scene.render.resolution_y = 360
scene.render.resolution_percentage = 100
scene.render.image_settings.file_format = "PNG"
scene.render.filepath = output_path
scene.render.film_transparent = False
scene.world.color = (0.04, 0.05, 0.08)

bpy.ops.mesh.primitive_plane_add(size=8, location=(0, 0, -1))
floor = bpy.context.object
floor.name = "Stage0Floor"

bpy.ops.mesh.primitive_cube_add(size=1.5, location=(0, 0, 0))
cube = bpy.context.object
cube.name = "Stage0Cube"
cube.rotation_euler[2] = math.radians(24)

bpy.ops.mesh.primitive_uv_sphere_add(segments=32, ring_count=16, radius=0.7, location=(1.4, 0, -0.1))
sphere = bpy.context.object
sphere.name = "Stage0Sphere"

bpy.ops.object.camera_add(location=(4.8, -7.0, 4.0))
camera = bpy.context.object
camera.name = "Stage0Camera"
camera.rotation_euler = (Vector((0, 0, 0)) - camera.location).to_track_quat("-Z", "Y").to_euler()
scene.camera = camera
camera.data.lens = 50

bpy.ops.object.light_add(type="AREA", location=(1.0, -3.5, 5.0))
light = bpy.context.object
light.name = "Stage0KeyLight"
light.data.energy = 500
light.data.shape = "DISK"
light.data.size = 4

scene.render.image_settings.color_mode = "RGBA"
scene.view_settings.view_transform = "Standard"
bpy.ops.render.render(write_still=True)
