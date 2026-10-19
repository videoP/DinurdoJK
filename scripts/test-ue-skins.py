"""Package a synthetic Unreal skeletal mesh with ue-skin-package.py and check the PK3.

    python scripts/test-ue-skins.py

No game or Unreal data needed: the export directory is built here.
"""
import hashlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
import zipfile

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))


def object_id(path):
    return hashlib.sha256(path.lower().encode()).hexdigest()[:24]


def write_json(path, value):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w') as f:
        json.dump(value, f)


def make_export(directory, size=64):
    mesh_path, material, base, normal = '/Game/SK_Test.SK_Test', '/Game/M_Skin.M_Skin', '/Game/T_Base.T_Base', '/Game/T_Normal.T_Normal'
    ids = {k: object_id(v) for k, v in dict(mesh=mesh_path, material=material, base=base, normal=normal).items()}
    write_json(os.path.join(directory, 'report.json'), {'assets': [
        {'asset': mesh_path, 'id': ids['mesh'], 'type': 'SkeletalMesh'},
        {'asset': base, 'id': ids['base'], 'type': 'Texture2D'},
        {'asset': normal, 'id': ids['normal'], 'type': 'Texture2D'}]})
    identity = {'Rotation': {'X': 0, 'Y': 0, 'Z': 0, 'W': 1}, 'Translation': {'X': 0, 'Y': 0, 'Z': 0}, 'Scale3D': {'X': 1, 'Y': 1, 'Z': 1}}
    vertex = lambda x: {'Position': {'X': x, 'Y': 0, 'Z': 0}, 'Normal': {'X': 0, 'Y': 0, 'Z': 1}, 'Uv': {'U': 0, 'V': 0},
                        'Influences': [{'Bone': 0, 'Weight': 1.0}]}
    write_json(os.path.join(directory, 'Meshes', ids['mesh'] + '.skeletal.json'), {
        'asset': mesh_path, 'vertices': [vertex(0), vertex(1), vertex(2)], 'indices': [0, 1, 2],
        'sections': [{'first_index': 0, 'triangles': 1, 'material': ids['material']}],
        'bones': [{'Name': 'root', 'ParentIndex': -1, 'Transform': identity}]})
    write_json(os.path.join(directory, 'materials.json'), {ids['material']: {
        'asset': material, 'diffuse': ids['base'], 'blend': 'BLEND_Opaque',
        'parameters': {'textures': {'BaseColor': base, 'Normal': normal}, 'colors': {}, 'scalars': {}, 'switches': {}}}})
    os.makedirs(os.path.join(directory, 'Textures'))
    Image.new('RGBA', (size, size), (200, 120, 80, 255)).save(os.path.join(directory, 'Textures', ids['base'] + '.png'))
    # A flat tangent-space normal map (BC5 exports carry only red/green).
    Image.new('RGBA', (size * 2, size * 2), (128, 128, 255, 255)).save(os.path.join(directory, 'Textures', ids['normal'] + '.png'))


class SkinPackaging(unittest.TestCase):
    def test_packages_a_descriptor_textures_and_half_size_normals(self):
        with tempfile.TemporaryDirectory() as tmp:
            export, out = os.path.join(tmp, 'export'), os.path.join(tmp, 'skin.pk3')
            make_export(export)
            bone_map = os.path.join(tmp, 'bones.json')
            write_json(bone_map, {'root': 'ValveBiped.Bip01_Pelvis'})
            result = subprocess.run([sys.executable, '-I', os.path.join(HERE, 'ue-skin-package.py'), export, 'test_skin', out,
                                     '--title', 'Test Skin', '--bone-map', bone_map, 'SK_Test'], capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            with zipfile.ZipFile(out) as z:
                names = set(z.namelist())
                root = 'models/players/test_skin/'
                self.assertTrue({root + 'source_model.json', root + 'part0.skeletal.json', root + 'part0_s0.png',
                                 root + 'part0_s0_n.png', root + 'sounds.cfg'} <= names, names)
                descriptor = json.loads(z.read(root + 'source_model.json'))
                self.assertEqual(descriptor['format'], 'ue_skeletal')
                self.assertEqual(descriptor['title'], 'Test Skin')
                self.assertEqual(descriptor['bone_map'], {'root': 'ValveBiped.Bip01_Pelvis'})
                self.assertEqual(descriptor['parts'][0]['shaders'], [root + 'part0_s0'])
                self.assertEqual(descriptor['parts'][0]['normals'], [root + 'part0_s0_n.png'])
                normal = Image.open(io.BytesIO(z.read(root + 'part0_s0_n.png')))
                self.assertEqual(normal.size, (64, 64))  # 128x128 source, halved
                self.assertEqual(normal.mode, 'RGBA')

    def test_generic_profile_does_not_apply_fortnite_hiding(self):
        with open(os.path.join(HERE, 'ue-skin-package.py'), encoding='utf-8') as f:
            text = f.read()
        self.assertIn("'hide_materials': ()", text)  # the default profile hides nothing


if __name__ == '__main__':
    unittest.main()
