"""Package compiled maps with exactly the textures and shaders they use.

    python scripts/pack-map.py <out.pk3> <map.bsp> [more.bsp ...] --from <dir or .pk3> [--from ...]

Reads each BSP's shader lump (Q3 IBSP or Jedi Academy RBSP) and copies, from
the --from sources (e.g. a ue-mapper-pack output), every texture a used
shader needs: the shader scripts' `map`/`qer_editorimage` images, or the
implicit image named like the shader. Shaders and textures not found in the
sources (stock game content such as textures/system/clip) are left out and
listed. The resulting pk3 is a normal self-contained map package.
"""
import argparse
import os
import re
import struct
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('out')
parser.add_argument('maps', nargs='+')
parser.add_argument('--from', dest='sources', action='append', default=[], required=True)
args = parser.parse_args()

# Every file the sources offer, by lower-case game path.
available = {}
for source in args.sources:
    if os.path.isdir(source):
        for folder, _, names in os.walk(source):
            for n in names:
                path = os.path.join(folder, n)
                available[os.path.relpath(path, source).replace(os.sep, '/').lower()] = ('dir', path)
    else:
        with zipfile.ZipFile(source) as z:
            for n in z.namelist():
                if not n.endswith('/'):
                    available[n.lower()] = ('pk3', source, n)


def read(entry):
    if entry[0] == 'dir':
        return open(entry[1], 'rb').read()
    with zipfile.ZipFile(entry[1]) as z:
        return z.read(entry[2])


# Shader script blocks offered by the sources, by shader name.
blocks = {}
for name, entry in available.items():
    if name.startswith(('shaders/', 'scripts/')) and name.endswith('.shader'):
        text = read(entry).decode('latin-1')
        for m in re.finditer(r'^\s*([^\s{}]+)\s*\n\s*\{', text, re.M):
            depth, i = 0, m.end() - 1
            while i < len(text):
                depth += {'{': 1, '}': -1}.get(text[i], 0)
                i += 1
                if depth == 0:
                    break
            blocks[m.group(1).lower()] = text[m.start():i].strip() + '\n'


def bsp_shaders(path):
    d = open(path, 'rb').read()
    magic = d[:4]
    if magic not in (b'IBSP', b'RBSP'):
        raise SystemExit('%s: not a Q3/JA BSP' % path)
    ofs, length = struct.unpack_from('<ii', d, 8 + 8 * 1)
    return [d[ofs + 72 * i:ofs + 72 * i + 64].split(b'\0')[0].decode('latin-1') for i in range(length // 72)]


def image(path):
    base = os.path.splitext(path)[0].lower()
    for ext in ('.tga', '.jpg', '.png', ''):
        if base + ext in available:
            return base + ext
    return None


files = {}
scripts = []
skipped = []
for bsp in args.maps:
    files['maps/' + os.path.basename(bsp)] = open(bsp, 'rb').read()
    for shader in bsp_shaders(bsp):
        key = shader.lower()
        if key in ('noshader',):
            continue
        if key in blocks:
            if blocks[key] not in scripts:
                scripts.append(blocks[key])
            images = re.findall(r'(?:\bmap|qer_editorimage|clampmap)\s+([^\s$][^\s]*)', blocks[key], re.I)
        else:
            images = [shader]
        found = [image(i) for i in images]
        if not any(found):
            skipped.append(shader)
        for f in filter(None, found):
            files[f] = read(available[f])

name = os.path.splitext(os.path.basename(args.out))[0]
if scripts:
    files['shaders/%s.shader' % name] = '\n'.join(scripts).encode('latin-1')
with zipfile.ZipFile(args.out, 'w', zipfile.ZIP_DEFLATED) as z:
    for n, data in sorted(files.items()):
        z.writestr(n, data)
print('%s: %d maps, %d textures, %d shaders; from the game itself: %s' % (
    args.out, len(args.maps), sum(n.startswith('textures/') for n in files), len(scripts), ', '.join(sorted(set(skipped))) or 'none'))
