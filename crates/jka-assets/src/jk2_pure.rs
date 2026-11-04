// SPDX-License-Identifier: GPL-2.0-only
//! JK2MV pure package selection and reference checksums, isolated from JKA.
//! Reference: JK2MV 7d601454c3db68492289d4d4e3dc30bff39e4246 files.cpp
//! (FS_PakIsPure, FS_FOpenFileRead, FS_ReferencedPakPureChecksums) and
//! server/sv_client.cpp (SV_VerifyPaks_f). See jka-protocol/src/jk2/LICENSE-JK2MV.

use crate::{pak_checksum, pk3};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurePak {
    pub path: PathBuf,
    pub checksum: i32,
    pub pure_checksum: i32,
}

/// Captured by each VFS index. Old readers never consult the next session's
/// policy, and JKA readers never collect JK2 references.
#[derive(Debug)]
pub struct Jk2PurePolicy {
    pub version: u8,
    pub feed: u32,
    pub directories: Vec<PathBuf>,
    pub server_checksums: Vec<i32>,
    pub packages: Vec<PurePak>,
    referenced: Mutex<HashSet<PathBuf>>,
}

impl PartialEq for Jk2PurePolicy {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
            && self.feed == other.feed
            && self.directories == other.directories
            && self.server_checksums == other.server_checksums
            && self.packages == other.packages
    }
}
impl Eq for Jk2PurePolicy {}

