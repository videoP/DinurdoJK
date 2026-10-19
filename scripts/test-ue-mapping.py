"""End-to-end test of the Unreal -> Radiant/q3map2 mapping pipeline.

    python scripts/test-ue-mapping.py

Builds a tiny synthetic export directory, imports it with `djk-ue-import`
(DJK_UE_IMPORT=<exe> to use a prebuilt binary, otherwise `cargo run`), turns
the result into a mapping pack with ue-mapper-pack.py, then packages a fake
BSP that uses it with pack-map.py.
"""
import hashlib
import json
import os
import struct
import subprocess
import sys
import tempfile
import unittest
import zipfile

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)


def object_id(path):
    return hashlib.sha256(path.lower().encode()).hexdigest()[:24]


def write_json(path, value):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        json.dump(value, f)


def make_export(directory):
    mesh_path, wood, glass = '/Game/Props/SM_Crate.SM_Crate', '/Game/M_Wood.M_Wood', '/Game/M_Glass.M_Glass'
    texture = '/Game/T_Wood.T_Wood'
    ids = {k: object_id(v) for k, v in dict(mesh=mesh_path, wood=wood, glass=glass, texture=texture).items()}
    write_json(os.path.join(directory, 'report.json'),
               {'assets': [{'asset': mesh_path, 'id': ids['mesh'], 'status': 'static_mesh'}]})
    write_json(os.path.join(directory, 'dependencies.json'), {'seeds': [{'asset': mesh_path}]})
    write_json(os.path.join(directory, 'Meshes', ids['mesh'] + '.json'), {
        'asset': mesh_path,
        'vertices': [[0, 0, 0, 0, 0, 1, 0, 0], [10, 0, 0, 0, 0, 1, 1, 0], [0, 10, 0, 0, 0, 1, 0, 1], [10, 10, 0, 0, 0, 1, 1, 1]],
        'indices': [0, 1, 2, 2, 1, 3],
        'sections': [{'first_index': 0, 'triangles': 1, 'material': ids['wood']},
                     {'first_index': 3, 'triangles': 1, 'material': ids['glass']}]})
    write_json(os.path.join(directory, 'materials.json'), {
        ids['wood']: {'asset': wood, 'diffuse': ids['texture'], 'blend': 'BLEND_Opaque'},
        ids['glass']: {'asset': glass, 'diffuse': ids['texture'], 'blend': 'BLEND_Translucent'}})
    os.makedirs(os.path.join(directory, 'Textures'))
    Image.new('RGBA', (4, 4), (180, 120, 60, 255)).save(os.path.join(directory, 'Textures', ids['texture'] + '.png'))


def fake_bsp(path, shaders):
    """A Jedi Academy RBSP whose only populated lump is the shader lump."""
    body = b''.join(s.encode().ljust(64, b'\0') + struct.pack('<ii', 0, 0) for s in shaders)
    header = bytearray(b'RBSP' + struct.pack('<i', 1) + bytes(17 * 8))
    struct.pack_into('<ii', header, 8 + 8, len(header), len(body))  # lump 1 = shaders
    with open(path, 'wb') as f:
        f.write(bytes(header) + body)


class MappingPipeline(unittest.TestCase):
    def run_tool(self, *args):
        subprocess.run([sys.executable, '-I', *args], check=True, capture_output=True, text=True)

    def test_import_pack_and_ship_map(self):
        with tempfile.TemporaryDirectory() as tmp:
            export, native = os.path.join(tmp, 'export'), os.path.join(tmp, 'native.pk3')
            make_export(export)
            importer = os.environ.get('DJK_UE_IMPORT')
            command = [importer] if importer else ['cargo', 'run', '--profile', 'fast', '-q', '-p', 'jka-ue-assets', '--']
            subprocess.run([*command, 'import', export, native, '--namespace', 'models/mygame'], check=True, cwd=ROOT)

            pack = os.path.join(tmp, 'pack')
            self.run_tool(os.path.join(HERE, 'ue-mapper-pack.py'), native, 'mygame', pack)
            self.assertTrue(os.path.isfile(os.path.join(pack, 'models', 'mygame', 'crate.md3')))
            self.assertTrue(os.path.isfile(os.path.join(pack, 'textures', 'mygame', 'wood.tga')))
            with open(os.path.join(pack, 'shaders', 'mygame.shader')) as f:
                shader = f.read()
            self.assertIn('textures/mygame/glass', shader)  # translucent needs a script
            self.assertNotIn('textures/mygame/wood\n', shader)  # opaque stays implicit

            bsp, out = os.path.join(tmp, 'crate.bsp'), os.path.join(tmp, 'crate.pk3')
            fake_bsp(bsp, ['textures/mygame/wood', 'textures/mygame/glass', 'textures/system/clip'])
            self.run_tool(os.path.join(HERE, 'pack-map.py'), out, bsp, '--from', pack)
            with zipfile.ZipFile(out) as z:
                names = set(z.namelist())
            self.assertTrue({'maps/crate.bsp', 'textures/mygame/wood.tga', 'textures/mygame/glass.tga',
                             'shaders/crate.shader'} <= names, names)
            self.assertNotIn('textures/system/clip.tga', names)  # stock content is left to the game


if __name__ == '__main__':
    unittest.main()
