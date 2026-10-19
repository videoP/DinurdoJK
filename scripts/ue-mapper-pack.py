"""Turn a native Unreal asset import into a mapping pack for Radiant/q3map2.

    python scripts/ue-mapper-pack.py <native import.pk3> <pack name> <out dir> [--pk3 out.pk3] [--jpg]

<native import.pk3> is `djk-ue-import import ...` output (it carries a
<namespace>/index.json, found automatically; pass --index to pick one when a
pk3 holds several). Any Unreal game works once its assets are exported (see
crates/jka-ue-assets/README.md and MAPPING.md).

The pack uses stock id Tech 3 / Jedi Academy conventions so mappers can place
the props as misc_model and q3map2 compiles them into an ordinary map:
  models/<pack>/<asset>.md3         readable names, surfaces -> textures/<pack>/<material>
  textures/<pack>/<material>.tga    (or .jpg with --jpg for opaque materials)
  shaders/<pack>.shader             only for masked, translucent and additive
                                    materials; every shader has qer_editorimage
Opaque materials use implicit shaders (texture x lightmap), so q3map2 lights
them like brush faces. The native pk3 itself stays the format for DinurdoJK's
runtime props; this pack drops its engine-only material programs.
"""
import argparse
import io
import json
import os
import re
import struct
import sys
import zipfile

from PIL import Image

parser = argparse.ArgumentParser()
parser.add_argument('native')
parser.add_argument('pack')
parser.add_argument('out')
parser.add_argument('--pk3', help='also write the pack as this .pk3')
parser.add_argument('--jpg', action='store_true', help='store opaque textures as JPEG (smaller)')
parser.add_argument('--index', help='index.json member to use (default: the only <namespace>/index.json)')
args = parser.parse_args()
if not re.fullmatch(r'[a-z0-9_]+', args.pack):
    sys.exit('pack name must be lower-case letters, digits and _')

source = zipfile.ZipFile(args.native)
indexes = [n for n in source.namelist() if n.endswith('/index.json') and n.startswith('models/')]
if args.index:
    indexes = [args.index]
if len(indexes) != 1:
    sys.exit('expected exactly one models/<namespace>/index.json in %s, found %s; use --index' % (args.native, indexes or 'none'))
index = json.loads(source.read(indexes[0]))
files = {}
used_names = set()


# MD3 stores shader and model names in 64-byte fields (63 characters).
MAX_NAME = 63 - len('textures/%s/' % args.pack) - 4


def slug(text, fallback):
    s = re.sub(r'[^a-z0-9]+', '_', (text or '').lower()).strip('_') or fallback
    return s[:MAX_NAME].rstrip('_')


def unique(folder, name):
    candidate, n = name, 2
    while (folder, candidate) in used_names:
        suffix = '_%d' % n
        candidate = name[:MAX_NAME - len(suffix)] + suffix
        n += 1
    used_names.add((folder, candidate))
    return candidate


def image_bytes(image, fmt):
    out = io.BytesIO()
    if fmt == 'jpg':
        image.convert('RGB').save(out, 'JPEG', quality=90)
    else:
        image.save(out, 'TGA', rle=True)
    return out.getvalue()


