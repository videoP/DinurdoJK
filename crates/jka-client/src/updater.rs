//! GitHub Releases updater. One check at launch, downloads only on a user click.
//! Release AUTHORIZATION is independent of GitHub: fail closed without a
//! compiled-in Ed25519 verifying key and a valid signature on exact manifest bytes.
use std::{collections::BTreeMap, fs, fs::File, io::{Read, Write}, path::{Path, PathBuf}};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/videop/DinurdoJK/releases/latest";
const MAX_ARCHIVE: u64 = 1024 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 16 * 1024;
const MANIFEST_NAME: &str = "release-manifest.json";
const SIGNATURE_NAME: &str = "release-manifest.sig";
// The build must supply only the PUBLIC key. Never ship or embed the private key.
// No key => update checks fail closed if/when a new release is published.
const PUBLIC_KEY_HEX: Option<&str> = option_env!("DINURDOJK_UPDATE_PUBLIC_KEY_HEX");

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub notes: String,
    pub url: String,
    pub archive_url: String,
    pub archive_size: u64,
    pub sha256: String,
    manifest_bytes: Vec<u8>,
    manifest_signature: Vec<u8>,
}

#[derive(Debug)]
pub enum Event {
    Checked(Result<Option<Release>, String>),
    Progress { received: u64, total: u64 },
    Staged(Result<PathBuf, String>),
}

#[derive(Default)]
pub struct State {
    pub auto_check: bool,
    pub started: bool,
    pub checking: bool,
    pub release: Option<Release>,
    pub notice: bool,
    pub downloading: bool,
    pub downloaded: u64,
    pub total: u64,
    pub staged: Option<PathBuf>,
    pub error: Option<String>,
    pub manual_result: Option<String>,
    settings_path: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct Preferences { auto_check: bool }

impl State {
    pub fn load(settings_dir: &Path) -> Self {
        let settings_path = settings_dir.join("updates.json");
        let auto_check = fs::read(&settings_path).ok()
            .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
            .map_or(true, |preferences| preferences.auto_check);
        Self { auto_check, settings_path, ..Self::default() }
    }
    pub fn set_auto_check(&mut self, enabled: bool) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&Preferences { auto_check: enabled })
            .map_err(|e| e.to_string())?;
        if let Some(parent) = self.settings_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&self.settings_path, bytes).map_err(|e| e.to_string())?;
        self.auto_check = enabled;
        Ok(())
    }
}

fn stable_version(text: &str) -> Option<[u64; 3]> {
    let trimmed = text.trim().strip_prefix('v').unwrap_or(text.trim());
    let mut items = trimmed.split('.');
    let parse = |v: Option<&str>| -> Option<u64> {
        let v = v?;
        if v.is_empty() || (v.len() > 1 && v.starts_with('0')) || !v.bytes().all(|b| b.is_ascii_digit()) { return None; }
        v.parse().ok()
    };
    let version = [parse(items.next())?, parse(items.next())?, parse(items.next())?];
    items.next().is_none().then_some(version)
}

fn decode_hex<const N: usize>(hex: &str) -> Result<[u8; N], String> {
    if hex.len() != N * 2 { return Err("incorrect hexadecimal length".into()); }
    let mut data = [0u8; N];
    for (i, byte) in data.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16)
            .map_err(|_| "invalid hexadecimal bytes")?;
    }
    Ok(data)
}

fn trusted_key() -> Result<VerifyingKey, String> {
    let key = PUBLIC_KEY_HEX.ok_or("update verification key not configured at build time")?;
    VerifyingKey::from_bytes(&decode_hex::<32>(key)?)
        .map_err(|_| "invalid embedded update public key".into())
}

