"""Resolve exported UE material parameters to the native material port schema.

    python material-ports.py <export dir>

Reads <export dir>/materials.json (and texture-classes.json from
`djk-ue-import audit-textures`, when present) and writes
native-materials-v2.json for `djk-ue-import import`.

Keep untranslatable inputs visible in the conversion report; never substitute
an arbitrary normal/position/VAT texture for an albedo. Cooked Niagara/Blueprint
programs are not equivalent to material parameter tables.

Textures are found by *parameter name*: the case-insensitive name lists inside
`run()` (Diffuse/BaseColor/Albedo/..., tints, emissive, normals) are the place
to add your project's master-material parameter names. The hologram and
invisible-material ports near the end are examples of project-specific ports
keyed on parameter names or asset names; copy that shape for your own.
"""
import argparse, hashlib, json, pathlib, re, math, struct

def asset_id(path):
    return hashlib.sha256(path.lower().encode()).hexdigest()[:24]

def run(source):
    materials=json.loads((source/'materials.json').read_text(encoding='utf-8'))
    # Content classes from `djk-fnemote texture-audit`: never show a normal map
    # or channel-packed mask as colour (that is what rendered purple).
    classes_path=source/'texture-classes.json'
    classes=json.loads(classes_path.read_text(encoding='utf-8')) if classes_path.is_file() else {}
    # Multi-hue textures whose channels are almost all fully on or off are RGB
    # region/ID masks that the material graph recolours; as albedo they show
    # as rainbow primaries. Real colourful art has mid tones and is kept.
    def id_mask(texture_id):
        c=classes.get(texture_id,{})
        return c.get('class')=='multimask' and c.get('pure',0)>0.75
    def colour_texture(texture_id):
        return bool(texture_id) and (source/'Textures'/f'{texture_id}.png').is_file() and classes.get(texture_id,{}).get('class') not in ('normal','packed') and not id_mask(texture_id)
    native={}
    for key,m in materials.items():
        p=m.get('parameters',{}); textures=p.get('textures',{}); colors=p.get('colors',{}); scalars=p.get('scalars',{})
        def texture(names):
            for name in names:
                for field,path in textures.items():
                    if field.lower()==name.lower() and path and colour_texture(asset_id(path)):
                        return asset_id(path)
        diffuse=m.get('diffuse')
        if not colour_texture(diffuse):
            diffuse=texture(['Diffuse','BaseColor','Base Color','Albedo','Main Tex','MainTexture','Texture',
                             'PM_Diffuse','Color_Lit_Map','Color_Shaded_Map','ColorTexture','Color Texture'])
        def color(names):
            for name in names:
                for field,value in colors.items():
                    if field.lower()==name.lower() and isinstance(value,dict):
                        return [float(value.get(c,1)) for c in ['R','G','B']]
        tint=color(['Base Color','BaseColor','Diffuse Color','BaseColorTint','DiffuseTint','ColorTint','TintColor','LightConeColor'])
        unlit=m.get('shading_model')=='MSM_Unlit'
        # Explicit authored color/flare inputs, not normal/VAT texture guesses.
        # These are native input ports; their missing procedural layers remain
        # recorded below even when a useful base color can now be displayed.
        if unlit and not tint:tint=color(['Color','ParticleColor','HDR_Tint'])
        if unlit and not diffuse and ('MeshSprites' in m['asset'] or 'Flare' in m['asset']):
            diffuse=texture(['FlareA','Flare','FlareTexture','Flare Texture'])
        emissive=texture(['Emissive','EmissiveTexture','Emissive Texture','EmissiveMask'])
        glow=color(['EmissiveColor','Emissive Color','EmissiveTint'])
        brightness=next((float(v) for k,v in scalars.items() if k.lower() in ['emissivebrightness','emissive intensity','emissiveintensity']),1)
        switches=p.get('switches',{})
        if any(k.lower() in ['usebasicemissive','useemissive'] and not v for k,v in switches.items()):emissive=None
        normal=next((asset_id(path) for name in ['Normals','Normal','NormalMap','Normal Texture'] for field,path in textures.items()
                     if field.lower()==name.lower() and path and (source/'Textures'/f'{asset_id(path)}.png').is_file()
                     and classes.get(asset_id(path),{}).get('class') in (None,'normal')),None)
        # Role fallback for graphs whose base color is computed: choose a
        # colour/shape texture by parameter role, never normal/noise/WPO maps.
        # Opaque surfaces only accept colour-like roles; translucent/additive
        # FX also accept masks, flipbooks and flares (shape * vertex colour).
        role=None
        if not diffuse and not tint:
            exclude=['normal','noise','distort','wpo','height','reflection','ramp','gradientmap','lut','cube','refraction','depth','vat','position','rotation','roughness','metal','specular','ao','gradient','curve','lines','motion','panning']
            roles=['basecolor','diffuse','albedo','color','base','texture','emissive']
            if m.get('blend') in ('BLEND_Translucent','BLEND_Additive','BLEND_Masked'):
                roles=['subuv','flipbook','sprite','particle','flare','mask','alpha','shape','halo','glow','spark','star','smoke','cloud','ring']+roles
            best=None
            for field,path in textures.items():
                role_name=field.lower().replace(' ','').replace('_','')
                leaf=(path or '').rsplit('.',1)[-1].lower()
                if not path or any(x in role_name for x in exclude) or 'normal' in leaf or 'noise' in leaf or 'blank' in leaf:
                    continue
                rank=next((i for i,r in enumerate(roles) if r in role_name),None)
                # A multi-hue texture reached only by role is channel data the
                # graph recolours; drawn as albedo it is rainbow primaries.
                if classes.get(asset_id(path),{}).get('class')=='multimask':
                    continue
                if rank is not None and colour_texture(asset_id(path)) and (best is None or rank<best[0]):
                    best=(rank,asset_id(path))
            if best:
                diffuse=best[1];role=True
                if unlit and not tint:tint=color(['Color','Color A','ColorA','Tint','Emissive Color','EmissiveColor'])
                if tint and max(tint)>1:tint=[c/max(tint) for c in tint]
        native[key]={'asset':m['asset'],'diffuse':diffuse,'blend':m.get('blend',''),'color':tint or [1,1,1],
            'solid_color':bool(tint and not diffuse),'emissive':emissive,'emissive_color':[v*brightness for v in (glow or [1,1,1])],
            'normal':normal,'unlit':unlit,'status':'role_texture_port' if role else 'material_inputs_ported' if diffuse or tint else 'unsupported_graph',
            'opacity':max(0,min(1,next((float(v) for k,v in scalars.items() if k.lower() in ['opacity','opacitymult','opacity multiplier']),1))),
            'unsupported':['Original per-pixel procedural graph is not executed'] if not diffuse and not tint else ['Texture chosen by parameter role; procedural graph layers are not executed'] if role else []}
        if native[key]['status']=='unsupported_graph':
            # Procedural graphs with no texture inputs: keep their authored
            # colour (glow/tint/emissive) instead of a diagnostic checker; with
            # nothing authored, a neutral surface. Both stay labelled as ports.
            translucent=m.get('blend') in ('BLEND_Translucent','BLEND_Additive')
            ranked=[]
            for field,value in colors.items():
                if not isinstance(value,dict):continue
                name=field.lower().replace(' ','').replace('_','')
                if not any(r in name for r in ['color','tint','glow','emissive','light','inner','outer']):continue
                rgb=[max(0.0,float(value.get(c,0))) for c in 'RGB']
                if max(rgb)<=0.01:continue
                ranked.append((0 if 'base' in name or name in ('color','tint') else 1 if 'emissive' in name or 'glow' in name else 2,field,rgb))
            ranked.sort()
            if ranked:
                rgb=ranked[0][2];peak=max(rgb)
                native[key].update(solid_color=True,color=[c/peak for c in rgb] if peak>1 else rgb,unlit=unlit or translucent or peak>1,
                    status='authored_color_port',unsupported=[f'Constant colour from {ranked[0][1]}; procedural graph layers are not executed'])
            else:
                native[key].update(solid_color=True,color=[0.8,0.8,0.8],unlit=translucent,opacity=0.35 if translucent else native[key]['opacity'],
                    status='neutral_port',unsupported=['No texture or colour inputs exported; neutral surface substituted for the procedural graph'])
        core=color(['FresnelCoreColor']);edge=color(['FresnelEdgeColor'])
        if unlit and core and edge:
            nodes=[{'op':'constant','value':core+[1]}, {'op':'constant','value':edge+[1]},
                   {'op':'fresnel','exponent':float(scalars.get('FresnelExp',scalars.get('FresnelExponent',5)))},
                   {'op':'lerp','inputs':[0,1,2]}]
            native[key].update(solid_color=True,color=[1,1,1],program={'version':1,'unlit':True,'nodes':nodes,'output':3},status='native_fresnel_color_port',
                               unsupported=['Authored core/edge colors ported per vertex; remaining procedural graph layers are unavailable'])
        if m['asset'].rsplit('.',1)[-1]=='M_Invisible':
            native[key].update(solid_color=True,opacity=0,unlit=True,status='native_invisible_port',unsupported=[])
        # A cooked material instance exposes parameters, not its editor graph.
        # This explicit native hologram port preserves authored colors and
        # animation inputs; it is labelled as a port, never an exact graph decode.
        hologram=color(['SlateColor-Bright'])
        if hologram and 'ScanlineTile' in scalars:
            nodes=[]
            def node(op,inputs=None,**fields):
                n={'op':op,**fields}
                if inputs is not None:n['inputs']=inputs
                nodes.append(n);return len(nodes)-1
            def const(v):return node('constant',value=v if isinstance(v,list) else [v]*4)
            base=const(hologram+[0]);rim=node('fresnel',exponent=float(scalars.get('FresnelExp',2.5)))
            intensity=node('max',[rim,const(float(scalars.get('FresnelMin',.4)))])
            time=node('time');phase=node('multiply',[time,const(float(scalars.get('FlickerSpeed',.5))*math.tau)])
            pulse=node('add',[node('multiply',[node('sine',[phase]),const(.04)]),const(.96)])
            rgb=node('multiply',[node('multiply',[base,intensity]),pulse])
            alpha=node('multiply',[const([0,0,0,1]),node('saturate',[node('multiply',[intensity,const(float(scalars.get('OpacityMult',1)))])])])
            output=node('add',[rgb,alpha])
            native[key].update(color=hologram,unlit=True,two_sided=True,program={'version':1,'unlit':True,'nodes':nodes,'output':output},status='native_hologram_port',normal=None)
            # Grayscale scanline alpha is sampled per fragment by the existing
            # material renderer; fresnel/pulse expressions run per vertex.
            directory=source/'NativeTextures';directory.mkdir(exist_ok=True)
            name=f'hologram_{key}.tga';width,height=8,256
            header=struct.pack('<BBBHHBHHHHBB',0,0,2,0,0,0,0,0,width,height,32,0x28)
            minimum=max(0,min(1,float(scalars.get('MinScanlineOpacity',.5))))
            pixels=bytearray()
            for y in range(height):
                alpha=int(255*(minimum+(1-minimum)*(1 if y%4<2 else 0)))
                pixels.extend(bytes([255,255,255,alpha])*width)
            (directory/name).write_bytes(header+pixels)
            native[key]['native_texture']=name
            native[key]['unsupported']=['Vertex fresnel and scanline port; original cooked shader graph is unavailable','Hologram WPO/noise graph not reconstructed']
    (source/'native-materials-v2.json').write_text(json.dumps(native,indent=2),encoding='utf-8')
    print(f'Resolved {len(native)} materials; {sum(m["status"]=="unsupported_graph" for m in native.values())} need graph ports')

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('export_directory',type=pathlib.Path)
    run(parser.parse_args().export_directory)
