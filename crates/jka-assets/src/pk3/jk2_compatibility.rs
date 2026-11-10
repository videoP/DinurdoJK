//! JK2MV files.cpp: FS_LoadZipFile compatibility and FS_FOpenFileRead rules.
//! Keep these in the VFS so diagnostics and asset reads see the same providers.

use std::path::Path;

const V102: u8 = 1;
const V103: u8 = 2;
const V104: u8 = 4;
const ALL: u8 = V102 | V103 | V104;

pub(super) struct PackageCompatibility {
    basename: String,
    compatible: bool,
    known: bool,
    retail: bool,
}

impl PackageCompatibility {
    /// `mv.info` is limited to the first 127 bytes, as in FS_LoadZipFile.
    pub(super) fn new(path: &Path, version: u8, info: &[u8]) -> Self {
        let basename = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        let mut versions = if basename.starts_with("o102_") {
            V102
        } else if basename.starts_with("o103_") {
            V103
        } else if basename.starts_with("o104_") {
            V104
        } else {
            0
        };
        if !info.is_empty() {
            // A present, nonempty mv.info replaces the filename declaration.
            let info = &info[..info.len().min(127)];
            let end = info.iter().position(|&b| b == 0).unwrap_or(info.len());
            let info = String::from_utf8_lossy(&info[..end]).to_ascii_lowercase();
            versions = 0;
            for (label, flag) in [
                ("compatible 1.02", V102),
                ("compatible 1.03", V103),
                ("compatible 1.04", V104),
                ("compatible all", ALL),
            ] {
                if info.contains(label) {
                    versions |= flag;
                }
            }
        }
        // These are JK2MV's native retail version declarations, applied last.
        versions = match basename.as_str() {
            "assets0" | "assets1" => ALL,
            "assets2" => V103 | V104,
            "assets5" => V104,
            _ => versions,
        };
        let in_base = path
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.eq_ignore_ascii_case("base"));
        // FS_idPak: base/assets0..8 and base/assetsmv.
        let retail = in_base
            && (basename == "assetsmv"
                || basename.strip_prefix("assets").is_some_and(|suffix| {
                    suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'0'..=b'8')
                }));
        let selected = match version {
            2 => V102,
            3 => V103,
            4 => V104,
            _ => 0,
        };
        Self {
            compatible: basename.starts_with("dl_")
                || versions & selected != 0
                || (!in_base && versions == 0),
            basename,
            known: versions != 0,
            retail,
        }
    }

    /// The qpath is already normalized by the VFS.
    pub(super) fn allows(&self, qpath: &str) -> bool {
        if !self.compatible {
            // Incompatible retail paks still supply artwork. Unknown base
            // packages also supply artwork, but neither may supply QVMs.
            if matches!(qpath, "vm/cgame.qvm" | "vm/ui.qvm" | "vm/jk2mpgame.qvm")
                || (self.known && !self.retail)
            {
                return false;
            }
        }
        // JK2MV only accepts its native menu patches from assetsmv.
        !qpath.ends_with(".menu_patch") || self.basename == "assetsmv"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_patch_art_and_game_modules_have_different_version_rules() {
        for (version, patch, module_allowed) in [
            (2, "assets2", false),
            (2, "assets5", false),
            (3, "assets2", true),
            (3, "assets5", false),
            (4, "assets5", true),
        ] {
            let path = format!("/game/base/{patch}.pk3");
            let policy = PackageCompatibility::new(Path::new(&path), version, b"");
            assert!(policy.allows("gfx/mp/chat_icon.tga"));
            assert_eq!(policy.allows("vm/cgame.qvm"), module_allowed);
        }
    }

    #[test]
    fn package_declarations_and_downloads_follow_jk2mv_precedence() {
        let cases: &[(&str, &[u8], bool, bool)] = &[
            ("/game/base/o104_test.pk3", b"", false, false),
            ("/game/base/o104_test.pk3", b"COMPATIBLE 1.02", true, true),
            ("/game/base/o102_test.pk3", b"compatible 1.04", false, false),
            ("/game/base/o104_test.pk3", b"compatible all", true, true),
            ("/game/base/dl_test.pk3", b"compatible 1.04", true, true),
            ("/game/base/custom.pk3", b"", true, false),
            ("/game/mod/custom.pk3", b"", true, true),
            ("/game/mod/o104_test.pk3", b"", false, false),
            ("/game/base/assets2.pk3", b"compatible all", true, false),
            ("/game/mod/assets2.pk3", b"", false, false),
            ("/game/base/o102_test.pk3", b"unrecognized", true, false),
        ];
        for &(path, info, art, qvm) in cases {
            let policy = PackageCompatibility::new(Path::new(path), 2, info);
            assert_eq!(policy.allows("textures/test.tga"), art, "{path}");
            assert_eq!(policy.allows("vm/ui.qvm"), qvm, "{path}");
        }
    }

    #[test]
    fn metadata_has_native_length_limit_and_menu_patches_have_native_owner() {
        let mut info = vec![b' '; 127];
        info.extend_from_slice(b"compatible 1.02");
        let policy = PackageCompatibility::new(Path::new("/game/base/o104_test.pk3"), 2, &info);
        assert!(policy.allows("textures/test.tga"));
        assert!(!policy.allows("vm/ui.qvm"));
        for (name, allowed) in [("assetsmv", true), ("custom", false)] {
            let path = format!("/game/mod/{name}.pk3");
            assert_eq!(
                PackageCompatibility::new(Path::new(&path), 2, b"").allows("ui/test.menu_patch"),
                allowed
            );
        }
    }
}
