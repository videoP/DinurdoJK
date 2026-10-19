using System.Security.Cryptography;
using System.Text;
using CUE4Parse.Compression;
using CUE4Parse.Encryption.Aes;
using CUE4Parse.FileProvider;
using CUE4Parse.MappingsProvider.Usmap;
using CUE4Parse.UE4.Objects.Core.Misc;
using CUE4Parse.UE4.Versions;
using CUE4Parse.UE4.IO;
using CUE4Parse.UE4.Objects.Engine.VectorField;
using CUE4Parse.UE4.Assets.Exports;
using CUE4Parse.UE4.Assets;
using CUE4Parse.UE4.Assets.Exports.StaticMesh;
using CUE4Parse.UE4.Assets.Exports.SkeletalMesh;
using CUE4Parse.UE4.Assets.Exports.Material;
using CUE4Parse.UE4.Assets.Exports.Texture;
using CUE4Parse_Conversion;
using CUE4Parse_Conversion.Meshes;
using CUE4Parse_Conversion.Dto;
using CUE4Parse_Conversion.Options;
using CUE4Parse_Conversion.Textures;
using CUE4Parse_Conversion.Textures.BC;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using SkiaSharp;
using CUE4Parse.UE4.Assets.Exports.Animation;
using CUE4Parse.UE4.Writers;
using CUE4Parse_Conversion.Animations;
using CUE4Parse_Conversion.Writers.ActorX;

if (args.Length < 4) throw new ArgumentException("<exporter root (folder holding .data/: Oodle DLL, .usmap, aes.json)> <game root> <dependency index.json> <fresh output> [max objects=12000] [max depth=64] [shader maps=true]");
var exporter = Path.GetFullPath(args[0]);
var output = Path.GetFullPath(args[3]);
if (Directory.Exists(output)) throw new ArgumentException("Use a fresh output directory");
var limit = args.Length > 4 ? int.Parse(args[4]) : 12000;
var maxDepth=args.Length>5?int.Parse(args[5]):64;
if(limit<1||limit>250000||maxDepth<0||maxDepth>256)throw new ArgumentException("Invalid dependency limits");
var input = JObject.Parse(await File.ReadAllTextAsync(args[2]));
var project=input["project"] as JObject;
var parserDiagnostics=new ShaderDiagnostics();
CUE4Parse.CUE4ParseLog.UseLogger(new Serilog.LoggerConfiguration().MinimumLevel.Warning().WriteTo.Sink(parserDiagnostics).CreateLogger());
ObjectTypeRegistry.RegisterClass("Material",typeof(CapturedMaterial));
ObjectTypeRegistry.RegisterClass("MaterialInstanceConstant",typeof(CapturedMaterialInstance));
await OodleHelper.InitializeAsync(Path.Combine(exporter, ".data", OodleHelper.OodleFileName));
// BC7/ETC textures use a separate native decoder. Extract it inside the
// exporter output runtime, never into the user's game or Unreal checkout.
var detexPath=Path.Combine(AppContext.BaseDirectory,DetexHelper.DLL_NAME);
if(!await DetexHelper.LoadDllAsync(detexPath))throw new InvalidOperationException("Detex texture decoder is unavailable");
DetexHelper.Initialize(detexPath);
var pakDirectory=project?["pak_directory"]?.Value<string>()??Path.Combine(args[1], "FortniteGame", "Content", "Paks");
// Pin the verified Fortnite shader-map ABI. "UE5 latest" currently uses the
// older map-ID ordering and silently loses this installation's shader maps.
// Other projects must specify their own cooked engine/version profile.
var gameVersion=project==null?"GAME_UE6_0":project["game_version"]?.Value<string>()??throw new ArgumentException("Generic UE projects must specify project.game_version");
if(!Enum.TryParse<EGame>(gameVersion,out var game))throw new ArgumentException("Unknown UE game/version profile");
using var provider = new DefaultFileProvider(pakDirectory, SearchOption.AllDirectories,
    new VersionContainer(game), StringComparer.OrdinalIgnoreCase);