# Materials: one texture (or colour swatch) and, when needed, a shader.
shaders = []
shader_of = {}
for m in index.get('materials', []):
    name = unique('textures', slug(m['asset'].split('.')[-1].removeprefix('MI_').removeprefix('M_'), m['id']))
    shader = 'textures/%s/%s' % (args.pack, name)
    shader_of[m['id']] = shader
    color = [max(0., min(1., c)) for c in (m.get('color') or [1, 1, 1])]
    texture = m.get('texture') or ''
    try:
        image = Image.open(io.BytesIO(source.read(texture))).convert('RGBA') if texture and 'neutral' not in texture else None
    except KeyError:
        image = None
    if image is None:
        image = Image.new('RGBA', (32, 32), tuple(int(c * 255) for c in color) + (255,))
    elif any(abs(c - 1) > 0.01 for c in color):
        # Fold the authored tint into the texture; stock shaders cannot tint.
        r, g, b, a = image.split()
        r, g, b = (band.point(lambda v, k=k: int(v * k)) for band, k in zip((r, g, b), color))
        image = Image.merge('RGBA', (r, g, b, a))
    blend = m.get('blend', 'BLEND_Opaque')
    opaque = blend in ('BLEND_Opaque', '')
    fmt = 'jpg' if args.jpg and opaque else 'tga'
    texture_path = '%s.%s' % (shader, fmt)
    files[texture_path] = image_bytes(image, fmt)
    if opaque:
        continue  # implicit shader: same name as the texture
    if blend == 'BLEND_Masked':
        stages = ['\t{\n\t\tmap %s\n\t\talphaFunc GE128\n\t\tdepthWrite\n\t\trgbGen identity\n\t}' % texture_path,
                  '\t{\n\t\tmap $lightmap\n\t\tblendFunc GL_DST_COLOR GL_ZERO\n\t\tdepthFunc equal\n\t\trgbGen identity\n\t}']
        parms = ['cull none', 'surfaceparm alphashadow']
    elif blend == 'BLEND_Additive':
        stages = ['\t{\n\t\tmap %s\n\t\tblendFunc GL_ONE GL_ONE\n\t\trgbGen identity\n\t}' % texture_path]
        parms = ['surfaceparm trans', 'surfaceparm nolightmap', 'surfaceparm nonsolid', 'cull none']
    else:  # translucent / modulate: alpha blend, lit by q3map2's vertex light
        stages = ['\t{\n\t\tmap %s\n\t\tblendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA\n\t\trgbGen vertex\n\t}' % texture_path]
        parms = ['surfaceparm trans', 'surfaceparm nolightmap', 'cull none']
    shaders.append('%s\n{\n\tqer_editorimage %s\n%s\n%s\n}\n' % (shader, texture_path, ''.join('\t%s\n' % p for p in parms).rstrip('\n'), '\n'.join(stages)))

# The importer's placeholders for materials it could not port.
for placeholder in ('neutral', 'unsupported'):
    shader_of[placeholder] = 'textures/%s/%s' % (args.pack, placeholder)
    files['textures/%s/%s.tga' % (args.pack, placeholder)] = image_bytes(Image.new('RGBA', (32, 32), (160, 160, 160, 255)), 'tga')

# Models: rename, and point every surface at its material's readable shader.
SURFACE = struct.Struct('<4s64siiiiiiiiii')
models = []
for row in index.get('models', []):
    try:
        data = bytearray(source.read(row['model']))
    except KeyError:
        continue
    if data[:4] != b'IDP3':
        continue
    name = unique('models', slug(row['asset'].split('.')[-1].removeprefix('SM_'), 'model'))
    path = 'models/%s/%s.md3' % (args.pack, name)
    struct.pack_into('<64s', data, 8, path.encode()[:63])
    num_surfaces, ofs_surfaces = struct.unpack_from('<i', data, 84)[0], struct.unpack_from('<i', data, 100)[0]
    at = ofs_surfaces
    for _ in range(num_surfaces):
        (_, _, _, _, num_shaders, _, _, _, ofs_shaders, _, _, ofs_end) = SURFACE.unpack_from(data, at)
        for k in range(num_shaders):
            slot = at + ofs_shaders + k * 68
            old = data[slot:slot + 64].split(b'\0')[0].decode(errors='replace')
            material = old.rsplit('/', 1)[-1].removeprefix('m_').removesuffix('.tga')
            new = shader_of.get(material, 'textures/%s/missing' % args.pack)
            struct.pack_into('<64s', data, slot, new.encode()[:63])
        at += ofs_end
    files[path] = bytes(data)
    models.append((path, row.get('display_name', name), row['asset']))

if shaders:
    files['shaders/%s.shader' % args.pack] = '\n'.join(shaders).encode()
files['models/%s/README.txt' % args.pack] = ('Unreal props converted for Radiant/q3map2. Place as misc_model '
    '(spawnflag 2 or _clipmodel for solid). Models:\n' + ''.join('%s\t%s\t%s\n' % m for m in models)).encode()

for name, data in files.items():
    path = os.path.join(args.out, *name.split('/'))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, 'wb').write(data)
if args.pk3:
    with zipfile.ZipFile(args.pk3, 'w', zipfile.ZIP_DEFLATED) as z:
        for name, data in sorted(files.items()):
            z.writestr(name, data)
print('%d models, %d materials (%d with shaders) -> %s%s' % (len(models), len(shader_of), len(shaders), args.out,
                                                           ' and ' + args.pk3 if args.pk3 else ''))