fn verify_signature_with_key(manifest: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<(), String> {
    if manifest.len() > MAX_MANIFEST as usize || signature.len() != 64 {
        return Err("invalid release manifest or signature length".into());
    }
    let sig = Signature::from_slice(signature).map_err(|_| "malformed release signature")?;
    key.verify_strict(manifest, &sig)
        .map_err(|_| "release manifest was not signed by the trusted publisher".into())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileInfo { size: u64, sha256: String }

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedManifest {
    format: u32,
    tag: String,
    archive: FileInfo,
    archive_name: String,
    files: BTreeMap<String, FileInfo>,
}

fn parse_signed_manifest(raw: &[u8], signature: &[u8]) -> Result<SignedManifest, String> {
    parse_signed_manifest_with_key(raw, signature, &trusted_key()?)
}

fn parse_signed_manifest_with_key(raw: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<SignedManifest, String> {
    verify_signature_with_key(raw, signature, key)?;
    let manifest: SignedManifest = serde_json::from_slice(raw)
        .map_err(|_| "invalid signed release manifest JSON")?;
    if manifest.format != 1 || stable_version(&manifest.tag).is_none() {
        return Err("unsupported release manifest format or tag".into());
    }
    if manifest.archive_name != format!("DinurdoJK-{}-windows-x64.zip", manifest.tag) {
        return Err("archive name is not bound to signed release tag".into());
    }
    check_digest(&manifest.archive.sha256)?;
    if manifest.archive.size == 0 || manifest.archive.size > MAX_ARCHIVE {
        return Err("signed archive exceeds size limit".into());
    }
    if !manifest.files.contains_key("DinurdoJK.exe") || manifest.files.len() > 9 {
        return Err("signed file allowlist must contain DinurdoJK.exe".into());
    }
    let mut total = 0u64;
    for (name, info) in &manifest.files {
        if !matches!(name.as_str(), "DinurdoJK.exe" | "phonon.dll" | "OpenALSoft.x86_64.dll" | "OpenAL-Soft-LGPL-2.0.txt" | "EalMan-LGPL-2.1.txt"
            | "msvcp140.dll" | "msvcp140_atomic_wait.dll" | "vcruntime140.dll" | "vcruntime140_1.dll") {
            return Err("signed manifest contains an unexpected file".into());
        }
        check_digest(&info.sha256)?;
        if info.size == 0 { return Err("signed manifest has an empty file".into()); }
        total = total.checked_add(info.size).ok_or("unpacked size overflow")?;
    }
    if total > MAX_UNPACKED { return Err("signed files exceed unpacked size limit".into()); }
    Ok(manifest)
}

fn check_digest(hash: &str) -> Result<(), String> {
    let _ = decode_hex::<32>(hash)?;
    if hash != hash.to_ascii_lowercase() { return Err("SHA-256 must be lowercase".into()); }
    Ok(())
}

fn asset_url(tag: &str, file: &str) -> String {
    format!("https://github.com/videop/DinurdoJK/releases/download/{tag}/{file}")
}

fn exact_asset_url(value: &serde_json::Value, tag: &str, name: &str) -> Result<String, String> {
    let assets = value["assets"].as_array().ok_or("GitHub release has no assets")?;
    if assets.iter().filter(|asset| asset["name"].as_str() == Some(name)).count() != 1 {
        return Err(format!("expected one GitHub asset named {name}"));
    }
    let found = assets.iter().find(|asset| asset["name"].as_str() == Some(name)).ok_or("missing release asset")?;
    let url = found["browser_download_url"].as_str().ok_or("missing asset URL")?;
    let expected = asset_url(tag, name);
    if url != expected { return Err("release asset URL differs from expected trusted repository path".into()); }
    Ok(expected)
}

fn fetch_small(url: &str, max: u64) -> Result<Vec<u8>, String> {
    let mut result = Vec::new();
    let status = http_get(url, max, |chunk, _| { result.extend_from_slice(chunk); Ok(()) })?;
    if status != 200 { return Err(format!("release metadata returned HTTP {status}")); }
    Ok(result)
}

fn parse_release_json(bytes: &[u8], installed: &str) -> Result<Option<Release>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    if value["draft"].as_bool().unwrap_or(false) || value["prerelease"].as_bool().unwrap_or(false) { return Ok(None); }
    let tag = value["tag_name"].as_str().ok_or("GitHub release has no tag_name")?;
    let remote = stable_version(tag).ok_or("release tag must be vMAJOR.MINOR.PATCH")?;
    let local = stable_version(installed).ok_or("installed version must be MAJOR.MINOR.PATCH")?;
    if remote <= local { return Ok(None); }

    let zip_name = format!("DinurdoJK-{tag}-windows-x64.zip");
    let archive_url = exact_asset_url(&value, tag, &zip_name)?;
    let manifest_url = exact_asset_url(&value, tag, MANIFEST_NAME)?;
    let signature_url = exact_asset_url(&value, tag, SIGNATURE_NAME)?;
    // No downloaded archive, no stage, and no installer until BOTH publisher
    // signature and the signed release/asset binding have been checked.
    let manifest_bytes = fetch_small(&manifest_url, MAX_MANIFEST)?;
    let manifest_signature = fetch_small(&signature_url, 64)?;
    let manifest = parse_signed_manifest(&manifest_bytes, &manifest_signature)?;
    if manifest.tag != tag || manifest.archive_name != zip_name {
        return Err("signed release identity does not match GitHub release".into());
    }
    let archive_asset = value["assets"].as_array().ok_or("no assets")?.iter()
        .find(|asset| asset["name"].as_str() == Some(zip_name.as_str())).ok_or("no archive asset")?;
    if archive_asset["size"].as_u64() != Some(manifest.archive.size) {
        return Err("GitHub asset size disagrees with signed manifest".into());
    }
    // GitHub asset digest is a helpful second consistency check, not the trust anchor.
    if let Some(digest) = archive_asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")) {
        if !digest.eq_ignore_ascii_case(&manifest.archive.sha256) {
            return Err("GitHub digest disagrees with signed manifest".into());
        }
    }
    Ok(Some(Release {
        tag: tag.to_owned(),
        notes: value["body"].as_str().unwrap_or("").chars().take(12_000).collect(),
        url: value["html_url"].as_str().unwrap_or("").to_owned(),
        archive_url,
        archive_size: manifest.archive.size,
        sha256: manifest.archive.sha256,
        manifest_bytes,
        manifest_signature,
    }))
}

fn interpret_check_response(status: u16, body: &[u8], installed: &str) -> Result<Option<Release>, String> {
    if status == 404 { return Ok(None); } // No published releases: intentionally silent.
    if status != 200 { return Err(format!("GitHub update check returned HTTP {status}")); }
    parse_release_json(body, installed)
}

pub fn check() -> Result<Option<Release>, String> {
    let mut data = Vec::new();
    let status = http_get(LATEST, 1024 * 1024, |chunk, _| {
        data.extend_from_slice(chunk);
        Ok(())
    })?;
    interpret_check_response(status, &data, env!("CARGO_PKG_VERSION"))
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        hash.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

// Reject reparse points (including Windows directory junctions, not just
// symlinks). These updater paths must not redirect outside their expected trees.
fn assert_no_reparse(path: &Path) -> Result<(), String> {
    for component in path.ancestors() {
        if let Ok(metadata) = fs::symlink_metadata(component) {
            if metadata.file_type().is_symlink() { return Err("update path contains symlink".into()); }
            #[cfg(windows)] {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 { return Err("update path contains Windows reparse point".into()); }
            }
        }
    }
    Ok(())
}

fn staging_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?;
    let root = PathBuf::from(home).join("DinurdoJK").join("updates");
    assert_no_reparse(&root)?;
    Ok(root)
}

