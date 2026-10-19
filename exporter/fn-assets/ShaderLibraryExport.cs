using CUE4Parse.Compression;
using CUE4Parse.FileProvider;
using CUE4Parse.UE4.IO;
using CUE4Parse.UE4.IO.Objects;
using CUE4Parse.UE4.Readers;
using CUE4Parse.UE4.Shaders;
using Newtonsoft.Json.Linq;
using Newtonsoft.Json;

// Resolve only maps referenced by exported assets. Code stays in authoring
// output until a runtime binds its RHI resources and shader permutations.
static class ShaderLibraryExport {
    public static async Task Run(DefaultFileProvider provider,string output,HashSet<string> needed,int hashBytes){
        var directory=Path.Combine(output,"Shaders");Directory.CreateDirectory(directory);
        var readers=provider.MountedVfs.OfType<IoStoreReader>().ToArray();
        var maps=new JObject();var codes=new JObject();var errors=new JArray();var libraries=0;
        string Hash(CUE4Parse.UE4.Objects.Core.Misc.FSHAHash hash)=>hash.ToString()[..(hashBytes*2)].ToLowerInvariant();
        byte[] Chunk(IoStoreReader preferred,FIoChunkId id){
            if(preferred.TryResolve(id,out _))return preferred.Read(id);
            foreach(var reader in readers)if(reader.TryResolve(id,out _))return reader.Read(id);
            throw new InvalidDataException("Referenced shader group chunk is unavailable: "+id);
        }
        foreach(var reader in readers){
            foreach(var chunk in reader.TocResource.ChunkIds.Where(c=>c.ChunkType==(byte)EIoChunkType5.ShaderCodeLibrary)){
                try{
                    using var archive=new FByteArchive("ShaderLibrary",Chunk(reader,chunk),reader.Versions);
                    var library=new FShaderCodeArchive(archive);
                    if(library.SerializedShaders is not FIoStoreShaderCodeArchive header){
                        errors.Add(new JObject{["library"]=chunk.ToString(),["error"]="Non-IoStore shader library requires a separate decoder"});continue;
                    }
                    libraries++;
                    var selected=new HashSet<uint>();
                    for(var mi=0;mi<header.ShaderMapHashes.Length;mi++){
                        var hash=Hash(header.ShaderMapHashes[mi]);if(!needed.Contains(hash))continue;
                        var entry=header.ShaderMapEntries[mi];
                        if(entry.NumShaders>100000||entry.ShaderIndicesOffset>header.ShaderIndices.Length||entry.NumShaders>header.ShaderIndices.Length-entry.ShaderIndicesOffset)throw new InvalidDataException("Shader map index range exceeds library");
                        var ids=new JArray();
                        for(uint i=0;i<entry.NumShaders;i++){
                            var index=header.ShaderIndices[entry.ShaderIndicesOffset+i];
                            if(index>=header.ShaderEntries.Length||index>=header.ShaderHashes.Length)throw new InvalidDataException("Shader index exceeds library");
                            selected.Add(index);ids.Add(Hash(header.ShaderHashes[index]));
                        }
                        maps[hash]=new JObject{["library_chunk"]=chunk.ToString(),["shaders"]=ids};
                    }
                    foreach(var grouping in selected.GroupBy(i=>header.ShaderEntries[i].ShaderGroupIndex)){
                        try{
                            var gi=grouping.Key;
                            if(gi>=header.ShaderGroupEntries.Length||gi>=header.ShaderGroupIoHashes.Length)throw new InvalidDataException("Shader group exceeds library");
                            var group=header.ShaderGroupEntries[gi];
                            if(group.UncompressedSize>256*1024*1024||group.CompressedSize>256*1024*1024)throw new InvalidDataException("Shader group exceeds byte limit");
                            var compressed=Chunk(reader,header.ShaderGroupIoHashes[gi]);
                            if(compressed.Length!=group.CompressedSize)throw new InvalidDataException("Shader group compressed size mismatch");
                            var data=compressed;
                            if(group.CompressedSize!=group.UncompressedSize){data=new byte[group.UncompressedSize];OodleHelper.Decompress(compressed,0,compressed.Length,data,0,data.Length);}
                            if(group.NumShaders>100000||group.ShaderIndicesOffset>header.ShaderIndices.Length||group.NumShaders>header.ShaderIndices.Length-group.ShaderIndicesOffset)throw new InvalidDataException("Shader group index range exceeds library");
                            var offsets=new SortedSet<uint>{group.UncompressedSize};
                            for(uint i=0;i<group.NumShaders;i++){
                                var index=header.ShaderIndices[group.ShaderIndicesOffset+i];if(index>=header.ShaderEntries.Length)throw new InvalidDataException("Shader group references an invalid shader");
                                var info=header.ShaderEntries[index];if(info.ShaderGroupIndex!=gi)throw new InvalidDataException("Shader points to another group");offsets.Add(info.UncompressedOffsetInGroup);
                            }
                            foreach(var index in grouping){
                                var entry=header.ShaderEntries[index];var hash=Hash(header.ShaderHashes[index]);
                                var start=entry.UncompressedOffsetInGroup;var end=offsets.FirstOrDefault(n=>n>start);
                                if(start>=end||end>data.Length)throw new InvalidDataException("Shader code range exceeds group");
                                var bytes=data.AsSpan((int)start,(int)(end-start)).ToArray();
                                var filename=hash+".bin";var path=Path.Combine(directory,filename);
                                if(File.Exists(path)){
                                    var existing=await File.ReadAllBytesAsync(path);
                                    if(!bytes.SequenceEqual(existing))throw new InvalidDataException("Shader hash collision has different code");
                                }else await File.WriteAllBytesAsync(path,bytes);
                                // Enum value 5 also aliases NumGraphicsFrequencies;
                                // reflection's ToString can hide compute modules.
                                var frequency=(int)entry.Frequency;
                                var name=frequency==5?"SF_Compute":entry.Frequency.ToString();
                                codes[hash]=new JObject{["file"]=filename,["bytes"]=bytes.Length,["frequency"]=name,["frequency_code"]=frequency,["group_chunk"]=header.ShaderGroupIoHashes[gi].ToString(),["magic_hex"]=Convert.ToHexString(bytes.AsSpan(0,Math.Min(8,bytes.Length))).ToLowerInvariant()};
                            }
                        }catch(Exception error){errors.Add(new JObject{["library"]=chunk.ToString(),["group"]=grouping.Key,["error"]=error.Message});}
                    }
                }catch(Exception error){errors.Add(new JObject{["library"]=chunk.ToString(),["error"]=error.Message});}
            }
        }
        var missing=new JArray(needed.Where(h=>maps[h.ToLowerInvariant()]==null).Order());
        var report=new JObject{["version"]=1,["libraries"]=libraries,["required_maps"]=needed.Count,["maps"]=maps,["shader_code"]=codes,["missing_maps"]=missing,["errors"]=errors,["runtime_execution_supported"]=false,["limitation"]="Native GPU modules require RHI buffer, texture, uniform, permutation and pipeline bindings; a module export is not a DinurdoJK renderer port"};
        await File.WriteAllTextAsync(Path.Combine(output,"shader-libraries.json"),report.ToString(Formatting.Indented));
        Console.WriteLine($"Shader libraries: {maps.Count}/{needed.Count} maps, {codes.Count} modules, {errors.Count} diagnostics");
    }
}
