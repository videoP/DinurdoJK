"""Package exported Unreal skeletal meshes as a DinurdoJK player model.

    python scripts/ue-skin-package.py <export dir> <model name> <out.pk3> [--profile ue|fortnite]
        [--bone-map map.json] [--female] [--icon icon.png] <MeshName>[=Material0,Material1,...] ...

<export dir> is a UE export directory (crates/jka-ue-assets/EXPORT-FORMAT.md)
holding the skeletal meshes, their materials and textures. Each part names a SkeletalMesh asset; the optional
material list overrides its sections in order (character part material
overrides), `-` keeps a section's own material and `off` hides it; profile-listed (`--profile fortnite`: `*NoHair`)
materials are hidden automatically. The first
part supplies the shared skeleton (the body). The game loads the result
through `models/players/<name>/source_model.json` (`"format": "ue_skeletal"`).
"""
import argparse
import io
import json
import os
import re
import sys
import zipfile

import numpy
from PIL import Image, ImageChops

parser = argparse.ArgumentParser()
parser.add_argument('export')
parser.add_argument('name')
parser.add_argument('out')
parser.add_argument('parts', nargs='+')
parser.add_argument('--female', action='store_true')
parser.add_argument('--profile', choices=['ue', 'fortnite'], default='ue',
                    help='material parameter names of the project (default: generic UE names)')
parser.add_argument('--bone-map', help='JSON {source bone: ValveBiped/_humanoid bone} for skeletons that do not use the UE mannequin names')
parser.add_argument('--icon')
parser.add_argument('--title', help='display name (the item title), stored in source_model.json')
parser.add_argument('--idle', help='bake-idle output directory (idle.json, tracks, fx-native/)')
parser.add_argument('--native', help='native import PK3 holding the idle effects\' particle models')
parser.add_argument('--attach', action='append', default=[],
                    help='MeshName=bone,x,y,z,pitch,yaw,roll: mount a rigid accessory on a socket')
args = parser.parse_args()

if not re.fullmatch(r'[a-z0-9_]+', args.name):
    sys.exit('model name must be lower-case letters, digits and _')
report = json.load(open(os.path.join(args.export, 'report.json')))['assets']
materials = json.load(open(os.path.join(args.export, 'materials.json')))
# Assets are named by full object path (`/Game/.../Name.Name`) or, when
# unambiguous, by short name; different folders reuse short names (a body and
# a head can both be `M_MED_CarrotCake`).
def by_name(pairs):
    out = {}
    for path, value in pairs:
        out[path.lower()] = value
        out.setdefault(path.split('.')[-1].lower(), value)
    return out


mesh_ids = by_name((a['asset'], a['id']) for a in report if a.get('type') == 'SkeletalMesh')
texture_ids = by_name((a['asset'], a['id']) for a in report if a.get('type') == 'Texture2D')
material_ids = by_name((m['asset'], mid) for mid, m in materials.items())
# `material-ports.py` output, when run on this export: the base colour it
# chose (rejecting normal maps and RGB region masks), tint and solid colours.
ported_path = os.path.join(args.export, 'native-materials-v2.json')
ported = json.load(open(ported_path, encoding='utf-8')) if os.path.isfile(ported_path) else {}
root = 'models/players/%s/' % args.name
shader_script = []


def png(image):
    out = io.BytesIO()
    image.save(out, 'PNG')
    return out.getvalue()


def texture(material, name):
    """A material's texture parameter as an RGBA image, unless it is a default."""
    path = (material.get('parameters') or {}).get('textures', {}).get(name, '')
    if not path or '/Global/Textures/Default/' in path:
        return None
    tid = texture_ids.get(path.lower()) or texture_ids.get(path.split('.')[-1].lower())
    file = tid and os.path.join(args.export, 'Textures', tid + '.png')
    return Image.open(file).convert('RGBA') if file and os.path.isfile(file) else None