// UStruct otherwise skips serialized Kismet bytecode entirely. Preserve it
// for explicit native behavior ports and capability audits.
provider.ReadScriptData = true;
provider.ReadShaderMaps=args.Length<=6||bool.Parse(args[6]);
var mappings=project?["mappings"]?.Value<string>()??(project==null?Path.Combine(exporter, ".data", "Fortnite-42.30.usmap"):null);
if(mappings!=null)provider.MappingsContainer = new FileUsmapTypeMappingsProvider(mappings);
provider.Initialize(); await provider.MountAsync();
var aesPath=project?["aes_file"]?.Value<string>()??(project==null?Path.Combine(exporter, ".data", "aes.json"):null);
var keys = new Dictionary<FGuid, FAesKey>();
if(aesPath!=null){var aes=JObject.Parse(await File.ReadAllTextAsync(aesPath))["data"]!;
if(aes["mainKey"]?.Value<string>() is string mainKey)keys[new FGuid()] = new FAesKey(mainKey);
foreach (var key in aes["dynamicKeys"]??new JArray()) keys[new FGuid(key["pakGuid"]!.Value<string>()!)] = new FAesKey(key["key"]!.Value<string>()!);}
// Fortnite streams texture top mips and other optional bulk data on demand;
// the local install only keeps small mips (64x64 for most textures). When a
// chunk host is configured, register the installation's .uondemandtoc files so
// CUE4Parse fetches missing chunks, caching them outside the export folder.
var onDemandHost=Environment.GetEnvironmentVariable("FN_ONDEMAND_HOST")??project?["on_demand_host"]?.Value<string>();
if(!string.IsNullOrEmpty(onDemandHost)){
    var cache=Environment.GetEnvironmentVariable("FN_ONDEMAND_CACHE")??Path.Combine(Path.GetDirectoryName(output)!,"fn-ondemand-cache");
    provider.OnDemandOptions=new IoStoreOnDemandOptions{ChunkHostUri=new Uri(onDemandHost,UriKind.Absolute),ChunkCacheDirectory=Directory.CreateDirectory(cache),Timeout=TimeSpan.FromSeconds(120)};
    foreach(var toc in Directory.GetFiles(pakDirectory,"*.uondemandtoc",SearchOption.AllDirectories)){
        await provider.RegisterVfsAsync(new IoChunkToc(toc,provider.Versions));
        Console.WriteLine($"On-demand TOC registered: {Path.GetFileName(toc)}");
    }
    await provider.MountAsync();
}
await provider.SubmitKeysAsync(keys); provider.PostMount(); provider.LoadVirtualPaths();
Directory.CreateDirectory(output);
// {"list": "<path fragment>"}: write the matching package paths (mounted and
// on-demand) to listing.txt instead of exporting, e.g. to find item IDs.
if(input["list"]?.Value<string>() is string listFragment){
    var hits=provider.Files.Keys.Where(k=>k.EndsWith(".uasset",StringComparison.OrdinalIgnoreCase)&&k.Contains(listFragment,StringComparison.OrdinalIgnoreCase)).OrderBy(k=>k,StringComparer.OrdinalIgnoreCase).ToList();
    await File.WriteAllLinesAsync(Path.Combine(output,"listing.txt"),hits);
    Console.WriteLine($"Listed {hits.Count} packages matching {listFragment}");
    return;
}
foreach (var sub in new[]{"Objects", "Meshes", "Textures", "Animations", "Programs"}) Directory.CreateDirectory(Path.Combine(output,sub));
string Id(string path) => Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(path.ToLowerInvariant())))[..24].ToLowerInvariant();
var queue = new Queue<(string Path,int Depth)>();
var queued = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
var records = new JArray();
var materials = new JObject();
var requiredShaderMaps=new HashSet<string>(StringComparer.OrdinalIgnoreCase);
foreach(var hash in input["shader_map_hashes"]??new JArray())if(hash.Value<string>() is string value)requiredShaderMaps.Add(value);
void Enqueue(string path,int depth) {
    // Exporting a package already includes its nested emitters, scripts and
    // data interfaces. A nested object is not another .uasset package.
    var dot=path.IndexOf('.');
    if(dot>=0){var tail=path[(dot+1)..];var nested=tail.IndexOfAny(new[]{':','.'});
        if(nested>=0)path=path[..(dot+1)]+tail[..nested];}
    if (path.StartsWith('/') && !path.StartsWith("/Script/") && queued.Add(path)) queue.Enqueue((path,depth));
}
foreach (var seed in input["seeds"]!) Enqueue(seed["asset"]!.Value<string>()!,0);
IEnumerable<string> Refs(JToken token) {
    if (token is JObject obj) {
        var type=obj["ObjectName"]?.Value<string>()?.Split('\'')[0] ?? "";
        var allowed=new[]{"StaticMesh","SkeletalMesh","Material","MaterialInstanceConstant","MaterialFunction","Texture2D","TextureCube","NiagaraSystem","NiagaraEmitter","NiagaraScript","ParticleSystem","Blueprint","BlueprintGeneratedClass","AnimBlueprintGeneratedClass","AnimMontage","AnimSequence","Skeleton","CurveFloat","CurveVector","CurveLinearColor","VectorFieldStatic"};
        var name=obj["ObjectName"]?.Value<string>(); var path=obj["ObjectPath"]?.Value<string>();
        if (path!=null && name!=null && path.LastIndexOf('.')>0 && (allowed.Contains(type)||type.StartsWith("NiagaraDataInterface")||type.StartsWith("MaterialExpression"))) {
            var pieces=name.Split('\'');if(pieces.Length>=2){var objectName=pieces[1];yield return objectName.StartsWith('/')?objectName:path[..path.LastIndexOf('.')] + "." + objectName;}
        }
        foreach (var p in obj.Properties()) foreach (var r in Refs(p.Value)) yield return r;
    } else if (token is JArray array) foreach (var child in array) foreach(var r in Refs(child)) yield return r;
}
async Task<string> Material(UMaterialInterface material) {
    var path=material.GetPathName(); var id=Id(path);
    if (materials[id]!=null) return id;
    var parameters=new CMaterialParams2(); material.GetParams(parameters, EMaterialDepth.AllLayersNoRef);
    var diffuse=parameters.GetTexturesByRegex(new System.Text.RegularExpressions.Regex(CMaterialParams2.RegexDiffuse,System.Text.RegularExpressions.RegexOptions.IgnoreCase)).FirstOrDefault();
    foreach (var reference in parameters.Textures.Values) if (reference.Load() is UTexture texture) Enqueue(texture.GetPathName(),1);
    var texturePaths=new JObject();
    foreach(var (name,reference) in parameters.Textures) if(reference.Load() is UTexture t) texturePaths[name]=t.GetPathName();
    materials[id]=new JObject { ["asset"]=path,["diffuse"]=diffuse is UTexture2D ? Id(diffuse.GetPathName()):null,
        ["blend"]=parameters.BlendMode.ToString(),["shading_model"]=parameters.ShadingModel.ToString(),
        ["parameters"]=new JObject { ["textures"]=texturePaths,["colors"]=JObject.FromObject(parameters.Colors),["scalars"]=JObject.FromObject(parameters.Scalars),["switches"]=JObject.FromObject(parameters.Switches) },
        ["limitation"]="Base-color approximation; Unreal shader bytecode/material graph is not executed" };
    return id;
}
while (queue.Count>0 && records.Count<limit) {
    var (path,depth)=queue.Dequeue(); var row=new JObject { ["asset"]=path,["id"]=Id(path),["depth"]=depth };
    parserDiagnostics.CurrentAsset=path;
    try {
        var package=await provider.LoadPackageAsync(path[..path.IndexOf('.')]);
        var exports=package.GetExports().ToArray();
        // Nested emitters/components are exports in their parent's package,
        // not top-level package objects. Match the complete object hierarchy.
        var asset=exports.FirstOrDefault(o=>o.GetPathName().Equals(path,StringComparison.OrdinalIgnoreCase));
        asset ??= await provider.LoadPackageObjectAsync(path);
        if (asset==null) throw new InvalidDataException("Missing asset");
        row["type"]=asset.ExportType;
        var json=JArray.FromObject(exports);
        await File.WriteAllTextAsync(Path.Combine(output,"Objects",Id(path)+".json"),json.ToString(Formatting.None));
        if (depth<maxDepth) foreach(var reference in Refs(json)) Enqueue(reference,depth+1);
        // UObject's normal JSON writer omits these CUE4Parse native fields.
        // Preserve the recovered programs explicitly instead of reporting a
        // cooked graph absent merely because its serializer hid it.
        var programs=new JArray();
        var serializer=JsonSerializer.Create(new JsonSerializerSettings{ReferenceLoopHandling=ReferenceLoopHandling.Ignore,MaxDepth=128});
        foreach(var obj in exports){
            var native=new JObject();
            var binaries=new JArray();var mapIndex=0;
            foreach(var map in ShaderMapAccess.Maps(obj)){
                var prefix=Id(obj.GetPathName())+".shader-"+(mapIndex++);
                var hashBytes=game>=EGame.GAME_UE5_8?8:20;
                var hash=map.ResourceHash?.ToString();
                var descriptor=new JObject{["shader_platform"]=map.ShaderPlatform.ToString(),["resource_hash"]=hash!=null?hash[..(hashBytes*2)]:null,["resource_hash_bytes"]=hashBytes};
                if(hash!=null)requiredShaderMaps.Add(hash[..(hashBytes*2)]);
                if(map.FrozenArchive?.FrozenObject is byte[] frozen && frozen.Length>0){
                    var name=prefix+".frozen.bin";await File.WriteAllBytesAsync(Path.Combine(output,"Programs",name),frozen);descriptor["frozen_object"]=name;
                }
                var codes=new JArray();
                if(map.Code!=null){
                    var codeIndex=0;
                    foreach(var shader in map.Code.ShaderCodeResources){
                        var name=prefix+".code-"+(codeIndex++)+".bin";
                        await File.WriteAllBytesAsync(Path.Combine(output,"Programs",name),shader.Code.Data);
                        codes.Add(new JObject{["file"]=name,["header"]=JObject.FromObject(shader.Header)});
                    }
                    foreach(var shader in map.Code.ShaderEntries){
                        var name=prefix+".code-"+(codeIndex++)+".bin";
                        await File.WriteAllBytesAsync(Path.Combine(output,"Programs",name),shader.Code);
                        codes.Add(new JObject{["file"]=name,["frequency"]=shader.Frequency.ToString(),["uncompressed_size"]=shader.UncompressedSize});
                    }
                }
                descriptor["inline_code"]=codes;
                descriptor["shared_shader_library_required"]=map.Code==null&&map.ResourceHash!=null;
                binaries.Add(descriptor);
            }
            if(binaries.Count>0)native["ShaderBinaryResources"]=binaries;
            if(obj is INativeMaterialTail captured && captured.NativeTail.Length>0){
                var tailName=Id(obj.GetPathName())+".material-native.bin";
                await File.WriteAllBytesAsync(Path.Combine(output,"Programs",tailName),captured.NativeTail);
                native["UnparsedNativeTail"]=new JObject{["file"]=tailName,["serial_offset"]=captured.NativeOffset,["bytes"]=captured.NativeTail.Length,["scope"]="Native material tail after ordinary property parsing; retained when inline shader-map parsing fails"};
            }
            foreach(var member in new[]{"LoadedMaterialResources","LoadedScriptResources","CachedExpressionData","Expressions","ReferencedTextures"}){
                var objectType=obj.GetType();
                var value=objectType.GetField(member,System.Reflection.BindingFlags.Public|System.Reflection.BindingFlags.Instance)?.GetValue(obj)
                    ??objectType.GetProperty(member,System.Reflection.BindingFlags.Public|System.Reflection.BindingFlags.Instance)?.GetValue(obj);
                if(value!=null){try{native[member]=JToken.FromObject(value,serializer);}catch(Exception error){native[member]=new JObject{["export_error"]=error.Message};}}
            }
            if(native.Count>0)programs.Add(new JObject{["Type"]=obj.ExportType,["Name"]=obj.Name,["NativeData"]=native});
        }
        if(programs.Count>0)await File.WriteAllTextAsync(Path.Combine(output,"Programs",Id(path)+".json"),programs.ToString(Formatting.None));
        row["program_objects"]=programs.Count;
        if (asset is UStaticMesh mesh) {
            using var converted=new StaticMeshDto(mesh);
            var lod=converted.LODs.First(l=>l.Indices.Length>0); var sections=new JArray();
            foreach(var section in lod.Sections) {
                var material=section.MaterialIndex>=0 && section.MaterialIndex<converted.Materials.Length ? converted.Materials[section.MaterialIndex].Material?.Load() as UMaterialInterface:null;
                sections.Add(new JObject { ["first_index"]=section.FirstIndex,["triangles"]=section.NumFaces,
                    ["material"]=material!=null ? await Material(material):null });
            }
            var vertices=new JArray(lod.Vertices.Select(v=>new JArray(v.Position.X,v.Position.Y,v.Position.Z,v.Normal.X,v.Normal.Y,v.Normal.Z,v.Uv.U,v.Uv.V)));
            var data=new JObject { ["asset"]=path,["vertices"]=vertices,["indices"]=JArray.FromObject(lod.Indices),["sections"]=sections,
                ["vertex_colors"]=lod.VertexColors!=null?JArray.FromObject(lod.VertexColors):null,
                ["extra_uvs"]=JArray.FromObject(lod.ExtraUvs) };
            await File.WriteAllTextAsync(Path.Combine(output,"Meshes",Id(path)+".json"),data.ToString(Formatting.None));
            row["status"]="static_mesh";
        } else if (asset is USkeletalMesh skeletal) {
            using var converted=new SkeletalMeshDto(skeletal,exportMorphTarget:false);
            var lod=converted.LODs.First(l=>l.Indices.Length>0);
            var sections=new JArray();
            foreach(var section in lod.Sections){var material=section.MaterialIndex>=0 && section.MaterialIndex<converted.Materials.Length ? converted.Materials[section.MaterialIndex].Material?.Load() as UMaterialInterface:null;
                sections.Add(new JObject{["first_index"]=section.FirstIndex,["triangles"]=section.NumFaces,["material"]=material!=null ? await Material(material):null});}
            var data=new JObject { ["asset"]=path,["vertices"]=JArray.FromObject(lod.Vertices),["indices"]=JArray.FromObject(lod.Indices),["sections"]=sections,["bones"]=JArray.FromObject(converted.Bones),
                ["vertex_colors"]=lod.VertexColors!=null?JArray.FromObject(lod.VertexColors):null,
                ["extra_uvs"]=JArray.FromObject(lod.ExtraUvs) };
            await File.WriteAllTextAsync(Path.Combine(output,"Meshes",Id(path)+".skeletal.json"),data.ToString(Formatting.None));
            row["status"]="skeletal_authoring_only";
            row["limitation"]="Vertices, weights and skeleton exported; independent prop animation requires runtime conversion";
        } else if (asset is UAnimSequence sequence) {
            var animSet=sequence.ConvertAnims();
            if(animSet.Sequences.Count!=1)throw new InvalidDataException("Expected one prop animation");
            using var writer=new FArchiveWriter();new ActorXAnim(animSet,0).Save(writer);
            await File.WriteAllBytesAsync(Path.Combine(output,"Animations",Id(path)+".psa"),writer.GetBuffer());row["status"]="prop_animation";
        } else if (asset is UTexture2D texture) {
            var bitmap=texture.Decode(1024) ?? throw new InvalidDataException("Texture mip unavailable");
            var data=bitmap.Encode(new ExportOptions(exportHdrTexturesAsHdr:false),out var extension);
            if (extension!="png") throw new InvalidDataException("Expected PNG texture export");
            await File.WriteAllBytesAsync(Path.Combine(output,"Textures",Id(path)+".png"),data);
            row["status"]="texture"; row["width"]=bitmap.Width;row["height"]=bitmap.Height;
        } else if (asset is UVectorFieldStatic field) {
            // Raw FFloat16Color volume (SizeX*SizeY*SizeZ texels) for CPU sampling;
            // sizes, bounds and intensity are in the object JSON.
            var bytes=field.SourceData?.Data ?? throw new InvalidDataException("Vector field has no source data");
            await File.WriteAllBytesAsync(Path.Combine(output,"Objects",Id(path)+".vectorfield.bin"),bytes);
            row["status"]="vector_field"; row["bytes"]=bytes.Length;
        } else if (asset is UMaterialInterface material) {
            await Material(material); row["status"]="material_approximation";
        } else {
            row["status"]="metadata_only";
            row["limitation"]="Unreal effect/Blueprint behavior requires a DinurdoJK implementation";
        }
    } catch(Exception ex) { row["status"]="failed";row["error"]=ex.Message; }
    records.Add(row);
    if (records.Count%100==0) Console.WriteLine($"Exported {records.Count}, pending {queue.Count}");
}
await File.WriteAllTextAsync(Path.Combine(output,"materials.json"),materials.ToString(Formatting.Indented));
// Full inherited material parameters can be enormous. The native importer
// needs only compact references; preserve complete authoring data separately.
var nativeMaterials=new JObject();
foreach(var property in materials.Properties()) {
    var m=property.Value;
    nativeMaterials[property.Name]=new JObject { ["asset"]=m["asset"],["diffuse"]=m["diffuse"],["blend"]=m["blend"] };
}
await File.WriteAllTextAsync(Path.Combine(output,"native-materials.json"),nativeMaterials.ToString(Formatting.Indented));
await File.WriteAllTextAsync(Path.Combine(output,"dependencies.json"),input.ToString(Formatting.Indented));
var report=new JObject { ["version"]=1,["mounted_archives"]=provider.MountedVfs.Count,["unavailable_archives"]=provider.UnloadedVfs.Count,
    ["remaining_queue"]=queue.Count,["depth_limit"]=maxDepth,["shader_maps_enabled"]=provider.ReadShaderMaps,["script_data_enabled"]=true,["game_version"]=gameVersion,["texture_max_mip"]=1024,["assets"]=records };
