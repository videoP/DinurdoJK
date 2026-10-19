"""Verify a presentation bake keeps character motion/audio byte-identical.

Compare compressed member payloads directly; works with Zstandard bundles
without requiring Python's zipfile to implement that compression method.
"""
import argparse, json, pathlib, struct, zipfile

def payload(path, member):
    with path.open('rb') as stream:
        stream.seek(member.header_offset)
        header=stream.read(30)
        assert header[:4]==b'PK\x03\x04'
        name,extra=struct.unpack_from('<HH',header,26)
        stream.seek(name+extra,1)
        return stream.read(member.compress_size)

def without_presentation(value):
    if isinstance(value,dict):
        return {k:without_presentation(v) for k,v in value.items() if k not in ('props','presentation_detail','contact_track')}
    if isinstance(value,list):return [without_presentation(v) for v in value]
    return value

def verify(original,baked,report):
    bundles=members=added_bytes=0
    for destination in sorted(baked.glob('*.djk_fnemote')):
        source=original/destination.name
        with zipfile.ZipFile(source) as a,zipfile.ZipFile(destination) as b:
            before={m.filename:m for m in a.infolist() if m.filename!='manifest.json' and not m.filename.startswith(('props/','contacts/'))}
            after={m.filename:m for m in b.infolist() if m.filename!='manifest.json' and not m.filename.startswith(('props/','contacts/'))}
            assert before.keys()==after.keys(),destination
            for name,old in before.items():
                new=after[name]
                assert (old.CRC,old.file_size,old.compress_type)==(new.CRC,new.file_size,new.compress_type),(destination,name)
                assert payload(source,old)==payload(destination,new),(destination,name)
                members+=1
            assert without_presentation(json.loads(a.read('manifest.json')))==without_presentation(json.loads(b.read('manifest.json'))),destination
        added_bytes+=destination.stat().st_size-source.stat().st_size
        bundles+=1
    assert bundles>0
    result={'bundles':bundles,'unchanged_compressed_members':members,'added_bytes':added_bytes,'character_audio_and_timeline_identical':True}
    report.write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps(result))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('original',type=pathlib.Path);parser.add_argument('baked',type=pathlib.Path);parser.add_argument('report',type=pathlib.Path)
    args=parser.parse_args();verify(args.original,args.baked,args.report)
