# Baked particle effects and native props

Emotes often spawn Niagara particle systems and prop meshes. The game does not
run Unreal's VM at playback: the compiler **bakes** them.

```text
UE export ──djk-fnemote import-assets──▶ native.pk3 (+ native.report.json)   props, textures, material ports
UE export ──effect_bake (CPU Niagara host from jka-ue-runtime)──▶ effects/*.fxt   particle tracks per emote
```

* `import-assets <export> <fresh.pk3> [scale]` is `djk-ue-import` with the emote
  runtime's namespace (`models/fnemote/`, `Emote_Prop_` stripped from display
  names). For any other project use `djk-ue-import --namespace <folder>`; see
  [jka-ue-assets](../jka-ue-assets/README.md) and its export layout.
* `effect_bake::bake_variant` simulates every effect of one animation variant on
  the composed source animation (a `PsaEnvironment` supplies bones and sockets
  to skeletal-mesh data interfaces) and records the renderer-bound particle
  attributes in the target model's space. It is the library entry point other
  drivers can use; the Fortnite item/notify driver is part of the library
  tooling follow-up.
* `effect_materials` ports particle materials to native unlit shaders
  (`shaders/fnemote_fx.shader`): vertex colour and alpha, authored blend mode,
  a shape/mask/flipbook texture chosen by parameter role, or a soft disc.

## Track format (`effects/*.fxt`)

Magic `DJKX`, version 2. Samples are recorded at 15 Hz; playback interpolates
particles by ID between samples. Sprites, meshes, ribbons and lights are stored
in model space with positions quantised to 1/8 game unit. At most 256 renderers,
10 minutes and 4 M records per track; the reader validates all of it.
`effects::Writer` and `effects::Track` are the writer and reader.

## Checking Niagara systems

```sh
# which exported systems run on the CPU host, with their particle counts and errors
djk-fnemote niagara-audit my-export niagara-audit.json 3 Sparkle
# render one system to a PNG to see it
djk-fnemote niagara-preview my-export Sparkle sparkle.png 3
# overlay a compiled bundle's baked effects on its pose
djk-fnemote effect-preview base/emotes/EID_Dance.djk_fnemote 0 fx-native effects.png
# which Blueprint / VectorVM programs decode (decoding is not execution)
djk-fnemote audit-ue-programs my-export programs.json
```

## What is and is not reproduced

CPU Niagara systems run with Unreal's scheduling and VectorVM semantics
(`jka-ue-runtime`). GPU compute emitters carry no VM code; they are shown through
a labelled approximation (real spawn counts, curves and bounds; heuristic
lifetime and velocity) and flagged as such. Vector fields return zero force
unless their asset is exported. Material graphs are ported by parameters, not
executed. The client switch `cg_emoteEffects 0` turns playback of all of this off.
