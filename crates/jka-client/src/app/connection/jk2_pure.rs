//! JK2 pure admission owns a separate VFS boundary; JKA pure handling is unchanged.
use crate::app::App;
use jka_assets::{
    jk2_pure::{parse_checksums, Jk2PurePolicy},
    pk3,
};
use jka_protocol::commands::{atoi, info_value};
use std::{collections::BTreeMap, sync::Arc};

impl App {
    pub(in crate::app) fn apply_jk2_pure_assets(
        &mut self,
        configstrings: &BTreeMap<u16, Vec<u8>>,
        feed: u32,
        map: &str,
    ) -> Result<bool, String> {
        let version = self
            .fs_roots
            .active_jk2
            .ok_or("JK2 pure admission without JK2 asset roots")?;
        let info = configstrings.get(&1).map(Vec::as_slice).unwrap_or_default();
        let pure = info_value(info, b"sv_pure").is_some_and(|v| atoi(v) != 0);
        let policy = if pure {
            let checksums = parse_checksums(info_value(info, b"sv_paks").unwrap_or_default())?;
            let dirs =
                pk3::filesystem_search_dirs(&self.base, self.game.as_deref(), &self.fs_roots);
            if self
                .fs_roots
                .jk2_pure
                .as_ref()
                .is_some_and(|p| p.matches(&dirs, version, feed, &checksums))
            {
                return Ok(false);
            }
            Some(Arc::new(Jk2PurePolicy::resolve(
                dirs, version, feed, checksums,
            )?))
        } else {
            if self.fs_roots.jk2_pure.is_none() {
                return Ok(false);
            }
            None
        };
        // Discard speculative work before creating readers for this policy.
        // A same-name BSP must not retain a previous policy's assets.
        self.cancel_vote_map_prewarm();
        self.cancel_live_cgame_prep();
        self.game_session = None;
        self.prepare_worldless_session(map);
        self.local_player_presenter = None;
        self.fs_roots.jk2_pure = policy;
        pk3::configure_filesystem_roots(&self.base, self.fs_roots.clone());
        self.pk3_manager_loaded = false;
        self.reset_asset_catalogs_for_game_change();
        self.invalidate_studio_sources();
        Ok(true)
    }

    pub(in crate::app) fn jk2_pure_systeminfo_matches(&self) -> bool {
        let Some(net) = &self.net else { return true };
        let Some(version) = self.fs_roots.active_jk2 else {
            return true;
        };
        let info = net
            .session()
            .decoder()
            .configstrings
            .get(&1)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !info_value(info, b"sv_pure").is_some_and(|v| atoi(v) != 0) {
            return self.fs_roots.jk2_pure.is_none();
        }
        let Ok(checksums) = parse_checksums(info_value(info, b"sv_paks").unwrap_or_default())
        else {
            return false;
        };
        let dirs = pk3::filesystem_search_dirs(&self.base, self.game.as_deref(), &self.fs_roots);
        self.fs_roots.jk2_pure.as_ref().is_some_and(|p| {
            p.matches(
                &dirs,
                version,
                net.session().decoder().checksum_feed,
                &checksums,
            )
        })
    }
}