fn stage_for(tag: &str) -> Result<PathBuf, String> {
    if stable_version(tag).is_none() || !tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.') {
        return Err("unsafe release tag".into());
    }
    let path = staging_root()?.join(tag);
    assert_no_reparse(&path)?;
    Ok(path)
}

fn read_bounded(path: &Path, max: u64) -> Result<Vec<u8>, String> {
    assert_no_reparse(path)?;
    let file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > max {
        return Err("update metadata exceeds size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max { return Err("update metadata exceeds size limit".into()); }
    Ok(bytes)
}

fn validate_stage(stage: &Path) -> Result<SignedManifest, String> {
    assert_no_reparse(stage)?;
    let raw = read_bounded(&stage.join(MANIFEST_NAME), MAX_MANIFEST)?;
    let sig = read_bounded(&stage.join(SIGNATURE_NAME), 64)?;
    let manifest = parse_signed_manifest(&raw, &sig)?;
    if stage != stage_for(&manifest.tag)? { return Err("stage not under trusted updater root".into()); }
    if !stage.join("ready").is_file() { return Err("update stage is not marked ready".into()); }
    let dir = stage.join("files");
    assert_no_reparse(&dir)?;
    let mut count = 0usize;
    for item in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        assert_no_reparse(&path)?;
        let name = path.file_name().and_then(|n| n.to_str()).ok_or("invalid staged name")?;
        let info = manifest.files.get(name).ok_or("unexpected staged file")?;
        if !path.is_file() || fs::metadata(&path).map_err(|e| e.to_string())?.len() != info.size || file_sha256(&path)? != info.sha256 {
            return Err(format!("staged {name} does not match signed manifest"));
        }
        count += 1;
    }
    if count != manifest.files.len() { return Err("staged files are missing".into()); }
    Ok(manifest)
}

pub fn stage<F>(release: &Release, mut report: F) -> Result<PathBuf, String>
where F: FnMut(u64, u64) {
    let manifest = parse_signed_manifest(&release.manifest_bytes, &release.manifest_signature)?;
    if manifest.tag != release.tag || manifest.archive.size != release.archive_size ||
        manifest.archive.sha256 != release.sha256 || manifest.archive_name != format!("DinurdoJK-{}-windows-x64.zip", release.tag) ||
        release.archive_url != asset_url(&release.tag, &manifest.archive_name) {
        return Err("release changed since signed metadata was verified".into());
    }
    if stable_version(&manifest.tag) <= stable_version(env!("CARGO_PKG_VERSION")) {
        return Err("refusing to install the same or an older release".into());
    }
    let stage = stage_for(&release.tag)?;
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    assert_no_reparse(&stage)?;
    if stage.join("installing").exists() { return Err("an installation is already in progress".into()); }
    let archive_tmp = stage.join("package.zip.part");
    let archive = stage.join("package.zip");
    let _ = fs::remove_file(stage.join("ready"));
    let _ = fs::remove_file(&archive_tmp);
    let _ = fs::remove_file(&archive);
    let result = (|| {
        let mut output = File::create_new(&archive_tmp).map_err(|e| e.to_string())?;
        let mut hasher = Sha256::new();
        let mut count = 0u64;
        let status = http_get(&release.archive_url, MAX_ARCHIVE, |part, _| {
            output.write_all(part).map_err(|e| e.to_string())?;
            hasher.update(part);
            count += part.len() as u64;
            report(count, release.archive_size);
            Ok(())
        })?;
        output.sync_all().map_err(|e| e.to_string())?;
        drop(output);
        if status != 200 || count != manifest.archive.size || format!("{:x}", hasher.finalize()) != manifest.archive.sha256 {
            return Err("download does not match signed release archive".into());
        }
        fs::rename(&archive_tmp, &archive).map_err(|e| e.to_string())?;
        let file = File::open(&archive).map_err(|e| e.to_string())?;
        let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        if zip.len() != manifest.files.len() { return Err("ZIP entry count differs from signed manifest".into()); }
        let dir = stage.join("files");
        assert_no_reparse(&dir)?;
        if dir.exists() { fs::remove_dir_all(&dir).map_err(|e| e.to_string())?; }
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let mut total = 0u64;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_owned();
            if entry.is_dir() || name.contains('/') || name.contains('\\') || name.contains(':') {
                return Err("ZIP paths and directories are not permitted".into());
            }
            if entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) {
                return Err("ZIP contains a symbolic link".into());
            }
            let info = manifest.files.get(&name).ok_or("ZIP contains an unauthorized file")?;
            if entry.size() != info.size { return Err("ZIP file size differs from signed manifest".into()); }
            total = total.checked_add(entry.size()).ok_or("ZIP size overflow")?;
            if total > MAX_UNPACKED { return Err("ZIP exceeds unpacked size limit".into()); }
            let path = dir.join(&name);
            if path.exists() { return Err("duplicate ZIP entry".into()); }
            let mut out = File::create_new(&path).map_err(|e| e.to_string())?;
            let copied = std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            if copied != info.size { return Err("short ZIP extraction".into()); }
            out.sync_all().map_err(|e| e.to_string())?;
            if file_sha256(&path)? != info.sha256 { return Err("extracted file hash does not match publisher signature".into()); }
        }
        for name in [MANIFEST_NAME, SIGNATURE_NAME, "ready"] {
            let path = stage.join(name);
            if path.exists() || fs::symlink_metadata(&path).is_ok() {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        let mut raw = File::create_new(stage.join(MANIFEST_NAME)).map_err(|e| e.to_string())?;
        raw.write_all(&release.manifest_bytes).map_err(|e| e.to_string())?;
        raw.sync_all().map_err(|e| e.to_string())?;
        let mut sig = File::create_new(stage.join(SIGNATURE_NAME)).map_err(|e| e.to_string())?;
        sig.write_all(&release.manifest_signature).map_err(|e| e.to_string())?;
        sig.sync_all().map_err(|e| e.to_string())?;
        // Re-read staged manifest and all files before committing readiness.
        let mut ready = File::create_new(stage.join("ready")).map_err(|e| e.to_string())?;
        ready.write_all(b"ready-v2").map_err(|e| e.to_string())?;
        ready.sync_all().map_err(|e| e.to_string())?;
        validate_stage(&stage)?;
        let pending = staging_root()?.join("pending.txt");
        assert_no_reparse(&pending)?;
        // Discard previous pending marker without following a user-controlled link.
        if fs::symlink_metadata(&pending).is_ok() { fs::remove_file(&pending).map_err(|e| e.to_string())?; }
        let mut pending_file = File::create_new(pending).map_err(|e| e.to_string())?;
        pending_file.write_all(release.tag.as_bytes()).map_err(|e| e.to_string())?;
        pending_file.sync_all().map_err(|e| e.to_string())?;
        Ok(stage.clone())
    })();
    if result.is_err() { let _ = fs::remove_file(&archive_tmp); let _ = fs::remove_file(stage.join("ready")); }
    result
}

/// A copy of the running executable acts as the helper: no separate unsigned
/// binary is downloaded. The helper re-verifies staged publisher authorization.
fn launch_helper(exe: &Path, stage: &Path, pid: u32) -> Result<(), String> {
    if exe.file_name().and_then(|n| n.to_str()) != Some("DinurdoJK.exe") {
        return Err("updater will only replace DinurdoJK.exe".into());
    }
    assert_no_reparse(exe)?;
    validate_stage(stage)?;
    // Keep the helper alongside the installed game, NOT inside the writable
    // downloaded stage: Windows searches the EXE directory for dependent DLLs
    // before our Rust signature validation runs.
    let dir = exe.parent().ok_or("game directory not found")?;
    assert_no_reparse(dir)?;
    let helper = dir.join(format!("DinurdoJK-Updater-{pid}.exe"));
    assert_no_reparse(&helper)?;
    if fs::symlink_metadata(&helper).is_ok() {
        fs::remove_file(&helper).map_err(|e| format!("remove stale updater helper: {e}"))?;
    }
    fs::copy(exe, &helper).map_err(|e| format!("copy updater helper: {e}"))?;
    std::process::Command::new(&helper)
        .arg("--djk-apply-update")
        .arg(pid.to_string())
        .arg(exe)
        .arg(stage)
        .spawn().map_err(|e| format!("launch updater helper: {e}"))?;
    Ok(())
}

pub fn restart_with_update(stage: &Path) -> Result<(), String> {
    let manifest = validate_stage(stage)?;
    if stable_version(&manifest.tag) <= stable_version(env!("CARGO_PKG_VERSION")) {
        return Err("not a newer signed release".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    launch_helper(&exe, stage, std::process::id())
}

pub fn apply_pending_on_launch() -> Result<bool, String> {
    let root = match staging_root() { Ok(root) => root, Err(_) => return Ok(false) };
    let tag = match fs::read_to_string(root.join("pending.txt")) { Ok(tag) => tag, Err(_) => return Ok(false) };
    let stage = match stage_for(&tag) {
        Ok(stage) => stage,
        Err(_) => { let _ = fs::remove_file(root.join("pending.txt")); return Ok(false); }
    };
    let manifest = validate_stage(&stage)?;
    let local = stable_version(env!("CARGO_PKG_VERSION")).ok_or("invalid running version")?;
    let remote = stable_version(&manifest.tag).ok_or("invalid release version")?;
    if remote <= local {
        // After a power loss, the EXE may have been atomically replaced but
        // the DLL not yet replaced. Resume ONLY when a transaction was begun
        // and this EXE matches the publisher-signed bytes exactly.
        if remote != local || !stage.join("installing").is_file() ||
            file_sha256(&std::env::current_exe().map_err(|e| e.to_string())?)? !=
                manifest.files.get("DinurdoJK.exe").ok_or("signed EXE missing")?.sha256 {
            let _ = fs::remove_file(root.join("pending.txt"));
            return Ok(false);
        }
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    launch_helper(&exe, &stage, std::process::id())?;
    Ok(true)
}

fn write_sync(path: &Path, contents: &[u8]) -> Result<(), String> {
    assert_no_reparse(path)?;
    let mut file = File::create(path).map_err(|e| e.to_string())?;
    file.write_all(contents).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

/// Copy to a temporary *in the installation directory*, fsync it, then
/// replace via Windows' same-volume MoveFileExW rather than truncating a live
/// file. Each file is atomic; transaction journal handles multiple files.
fn replace_atomically(source: &Path, target: &Path, expected: &str) -> Result<(), String> {
    assert_no_reparse(source)?;
    assert_no_reparse(target)?;
    if file_sha256(source)? != expected { return Err("replacement file hash mismatch".into()); }
    let mut target_tmp = target.to_path_buf();
    target_tmp.set_extension("djk-update-tmp");
    assert_no_reparse(&target_tmp)?;
    if target_tmp.exists() { fs::remove_file(&target_tmp).map_err(|e| e.to_string())?; }
    fs::copy(source, &target_tmp).map_err(|e| e.to_string())?;
    File::open(&target_tmp).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
    if file_sha256(&target_tmp)? != expected { return Err("copied replacement hash mismatch".into()); }
    #[cfg(windows)] {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
        }
        let from: Vec<u16> = target_tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } == 0 {
            return Err(format!("atomic replacement failed: {}", std::io::Error::last_os_error()));
        }
    }
    #[cfg(not(windows))] {
        fs::rename(&target_tmp, target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Backup the installed files *before* creating the durable `installing`
/// marker. An incomplete backup preparation cannot lead to a partial update.
fn prepare_backups(stage: &Path, parent: &Path, manifest: &SignedManifest) -> Result<BTreeMap<String, Option<String>>, String> {
    let backup = stage.join("backup");
    assert_no_reparse(&backup)?;
    if backup.exists() { fs::remove_dir_all(&backup).map_err(|e| e.to_string())?; }
    fs::create_dir(&backup).map_err(|e| e.to_string())?;
    let mut snapshot = BTreeMap::new();
    for name in manifest.files.keys() {
        let installed = parent.join(name);
        assert_no_reparse(&installed)?;
        if installed.exists() {
            if !installed.is_file() { return Err("installed target is not a normal file".into()); }
            let copied = backup.join(name);
            fs::copy(installed, &copied).map_err(|e| e.to_string())?;
            File::open(&copied).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            snapshot.insert(name.clone(), Some(file_sha256(&copied)?));
        } else {
            snapshot.insert(name.clone(), None);
        }
    }
    write_sync(&stage.join("installing"), b"signed-update-transaction-v1")?;
    Ok(snapshot)
}

/// Best-effort rollback if an install fails in-process. Power loss recovery is
/// forward completion using the signed staged files on the next game launch.
fn rollback(stage: &Path, parent: &Path, manifest: &SignedManifest,
            snapshot: &BTreeMap<String, Option<String>>) -> Result<(), String> {
    let backup = stage.join("backup");
    assert_no_reparse(&backup)?;
    for name in manifest.files.keys() {
        let old = backup.join(name);
        let target = parent.join(name);
        assert_no_reparse(&target)?;
        match snapshot.get(name).ok_or("backup snapshot is incomplete")? {
            Some(expected) => {
                assert_no_reparse(&old)?;
                if file_sha256(&old)? != *expected {
                    return Err("original backup changed after it was prepared".into());
                }
                replace_atomically(&old, &target, expected)?;
            }
            None if name != "DinurdoJK.exe" => {
                if target.exists() { fs::remove_file(&target).map_err(|e| e.to_string())?; }
            }
            None => return Err("original executable backup missing".into()),
        }
    }
    Ok(())
}

fn apply_update(exe: &Path, stage: &Path, pid: u32) -> Result<(), String> {
    if exe.file_name().and_then(|n| n.to_str()) != Some("DinurdoJK.exe") {
        return Err("unexpected installation target".into());
    }
    let manifest = validate_stage(stage)?;
    // Do not accept the command-line target as authoritative: it must be the
    // exact currently-running game image that was copied into this helper.
    let helper_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    assert_no_reparse(&helper_exe)?;
    if file_sha256(&helper_exe)? != file_sha256(exe)? {
        return Err("installer target does not match the invoking game executable".into());
    }
    let installed = stable_version(env!("CARGO_PKG_VERSION")).ok_or("invalid helper version")?;
    if stable_version(&manifest.tag) <= Some(installed) {
        // Existing helper is a copy of the pre-update EXE; a retry can carry
        // the post-update version only if a journal exists.
        if stable_version(&manifest.tag) != Some(installed) || !stage.join("installing").is_file() {
            return Err("refusing same-version or downgrade installation".into());
        }
    }
    assert_no_reparse(exe)?;
    let parent = exe.parent().ok_or("missing target directory")?;
    assert_no_reparse(parent)?;
    wait_for_process(pid)?;
    // Recheck trust and file hashes after parent process exits, not only when
    // Download & Install was clicked.
    validate_stage(stage)?;
    // Never restore old code from a disk-only crash journal: another local
    // process could have tampered with the unsigned backup while we were gone.
    // Crash recovery is roll-forward using the publisher-signed new files.
    let snapshot = if stage.join("installing").is_file() {
        None
    } else {
        Some(prepare_backups(stage, parent, &manifest)?)
    };
    let operation = (|| -> Result<(), String> {
        for (name, info) in &manifest.files {
            let target = parent.join(name);
            assert_no_reparse(&target)?;
            if target.exists() && file_sha256(&target)? == info.sha256 { continue; }
            replace_atomically(&stage.join("files").join(name), &target, &info.sha256)?;
        }
        for (name, info) in &manifest.files {
            if file_sha256(&parent.join(name))? != info.sha256 {
                return Err("installed files failed final verification".into());
            }
        }
        Ok(())
    })();
    if let Err(error) = operation {
        if let Some(snapshot) = &snapshot {
            if let Err(recovery_error) = rollback(stage, parent, &manifest, snapshot) {
                // Leave pending + journal to permit recovery on subsequent launch.
                return Err(format!("{error}; rollback also failed: {recovery_error}"));
            }
            let _ = fs::remove_file(stage.join("installing"));
            let _ = fs::remove_file(staging_root()?.join("pending.txt"));
        }
        // If resuming an interrupted install, do NOT trust/reinstall the
        // unsigned original backup; leave the journal for a signed retry.
        return Err(error);
    }
    // Commit before starting the new process. Otherwise its startup could
    // race pending.txt and launch a second installer for the same release.
    let _ = fs::remove_file(staging_root()?.join("pending.txt"));
    let _ = fs::remove_file(stage.join("installing"));
    let _ = fs::remove_file(stage.join("ready"));
    std::process::Command::new(exe).spawn()
        .map_err(|e| format!("update installed; relaunch DinurdoJK manually: {e}"))?;
    Ok(())
}

pub fn helper_entry() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--djk-apply-update")) { return None; }
    let result = (|| -> Result<(), String> {
        let pid = args.next().and_then(|p| p.to_string_lossy().parse::<u32>().ok()).ok_or("missing process ID")?;
        let exe = PathBuf::from(args.next().ok_or("missing game path")?);
        let stage = PathBuf::from(args.next().ok_or("missing stage path")?);
        if args.next().is_some() { return Err("unexpected helper arguments".into()); }
        apply_update(&exe, &stage, pid)
    })();
    if let Err(err) = &result {
        if let Ok(root) = staging_root() {
            let _ = fs::create_dir_all(&root);
            let _ = fs::write(root.join("last-error.txt"), err);
        }
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

#[cfg(windows)]
fn wait_for_process(pid: u32) -> Result<(), String> {
    use windows_sys::Win32::{Foundation::{CloseHandle, WAIT_OBJECT_0}, System::Threading::{OpenProcess, WaitForSingleObject}};
    // SYNCHRONIZE is a standard-access right (0x0010_0000), not a Threading
    // module constant in windows-sys 0.61.
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let handle = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
    // A fast-exiting parent may already be gone when the helper starts.
    if handle.is_null() { return Ok(()); }
    let status = unsafe { WaitForSingleObject(handle, 60_000) };
    unsafe { CloseHandle(handle) };
    if status != WAIT_OBJECT_0 { return Err("game did not exit within 60 seconds".into()); }
    Ok(())
}
#[cfg(not(windows))]
fn wait_for_process(_pid: u32) -> Result<(), String> { Err("Updater currently supports Windows only".into()) }


/// Bounded, HTTPS-only streaming GET. WinHTTP follows GitHub's secure CDN
/// redirects and validates the server TLS certificate. Used on workers only.
#[cfg(windows)]
fn http_get<F>(url: &str, max: u64, mut receive: F) -> Result<u16, String>
where F: FnMut(&[u8], u64) -> Result<(), String> {
    use std::{ffi::c_void, ptr};
    type Internet = *mut c_void;
    #[link(name = "winhttp")]
    unsafe extern "system" {
        fn WinHttpOpen(agent: *const u16, access: u32, proxy: *const u16, bypass: *const u16, flags: u32) -> Internet;
        fn WinHttpConnect(session: Internet, host: *const u16, port: u16, reserved: u32) -> Internet;
        fn WinHttpOpenRequest(connect: Internet, verb: *const u16, path: *const u16, version: *const u16, referer: *const u16, types: *const *const u16, flags: u32) -> Internet;
        fn WinHttpSetTimeouts(handle: Internet, resolve: i32, connect: i32, send: i32, receive: i32) -> i32;
        fn WinHttpSendRequest(request: Internet, headers: *const u16, len: u32, optional: *mut c_void, optional_len: u32, total: u32, context: usize) -> i32;
        fn WinHttpReceiveResponse(request: Internet, reserved: *mut c_void) -> i32;
        fn WinHttpQueryHeaders(request: Internet, level: u32, name: *const u16, output: *mut c_void, size: *mut u32, index: *mut u32) -> i32;
        fn WinHttpReadData(request: Internet, buffer: *mut c_void, want: u32, got: *mut u32) -> i32;
        fn WinHttpCloseHandle(handle: Internet) -> i32;
    }
    struct Handle(Internet);
    impl Drop for Handle { fn drop(&mut self) { if !self.0.is_null() { unsafe { WinHttpCloseHandle(self.0); } } } }
    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }
    let authority = url.strip_prefix("https://").ok_or("updater requires HTTPS")?;
    let slash = authority.find('/').unwrap_or(authority.len());
    let (host, path) = authority.split_at(slash);
    if host.is_empty() || host.contains('@') || host.contains(':') { return Err("invalid GitHub HTTPS URL".into()); }
    let host = wide(host);
    let agent = wide("DinurdoJK-Updater/1.0");
    let path = wide(if path.is_empty() { "/" } else { path });
    let session = Handle(unsafe { WinHttpOpen(agent.as_ptr(), 0, ptr::null(), ptr::null(), 0) });
    if session.0.is_null() { return Err("WinHTTP open failed".into()); }
    unsafe { WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 20_000); }
    let connection = Handle(unsafe { WinHttpConnect(session.0, host.as_ptr(), 443, 0) });
    if connection.0.is_null() { return Err("WinHTTP connect failed".into()); }
    let method = wide("GET");
    let request = Handle(unsafe { WinHttpOpenRequest(connection.0, method.as_ptr(), path.as_ptr(), ptr::null(), ptr::null(), ptr::null(), 0x0080_0000) });
    if request.0.is_null() { return Err("WinHTTP request failed".into()); }
    let headers = wide("Accept: application/vnd.github+json\r\nAccept-Encoding: identity\r\n");
    if unsafe { WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, ptr::null_mut(), 0, 0, 0) } == 0 ||
       unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
        return Err("GitHub HTTPS request failed".into());
    }
    let mut status = 0u32;
    let mut len = 4u32;
    if unsafe { WinHttpQueryHeaders(request.0, 19 | 0x20000000, ptr::null(), (&mut status as *mut u32).cast(), &mut len, ptr::null_mut()) } == 0 {
        return Err("GitHub HTTP status unavailable".into());
    }
    if status != 200 { return Ok(status as u16); }
    let mut total = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let mut n = 0u32;
        if unsafe { WinHttpReadData(request.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n) } == 0 {
            return Err("GitHub download interrupted".into());
        }
        if n == 0 { break; }
        total = total.checked_add(n as u64).ok_or("download overflow")?;
        if total > max { return Err("update download exceeds size limit".into()); }
        receive(&buf[..n as usize], total)?;
    }
    Ok(status as u16)
}
#[cfg(not(windows))]
fn http_get<F>(_url: &str, _max: u64, _receive: F) -> Result<u16, String>
where F: FnMut(&[u8], u64) -> Result<(), String> {
    Err("GitHub update checking currently supports Windows only".into())
}