# Material parameter names, per project, best first. The `ue` profile uses the
# names of Epic's own templates; add a profile (or extend one) for your game's
# master materials. `emissive` names the parameters of an emissive pass and
# `galaxy` an optional screen-space nebula effect; `hide_materials` lists
# lower-case fragments of material names to hide outright.
PROFILES = {
    'ue': {
        'base_color': ('BaseColor', 'Base Color', 'Diffuse', 'Albedo'),
        'normal': ('Normal', 'NormalMap', 'Normals'),
        'specular': ('SRM', 'Specular', 'SpecularMasks'),
        'emissive': {'switch': None, 'texture': 'Emissive', 'color': 'Emissive Color', 'mult': None},
        'galaxy': None,
        'hide_materials': (),
    },
    'fortnite': {
        # `LitDiffuse` is the anime/toon masters' lit colour.
        'base_color': ('LitDiffuse', 'Diffuse', 'PM_Diffuse', 'BaseColor', 'Base Color'),
        'normal': ('Normals', 'PM_Normals', 'Normal', 'NormalMap'),
        'specular': ('SpecularMasks', 'PM_SpecularMasks', 'SRM', 'Specular'),
        'emissive': {'switch': 'Emissive', 'texture': 'Emissive', 'color': 'Emissive Color', 'mult': 'emissive mult'},
        # M_FN_Celestial_Master `Use Galaxy`.
        'galaxy': {'switch': 'Use Galaxy', 'texture': 'GalaxyTexture', 'mask': 'SkinFX_Mask', 'stars': 'Stars'},
        # Hair hidden under hats uses vertex-hiding shaders that are not ported.
        'hide_materials': ('nohair',),
    },
}
PROFILE = PROFILES[args.profile]
BASE_COLOR_PARAMETERS = PROFILE['base_color']
NORMAL_PARAMETERS = PROFILE['normal']
SPECULAR_PARAMETERS = PROFILE['specular']


