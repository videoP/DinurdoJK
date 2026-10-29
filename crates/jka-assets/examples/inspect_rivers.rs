//! Read map metadata without loading/rendering a game or accepting old bakes.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: inspect_rivers <map.bsp>")?;
    let bsp = jka_assets::bsp::Bsp::parse(&std::fs::read(path)?)?;
    let rivers = bsp.rivers();
    for river in &rivers.rivers {
        println!(
            "river {:?}: model *{}, bounds {:?}..{:?}, cell {} JKA units",
            river.name,
            river.volume.model,
            river.volume.mins,
            river.volume.maxs,
            river.settings.cell_size
        );
        println!(
            "  authored JAPRO friction {}, capture headroom {}",
            river.settings.japro_friction, river.settings.capture_headroom
        );
        for source in &river.sources {
            println!("  source: {source:?}");
        }
        for sink in &river.sinks {
            println!("  sink: {sink:?}");
        }
    }
    for diagnostic in &rivers.diagnostics {
        eprintln!("entity {}: {}", diagnostic.entity_index, diagnostic.message);
    }
    if !rivers.diagnostics.is_empty() {
        return Err("invalid river metadata".into());
    }
    Ok(())
}