#[cfg(test)]
mod security_tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    // Deterministic throwaway key exclusively for negative test vectors.
    fn signed_fixture() -> (Vec<u8>, Vec<u8>, VerifyingKey) {
        let key = SigningKey::from_bytes(&[41; 32]);
        let manifest = serde_json::json!({
            "format": 1,
            "tag": "v0.2.0",
            "archive_name": "DinurdoJK-v0.2.0-windows-x64.zip",
            "archive": {"size": 100, "sha256": "a".repeat(64)},
            "files": {"DinurdoJK.exe": {"size": 3, "sha256": "b".repeat(64)}}
        });
        let raw = serde_json::to_vec(&manifest).unwrap();
        let signature = key.sign(&raw).to_bytes().to_vec();
        (raw, signature, key.verifying_key())
    }

    #[test]
    fn signed_manifest_is_bound_to_exact_bytes() {
        let (raw, sig, key) = signed_fixture();
        assert!(parse_signed_manifest_with_key(&raw, &sig, &key).is_ok());
        let mut modified = raw.clone();
        modified[0] ^= 1;
        assert!(parse_signed_manifest_with_key(&modified, &sig, &key).is_err());
        assert!(parse_signed_manifest_with_key(&raw, &[0; 64], &key).is_err());
        let impostor = SigningKey::from_bytes(&[42; 32]).verifying_key();
        assert!(parse_signed_manifest_with_key(&raw, &sig, &impostor).is_err());
        assert!(parse_signed_manifest_with_key(&raw, &sig[..63], &key).is_err());
    }

    #[test]
    fn publisher_cannot_accidentally_include_arbitrary_install_paths() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        let files = value["files"].as_object_mut().unwrap();
        files.insert("../evil.exe".into(), serde_json::json!({"size": 3, "sha256": "b".repeat(64)}));
        let invalid = serde_json::to_vec(&value).unwrap();
        let signed = signing.sign(&invalid).to_bytes();
        assert!(parse_signed_manifest_with_key(&invalid, &signed, &key).is_err());
    }

    #[test]
    fn signed_eal_runtime_and_license_are_accepted_without_widening_install_paths() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        for name in ["phonon.dll", "OpenALSoft.x86_64.dll", "OpenAL-Soft-LGPL-2.0.txt", "EalMan-LGPL-2.1.txt",
            "msvcp140.dll", "msvcp140_atomic_wait.dll", "vcruntime140.dll", "vcruntime140_1.dll"] {
            value["files"][name] = serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        }
        let raw = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&raw, &signing.sign(&raw).to_bytes(), &key).is_ok());
        value["files"].as_object_mut().unwrap().remove("phonon.dll");
        value["files"]["../OpenALSoft.x86_64.dll"] = serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        let raw = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&raw, &signing.sign(&raw).to_bytes(), &key).is_err());
    }

    #[test]
    fn malformed_signed_version_and_unsigned_release_are_rejected() {
        assert_eq!(stable_version("v0.10.0"), Some([0, 10, 0]));
        assert_eq!(stable_version("v0.2.0-rc1"), None);
        assert!(interpret_check_response(404, b"", "0.1.0").unwrap().is_none());
        assert!(interpret_check_response(403, b"", "0.1.0").is_err());
        let release = br#"{"tag_name":"v0.2.0","assets":[]}"#;
        assert!(parse_release_json(release, "0.1.0").is_err());
    }
}