def normal_map(material, shader):
    """`<shader>_n.png` for the game's normal-mapped Phong path: RGB is the
    tangent-space normal (Unreal and Source share the DirectX convention), with
    blue rebuilt from red/green since BC5 exports carry only two channels; alpha
    is the Phong mask from the specular masks' red channel. Stored at half
    the source resolution (pack size); None without a normal map."""
    normal = next((t for t in (texture(material, n) for n in NORMAL_PARAMETERS) if t is not None), None)
    if normal is None:
        return None
    if min(normal.size) > 64:
        normal = normal.resize((normal.width // 2, normal.height // 2), Image.LANCZOS)
    rg = numpy.clip(numpy.asarray(normal, dtype=numpy.float32)[:, :, :2] / 127.5 - 1, -1, 1)
    z = numpy.sqrt(numpy.clip(1 - (rg ** 2).sum(axis=2), 0, 1))
    r, g, _, _ = normal.split()
    b = Image.fromarray(((z * 0.5 + 0.5) * 255).astype(numpy.uint8), 'L')
    specular = next((t for t in (texture(material, n) for n in SPECULAR_PARAMETERS) if t is not None), None)
    a = specular.resize(normal.size).split()[0] if specular is not None else Image.new('L', normal.size, 64)
    files[shader + '_n.png'] = png(Image.merge('RGBA', (r, g, b, a)))
    return shader + '_n.png'


def base_color(material, port):
    """The section's colour texture. An authored base-colour parameter wins:
    the texture audit can mistake low-saturation skin tones for packed masks,
    and the exporter's own pick can be an engine placeholder (mid grey).
    Then material-ports.py's choice; None when neither exists."""
    for name in BASE_COLOR_PARAMETERS:
        image = texture(material, name)
        if image is not None:
            return image
    diffuse_id = port.get('diffuse') if port else material.get('diffuse')
    path = diffuse_id and os.path.join(args.export, 'Textures', diffuse_id + '.png')
    return Image.open(path).convert('RGBA') if path and os.path.isfile(path) else None


def scaled(image, color, size=None):
    """RGB * linear colour (clamped), as an opaque RGB image."""
    image = image.convert('RGB')
    if size:
        image = image.resize(size)
    bands = [b.point(lambda v, k=k: min(255, int(v * k))) for b, k in zip(image.split(), color)]
    return Image.merge('RGB', bands)


def stage(*lines):
    return '\t{\n' + ''.join('\t\t%s\n' % line for line in lines) + '\t}'


def material_shader(material, shader, diffuse):
    """Write a multi-stage shader for effects the plain diffuse cannot show;
    return False when the material needs none.

    Emissive: emissive texture x emissive colour x multiplier (names from the
    profile), added with glow. Galaxy (profile-defined, Fortnite's celestial
    master): the game shades the masked area with a screen-space nebula and
    stars; here an environment-mapped nebula and star layer drift behind the
    lit diffuse, which is blended away where the mask's green channel marks it.
    """
    params = material.get('parameters') or {}
    switches, colors, scalars = params.get('switches', {}), params.get('colors', {}), params.get('scalars', {})
    stages = []
    gx = PROFILE['galaxy']
    galaxy = gx and switches.get(gx['switch']) and texture(material, gx['texture']) and texture(material, gx['mask'])
    if galaxy:
        files[shader + '_nebula.png'] = png(texture(material, gx['texture']).convert('RGB'))
        # UE multiplies the UVs by the tiling, as tcMod scale does.
        tiling = max(0.05, float(scalars.get('GalaxyTiling', 0.5)))
        spin = 360.0 * float(scalars.get('Galaxy Rotation Speed', 0.02))
        stages.append(stage('map %s_nebula.png' % shader, 'tcGen environment', 'tcMod rotate %g' % spin,
                            'tcMod scale %g %g' % (tiling, tiling), 'rgbGen identity'))
        stars = texture(material, gx['stars'])
        if stars:
            # Red and blue hold the star points; green is a soft gradient.
            r, _, b, _ = stars.split()
            # Fortnite drives these at `Star Brightness` (hundreds); triple them.
            points = ImageChops.lighter(r, b).point(lambda v: min(255, v * 3))
            files[shader + '_stars.png'] = png(Image.merge('RGB', (points, points, points)))
            star_tiling = 0.5 * float(scalars.get('Small Star Tiling', 5.0))
            star_spin = 360.0 * float(scalars.get('Small Star Rotation speed', -0.125))
            stages.append(stage('map %s_stars.png' % shader, 'tcGen environment', 'tcMod rotate %g' % star_spin,
                                'tcMod scale %g %g' % (star_tiling, star_tiling), 'blendFunc GL_ONE GL_ONE',
                                'rgbGen identity'))
        mask = texture(material, gx['mask']).resize(diffuse.size).split()[1]
        surface = diffuse.convert('RGB')
        surface.putalpha(mask.point(lambda v: 255 - v))
        files[shader + '_surface.png'] = png(surface)
        stages.append(stage('map %s_surface.png' % shader, 'blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA',
                            'rgbGen lightingDiffuse'))
    em = PROFILE['emissive']
    emissive = (not em['switch'] or switches.get(em['switch'])) and texture(material, em['texture'])
    if emissive:
        c = colors.get(em['color'], {'R': 1, 'G': 1, 'B': 1})
        k = float(scalars.get(em['mult'], 1.0)) if em['mult'] else 1.0
        glow = scaled(emissive, [c['R'] * k, c['G'] * k, c['B'] * k])
        # Unreal's exposure and tone mapping keep a surface-wide emissive
        # subtle; added at full strength it washes the whole part out (or
        # floods it red). Small emissive details (eyes, trims) stay as they
        # are; the stronger the coverage, the more the glow is scaled down.
        coverage = float((numpy.asarray(glow.resize((64, 64))).max(axis=2) > 60).mean())
        if coverage > 0.2:
            factor = max(0.15, 0.2 / coverage)
            glow = glow.point(lambda v: int(v * factor))
        # A near-black result (Skull Trooper's 0.05) adds nothing visible.
        if max(hi for _, hi in glow.getextrema()) >= 8:
            files[shader + '_glow.png'] = png(glow)
            stages.append(stage('map %s_glow.png' % shader, 'blendFunc GL_ONE GL_ONE', 'rgbGen identity', 'glow'))
    if not stages:
        return False
    if not galaxy:
        # Lit diffuse first, effects on top.
        files[shader + '.png'] = png(diffuse)
        stages.insert(0, stage('map %s.png' % shader, 'rgbGen lightingDiffuse'))
    shader_script.append('%s\n{\n%s\n}\n' % (shader, '\n'.join(stages)))
    return True

attach = {}
for spec in args.attach:
    mesh_name, _, value = spec.partition('=')
    bone, *numbers = value.split(',')
    numbers = [float(n) for n in numbers] + [0.0] * (6 - len(numbers))
    attach[mesh_name.lower()] = {'bone': bone, 'location': numbers[0:3], 'rotation': numbers[3:6]}
files = {}
descriptor_parts = []
for part_index, spec in enumerate(args.parts):
    mesh_name, _, overrides = spec.partition('=')
    mesh_id = mesh_ids.get(mesh_name.lower())
    if not mesh_id:
        sys.exit('no exported SkeletalMesh %r (have: %s)' % (mesh_name, ', '.join(sorted(mesh_ids))))
    mesh_path = os.path.join(args.export, 'Meshes', mesh_id + '.skeletal.json')
    mesh = json.load(open(mesh_path))
    # Drop data the game does not read (vertex colours, extra UV channels).
    slim = {k: mesh[k] for k in ('asset', 'vertices', 'indices', 'sections', 'bones')}
    for v in slim['vertices']:
        for k in ('Tangent',):
            v.pop(k, None)
    override_list = overrides.split(',') if overrides else []
    shaders = []
    normals = []
    for s, section in enumerate(mesh['sections']):
        wanted = override_list[s] if s < len(override_list) else '-'
        if wanted == 'off':
            shaders.append('')
            normals.append('')
            continue
        material_id = section.get('material') if wanted == '-' else material_ids.get(wanted.lower())
        material = materials.get(material_id or '')
        # Materials the profile lists (vertex-hiding shaders that are not
        # ported) hide their section outright.
        if material and any(h in material['asset'].split('.')[-1].lower() for h in PROFILE['hide_materials']):
            print('part %d section %d: %s is hidden by the %s profile' % (part_index, s, material['asset'].split('.')[-1], args.profile))
            shaders.append('')
            normals.append('')
            continue
        port = ported.get(material_id or '', {})
        material = material or {'asset': port.get('asset', material_id or '?')}
        diffuse = base_color(material, port)
        if diffuse is None and port.get('solid_color'):
            diffuse = Image.new('RGBA', (64, 64), tuple(int(max(0, min(1, c)) * 255) for c in port['color']) + (255,))
        if diffuse is None:
            print('part %d section %d: no diffuse texture for %r; section hidden' % (part_index, s, wanted if wanted != '-' else material_id))
            shaders.append('')
            normals.append('')
            continue
        shader = '%spart%d_s%d' % (root, part_index, s)
        if material_shader(material, shader, diffuse):
            print('part %d section %d: %s ported as a multi-stage shader' % (part_index, s, material['asset'].split('.')[-1]))
            normals.append('')  # multi-stage surfaces keep the shader path
        else:
            files[shader + '.png'] = png(diffuse)
            normals.append(normal_map(material, shader) or '')
        shaders.append(shader)
        print('part %d %s section %d -> %s (%s)' % (part_index, mesh_name, s, material['asset'].split('.')[-1], shader))
    mesh_qpath = '%spart%d.skeletal.json' % (root, part_index)
    files[mesh_qpath] = json.dumps(slim, separators=(',', ':')).encode()
    entry = {'name': mesh_name.split('.')[-1], 'mesh': mesh_qpath, 'shaders': shaders}
    if any(normals):
        entry['normals'] = normals
    if mesh_name.lower() in attach:
        entry['attach'] = attach[mesh_name.lower()]
        print('part %d %s attached to %s' % (part_index, mesh_name, entry['attach']))
    descriptor_parts.append(entry)

# Unreal mannequin bone -> the `_humanoid` bone an idle effect is seated on.
GLA_BONE = {'root': 'model_root', 'pelvis': 'pelvis', 'spine_01': 'lower_lumbar', 'spine_02': 'lower_lumbar',
            'spine_03': 'upper_lumbar', 'spine_04': 'thoracic', 'spine_05': 'thoracic', 'neck_01': 'cervical',
            'neck_02': 'cervical', 'head': 'cranium', 'hand_l': 'lhand', 'hand_r': 'rhand'}
idle_effects = []
if args.idle:
    baked = json.load(open(os.path.join(args.idle, 'idle.json')))
    for i, effect in enumerate(baked['effects']):
        bone = GLA_BONE.get(effect['bone'].lower())
        if not bone:
            print('idle effect %s: bone %s has no _humanoid mapping; skipped' % (effect['system'], effect['bone']))
            continue
        track = '%sidle%d.fxt' % (root, i)
        files[track] = open(os.path.join(args.idle, effect['track']), 'rb').read()
        idle_effects.append({'track': track, 'bone': bone, 'loop_start': effect['loop_start'], 'renderers': effect['renderers']})
        print('idle effect %s on %s (%d renderers)' % (effect['system'].rsplit('.', 1)[-1], bone, len(effect['renderers'])))
    # Ported particle materials, then the particle meshes they draw. Their
    # shader scripts get skin-specific file names: a pk3 file with the same
    # path as the emote library's would replace its whole script, while the
    # definitions inside are merged by name. The asset-browser index is the
    # library's alone.
    def add_native(name, data):
        if name == 'models/fnemote/index.json':
            return
        if name.startswith('shaders/') and name.endswith('.shader'):
            name = 'shaders/%s_%s' % (args.name, name[len('shaders/'):])
        files.setdefault(name, data)
    native_root = os.path.join(args.idle, 'fx-native')
    for folder, _, names in os.walk(native_root):
        for n in names:
            path = os.path.join(folder, n)
            add_native(os.path.relpath(path, native_root).replace(os.sep, '/'), open(path, 'rb').read())
    if args.native and idle_effects:
        with zipfile.ZipFile(args.native) as native:
            for n in native.namelist():
                if not n.endswith('/'):
                    add_native(n, native.read(n))

descriptor = {
    'format': 'ue_skeletal',
    'schema_version': 1,
    'female': args.female,
    'parts': descriptor_parts,
}
if args.title:
    descriptor['title'] = args.title
if idle_effects:
    descriptor['idle_effects'] = idle_effects
if args.bone_map:
    descriptor['bone_map'] = json.load(open(args.bone_map, encoding='utf-8'))
files[root + 'source_model.json'] = json.dumps(descriptor, indent=2).encode()
if shader_script:
    files['shaders/%s.shader' % args.name] = '\n'.join(shader_script).encode()
files[root + 'sounds.cfg'] = b'mp_generic_female\nf\n' if args.female else b'mp_generic_male\nm\n'
if args.icon:
    files[root + 'icon_default.png'] = open(args.icon, 'rb').read()
with zipfile.ZipFile(args.out, 'w', zipfile.ZIP_DEFLATED) as z:
    for name, data in sorted(files.items()):
        z.writestr(name, data)
print('wrote %s: %d files, %.1f MB' % (args.out, len(files), os.path.getsize(args.out) / 1e6))