pub fn parse_checksums(text: &[u8]) -> Result<Vec<i32>, String> {
    let text = std::str::from_utf8(text).map_err(|_| "JK2 sv_paks is not ASCII")?;
    let values = text
        .split_ascii_whitespace()
        .map(|v| {
            v.parse::<i32>()
                .map_err(|_| format!("Invalid JK2 sv_paks checksum: {v}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.is_empty() || values.len() > 1024 {
        return Err("JK2 pure server must advertise 1..1024 package checksums".into());
    }
    Ok(values)
}

impl Jk2PurePolicy {
    pub fn matches(&self, dirs: &[PathBuf], version: u8, feed: u32, checksums: &[i32]) -> bool {
        self.directories == dirs
            && self.version == version
            && self.feed == feed
            && self.server_checksums == checksums
    }

    pub fn resolve(
        directories: Vec<PathBuf>,
        version: u8,
        feed: u32,
        server_checksums: Vec<i32>,
    ) -> Result<Self, String> {
        if version != 2 && version != 4 {
            return Err("Unsupported JK2 pure asset version".into());
        }
        let allowed: HashSet<_> = server_checksums.iter().copied().collect();
        let disabled = pk3::disabled_package_paths();
        let mut packages = Vec::new();
        for dir in &directories {
            // The DinurdoJK folder only holds the client's own packages, which a
            // server never lists: skip checksumming them on every connect.
            if pk3::is_package_only_dir(dir) { continue; }
            let entries = match fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", dir.display())),
            };
            let mut paths = entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file() && p.extension().is_some_and(|s| s.eq_ignore_ascii_case("pk3"))
                })
                .filter(|p| !disabled.contains(p) || retail(p))
                .filter(|p| {
                    version != 2
                        || !p.file_name().is_some_and(|n| {
                            n.eq_ignore_ascii_case("assets2.pk3")
                                || n.eq_ignore_ascii_case("assets5.pk3")
                        })
                })
                .collect::<Vec<_>>();
            paths.sort_by_key(|p| {
                std::cmp::Reverse(
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_ascii_lowercase(),
                )
            });
            for path in paths {
                let (checksum, pure_checksum) = pak_checksum::pk3_checksums(&path, feed)
                    .map_err(|e| format!("JK2 pure checksum {}: {e}", path.display()))?;
                if allowed.contains(&checksum) {
                    packages.push(PurePak {
                        path,
                        checksum,
                        pure_checksum,
                    });
                }
            }
        }
        // JK2MV references every pak in a non-base fs_game at mount time.
        let referenced = packages
            .iter()
            .filter(|pak| !in_base(&pak.path))
            .map(|pak| pak.path.clone())
            .collect();
        Ok(Self {
            version,
            feed,
            directories,
            server_checksums,
            packages,
            referenced: Mutex::new(referenced),
        })
    }

    /// Local packages always mount, exactly as on JKA: JKA's sv_pure never
    /// restricts what the client loads locally, it only decides what the client
    /// reports. A package the server did not list (`packages`) is therefore
    /// mounted but never reported; `reply` only reports listed packages.
    pub fn allows_package(&self, _path: &Path) -> bool {
        true
    }

    /// Loose files are unrestricted for the same reason as [`Self::allows_package`].
    pub fn allows_loose(&self, _qpath: &str) -> bool {
        true
    }

    pub(crate) fn record_read(&self, path: &Path, qpath: &str) {
        if retail(path) || !in_base(path) || qpath.to_ascii_lowercase().ends_with(".bsp") {
            self.referenced
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(path.to_path_buf());
        }
    }

    /// Resolve the actual permitted QVM providers without executing a module.
    /// Report general refs in local search order; JK2MV does not reorder the
    /// search path to the server's list. Equivalent duplicate paks count once.
    pub fn reply(
        &self,
        assets: &pk3::AssetSearchPath,
        map_name: &str,
    ) -> Result<PureReply, String> {
        if !assets
            .jk2_pure_policy()
            .is_some_and(|policy| std::ptr::eq(self, policy.as_ref()))
        {
            return Err("JK2 pure reply requires an asset view of the current policy".into());
        }
        // Local packages the server did not list may shadow a qpath (they are
        // mounted but invisible to the server), so the package to report is the
        // highest-priority provider that the server *did* list.
        let provider = |qpath: &str| -> Result<&PurePak, String> {
            assets
                .indexed_providers(qpath)
                .iter()
                .find_map(|info| self.packages.iter().find(|p| info.archive && p.path == info.source))
                .ok_or_else(|| {
                    format!("JK2 pure package missing for {qpath}; install the server's matching PK3")
                })
        };
        let cgame = provider("vm/cgame.qvm")?;
        let ui = provider("vm/ui.qvm")?;
        // Native registration opens the two QVMs even though this client uses
        // its ported CGame. Retail packages gain GENERAL_REF on these reads.
        self.record_read(&cgame.path, "vm/cgame.qvm");
        self.record_read(&ui.path, "vm/ui.qvm");
        let bsp = format!("maps/{}.bsp", map_name.trim_end_matches(".bsp"));
        let map = provider(&bsp)?;
        self.record_read(&map.path, &bsp);
        let referenced = self
            .referenced
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut seen = HashSet::new();
        let general = self
            .packages
            .iter()
            .filter(|pak| referenced.contains(&pak.path) && seen.insert(pak.checksum))
            .collect::<Vec<_>>();
        let encoded = general
            .iter()
            .fold(self.feed, |v, p| v ^ p.pure_checksum as u32)
            ^ general.len() as u32;
        let refs = general
            .iter()
            .map(|pak| pak.pure_checksum.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!(
            "cp {} {} @ {refs} {}",
            cgame.pure_checksum, ui.pure_checksum, encoded as i32
        )
        .into_bytes();
        if command.len() >= 1024 {
            return Err("JK2 pure reply exceeds the native reliable command limit".into());
        }
        Ok(PureReply {
            command,
            reported_paks: general.len(),
            detail: format!(
                "cgame={} ui={} general=[{}]",
                cgame.path.display(),
                ui.path.display(),
                general
                    .iter()
                    .map(|p| p.path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
    }
}

#[derive(Debug)]
pub struct PureReply {
    pub command: Vec<u8>,
    pub reported_paks: usize,
    pub detail: String,
}

fn in_base(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|n| n.eq_ignore_ascii_case("base") || n.eq_ignore_ascii_case(pk3::JK2_BASE_FOLDER))
}
fn retail(path: &Path) -> bool {
    in_base(path)
        && path.file_name().is_some_and(|n| {
            ["assets0.pk3", "assets1.pk3", "assets2.pk3", "assets5.pk3"]
                .iter()
                .any(|name| n.eq_ignore_ascii_case(name))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlisted_local_packages_mount_but_are_never_reported() {
        let policy = Jk2PurePolicy {
            version: 4,
            feed: 0,
            directories: Vec::new(),
            server_checksums: Vec::new(),
            packages: Vec::new(),
            referenced: Mutex::new(HashSet::new()),
        };
        assert!(policy.allows_package(Path::new("/game/DinurdoJK/dinurdo-assets.pk3")));
        assert!(policy.allows_package(Path::new("/game/jk2_base/custom.pk3")));
        assert!(policy.allows_loose("textures/anything.jpg"));
    }
}
