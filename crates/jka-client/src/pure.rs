//! Jedi Academy sv_pure checksum negotiation.
//!
//! The base-game bypass intentionally mirrors TaystJK's `FS_ReferencedPakPureChecksums`
//! behaviour: report only stock base assets to a pure server even though the local
//! filesystem remains free to mount additional client-side PK3s.

use std::path::{Path, PathBuf};

const STOCK_PAKS_DESCENDING: [&str; 4] = [
    "assets3.pk3",
    "assets2.pk3",
    "assets1.pk3",
    "assets0.pk3",
];

// TaystJK uses these ordinary checksums as sentinels for the normal JKA path.
const ASSETS3_CHECKSUM: i32 = -1_342_311_474;
const ASSETS0_CHECKSUM: i32 = 1_767_559_464;

#[derive(Debug)]
pub struct BasePureBypass {
    pub command: Vec<u8>,
    pub reported_paks: usize,
}

/// Build the normal JKA `cp` reliable command while limiting the reported
/// referenced packages to the four retail base archives.
///
/// TaystJK starts its bypass walk at assets3 (which it marks as cgame/ui/general)
/// and prevents higher-priority custom PK3s from entering the response. This
/// engine does not maintain OpenJK's per-pack reference flags yet, so reporting
/// all four stock archives is the deterministic equivalent: every checksum is
/// server-approved base content, while local/custom packages are omitted.
pub fn build_basejka_bypass_command(
    base_dir: &Path,
    server_id: i32,
    checksum_feed: u32,
) -> Result<BasePureBypass, String> {
    let mut regular = Vec::with_capacity(STOCK_PAKS_DESCENDING.len());
    let mut pure = Vec::with_capacity(STOCK_PAKS_DESCENDING.len());

    for name in STOCK_PAKS_DESCENDING {
        let path = find_case_insensitive(base_dir, name)
            .ok_or_else(|| format!("required stock PK3 is missing: {name}"))?;
        let (regular_checksum, pure_checksum) =
            jka_assets::pak_checksum::pk3_checksums(&path, checksum_feed)
                .map_err(|error| format!("failed to checksum {}: {error}", path.display()))?;
        regular.push(regular_checksum);
        pure.push(pure_checksum);
    }

    // Refuse to call a modified/repacked install the retail bypass. Apart from
    // matching TaystJK's sentinels, this keeps an accidental replacement of a
    // core assets archive from being silently hidden from the server.
    if regular[0] != ASSETS3_CHECKSUM {
        return Err(format!(
            "assets3.pk3 checksum is {}, expected retail JKA {}",
            regular[0], ASSETS3_CHECKSUM
        ));
    }
    if regular[3] != ASSETS0_CHECKSUM {
        return Err(format!(
            "assets0.pk3 checksum is {}, expected retail JKA {}",
            regular[3], ASSETS0_CHECKSUM
        ));
    }

    // JKA pure response format:
    //   cgame ui @ general-ref... encoded-checksum
    // assets3 is the stock package TaystJK marks with all three reference bits.
    let cgame = pure[0];
    let ui = pure[0];
    let mut encoded = checksum_feed;
    for checksum in &pure {
        encoded ^= *checksum as u32;
    }
    encoded ^= pure.len() as u32;

    let refs = pure
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!(
        "cp {server_id} {cgame} {ui} @ {refs} {}",
        encoded as i32
    );

    Ok(BasePureBypass {
        command: text.into_bytes(),
        reported_paks: pure.len(),
    })
}

fn find_case_insensitive(directory: &Path, wanted: &str) -> Option<PathBuf> {
    let direct = directory.join(wanted);
    if direct.is_file() {
        return Some(direct);
    }

    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(wanted))
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_pak_order_matches_taystjk_walk() {
        assert_eq!(
            STOCK_PAKS_DESCENDING,
            ["assets3.pk3", "assets2.pk3", "assets1.pk3", "assets0.pk3"]
        );
    }
}
