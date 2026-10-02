//! Jedi Academy sv_pure checksum negotiation.
//!
//! Port of TaystJK's `CL_SendPureChecksums` + `FS_ReferencedPakPureChecksums`
//! (codemp/client/cl_main.cpp, codemp/qcommon/files.cpp) for protocol 26, which is
//! the only protocol this client speaks (`protocolswitch != 2`).
//!
//! The reply is `cp <cgame> <ui> @ <general refs...> <encoded>` with no server id:
//! `SV_VerifyPaks_f` reads the cgame checksum from argument 1.
//!
//! TaystJK marks assets3.pk3 as cgame|ui|general, then walks the search path from
//! assets3 downwards reporting only paks that carry the general reference bit.
//! In this client that bit is set when the map's `.bsp` was loaded from that pak
//! (`FS_FOpenFileRead`) or when the pak lives outside `base`. Everything with
//! higher priority than assets3 (custom/downloaded PK3s) is never reported.

use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
};

// FS_ReferencedPakPureChecksums: `lastPack = -1342311474; //assets3.pk3`.
const ASSETS3_CHECKSUM: i32 = -1_342_311_474;

#[derive(Debug)]
pub struct PureReply {
    pub command: Vec<u8>,
    pub reported_paks: usize,
    /// What was reported and why, for the log.
    pub detail: String,
}

/// Build the `cp` reliable command TaystJK would send for `map_name`.
pub fn build_pure_command(
    base_dir: &Path,
    game_dir: Option<&Path>,
    checksum_feed: u32,
    map_name: &str,
) -> Result<PureReply, String> {
    let order = pk3_search_order(base_dir);
    let file_name = |path: &Path| path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();

    let assets3_index = order
        .iter()
        .position(|path| file_name(path).eq_ignore_ascii_case("assets3.pk3"))
        .ok_or_else(|| "required stock PK3 is missing: assets3.pk3".to_owned())?;
    let checksums_of = |path: &Path| {
        jka_assets::pak_checksum::pk3_checksums(path, checksum_feed)
            .map_err(|error| format!("failed to checksum {}: {error}", path.display()))
    };
    let (assets3_regular, assets3_pure) = checksums_of(&order[assets3_index])?;
    if assets3_regular != ASSETS3_CHECKSUM {
        return Err(format!(
            "assets3.pk3 checksum is {assets3_regular}, expected retail JKA {ASSETS3_CHECKSUM}"
        ));
    }

    // referenced = 7: cgame, ui and general. The walk starts here, so these are
    // the first two checksums and the first general reference.
    let cgame = assets3_pure;
    let ui = assets3_pure;
    let mut general = vec![(file_name(&order[assets3_index]), assets3_pure)];

    // FS_FOpenFileRead marks FS_GENERAL_REF on the pak a `.bsp` is read from (the
    // first pak in search order holding it; fs_game paks outrank base). Only paks
    // below assets3 can be reached by the walk.
    let bsp = format!("maps/{}.bsp", map_name.trim_end_matches(".bsp"));
    let has_bsp = |path: &Path| jka_assets::pak_checksum::pk3_has_entry(path, &[bsp.as_str()]).unwrap_or(false);
    let bsp_in_game_dir = game_dir
        .filter(|game| *game != base_dir)
        .is_some_and(|game| pk3_search_order(game).iter().any(|path| has_bsp(path)));
    let mut bsp_holder = None;
    if !bsp_in_game_dir {
        if let Some(index) = order.iter().position(|path| has_bsp(path)) {
            let name = file_name(&order[index]);
            // `noDL*` paks are noref: they never carry a reference.
            let noref = name.len() >= 4 && name[..4].eq_ignore_ascii_case("nodl");
            bsp_holder = Some(name.clone());
            if index > assets3_index && !noref {
                // The walk's `checksum == 1767559464` (assets0) break ends it after
                // this pak at the latest; only one pak holds the bsp, so nothing
                // further can be added either way.
                let (_, pure) = checksums_of(&order[index])?;
                general.push((name, pure));
            }
        }
    }

    // checksum = fs_checksumFeed ^ (general pure checksums) ^ numPaks
    let mut encoded = checksum_feed;
    for (_, pure) in &general {
        encoded ^= *pure as u32;
    }
    encoded ^= general.len() as u32;

    let refs = general.iter().map(|(_, pure)| pure.to_string()).collect::<Vec<_>>().join(" ");
    let text = format!("cp {cgame} {ui} @ {refs} {}", encoded as i32);

    let detail = format!(
        "cgame/ui=assets3.pk3 general=[{}] map_bsp={bsp} bsp_pak={}",
        general.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", "),
        if bsp_in_game_dir { "fs_game".to_owned() } else { bsp_holder.unwrap_or_else(|| "none".to_owned()) },
    );
    Ok(PureReply {
        command: text.into_bytes(),
        reported_paks: general.len(),
        detail,
    })
}

/// PK3s of one game directory in OpenJK search order: sorted case-insensitively,
/// later names first (a later pak overrides an earlier one).
fn pk3_search_order(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut paks: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("pk3"))
        })
        .collect();
    paks.sort_by_key(|path| Reverse(path.file_name().map(|name| name.to_string_lossy().to_ascii_lowercase())));
    paks
}