await File.WriteAllTextAsync(Path.Combine(output,"report.json"),report.ToString(Formatting.Indented));
Console.WriteLine($"Finished {records.Count} assets; {queue.Count} beyond limit; {materials.Count} material approximations");
await File.WriteAllTextAsync(Path.Combine(output,"shader-parser-diagnostics.json"),parserDiagnostics.Events.ToString(Formatting.Indented));
if(project?["export_shader_libraries"]?.Value<bool>()??true){
    await ShaderLibraryExport.Run(provider,output,requiredShaderMaps,game>=EGame.GAME_UE5_8?8:20);
}

// The parser otherwise swallows inline shader-map failures. Keep that
// distinction visible: a failed binary parse is not proof a graph was stripped.
sealed class ShaderDiagnostics : Serilog.Core.ILogEventSink {
    public readonly JArray Events=new();
    public string? CurrentAsset;
    public void Emit(Serilog.Events.LogEvent entry){
        if(!entry.MessageTemplate.Text.Contains("shader",StringComparison.OrdinalIgnoreCase))return;
        lock(Events){if(Events.Count<10000)Events.Add(new JObject{["asset_context"]=CurrentAsset,["message"]=entry.RenderMessage(),["error_type"]=entry.Exception?.GetType().Name,["error"]=entry.Exception?.Message,["stack"]=entry.Exception?.StackTrace});}
    }
}
interface INativeMaterialTail {byte[] NativeTail{get;}long NativeOffset{get;}}
sealed class CapturedMaterial : UMaterial,INativeMaterialTail {
    public byte[] NativeTail{get;private set;}=Array.Empty<byte>();public long NativeOffset{get;private set;}
    public override void Deserialize(CUE4Parse.UE4.Assets.Readers.FAssetArchive archive,long validPos){
        base.Deserialize(archive,validPos);
        if(LoadedMaterialResources.Count==0&&archive.Position<validPos&&validPos-archive.Position>32&&validPos-archive.Position<64*1024*1024){
            NativeOffset=archive.Position;var copy=(CUE4Parse.UE4.Readers.FArchive)archive.Clone();NativeTail=copy.ReadBytes(checked((int)(validPos-archive.Position)));
        }
    }
}
sealed class CapturedMaterialInstance : UMaterialInstanceConstant,INativeMaterialTail {
    public byte[] NativeTail{get;private set;}=Array.Empty<byte>();public long NativeOffset{get;private set;}
    public override void Deserialize(CUE4Parse.UE4.Assets.Readers.FAssetArchive archive,long validPos){
        base.Deserialize(archive,validPos);
        if(LoadedMaterialResources.Count==0&&archive.Position<validPos&&validPos-archive.Position>32&&validPos-archive.Position<64*1024*1024){
            NativeOffset=archive.Position;var copy=(CUE4Parse.UE4.Readers.FArchive)archive.Clone();NativeTail=copy.ReadBytes(checked((int)(validPos-archive.Position)));
        }
    }
}
static class ShaderMapAccess {
    public static IEnumerable<FShaderMapBase> Maps(UObject obj){
        if(obj is UMaterialInterface material){foreach(var resource in material.LoadedMaterialResources)if(resource.LoadedShaderMap!=null)yield return resource.LoadedShaderMap;}
        var scripts=obj.GetType().GetField("LoadedScriptResources")?.GetValue(obj) as System.Collections.IEnumerable;
        if(scripts!=null)foreach(var script in scripts){
            if(script?.GetType().GetField("RenderingThreadShaderMap")?.GetValue(script) is FShaderMapBase map)yield return map;
        }
    }
}
