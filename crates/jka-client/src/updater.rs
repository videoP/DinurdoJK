//! GitHub Releases updater. One check at launch, downloads only on a user click.
//! The signed v2 manifest lists per-file release assets, so unchanged installed
//! files are not downloaded again.
//! Release AUTHORIZATION is independent of GitHub: fail closed without a
//! compiled-in Ed25519 verifying key and a valid signature on exact manifest bytes.
use std::{collections::BTreeMap, fs, fs::File, io::{Read, Write}, path::{Path, PathBuf}};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LATEST: &str = "https://api.github.com/repos/videop/DinurdoJK/releases/latest";
const MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;
const MAX_UNPACKED: u64 = 1024 * 1024 * 1024;
const MAX_MANIFEST: u64 = 16 * 1024;
// Release assets are published with plain basenames but installed under
// DinurdoJK/dependencies/. Keep this explicit list: never accept arbitrary
// signed subpaths or filesystem traversal.
const DEPENDENCY_RELEASE_FILES: &[&str] = &[
    "phonon.dll", "OpenALSoft.x86_64.dll", "nvngx_dlss.dll",
    "dxcompiler.dll", "dxil.dll", "ffmpeg.exe",
    "msvcp140.dll", "msvcp140_atomic_wait.dll",
    "vcruntime140.dll", "vcruntime140_1.dll",
];

const MANIFEST_NAME: &str = "release-manifest.json";
const SIGNATURE_NAME: &str = "release-manifest.sig";
const LAST_ERROR_NAME: &str = "last-error.txt";
// New files are written beside their targets while the game is still running;
// the displaced originals are renamed (not copied) to OLD_SUFFIX during the swap.
const TMP_SUFFIX: &str = ".djk-update-tmp";
const OLD_SUFFIX: &str = ".djk-old";
// Set by a failed helper on the game it relaunches, so that one launch cannot
// re-enter the updater. The game must always start after a failed update.
const SKIP_PENDING_ENV: &str = "DINURDOJK_SKIP_PENDING_UPDATE";
// A journalled (partially applied) install is rolled forward on later launches,
// but only this many times before the stage is abandoned.
const MAX_RESUME_ATTEMPTS: u32 = 2;
// The build must supply only the PUBLIC key. Never ship or embed the private key.
// No key => update checks fail closed if/when a new release is published.
const PUBLIC_KEY_HEX: Option<&str> = option_env!("DINURDOJK_UPDATE_PUBLIC_KEY_HEX");

#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub notes: String,
    pub url: String,
    pub total_size: u64,
    manifest_bytes: Vec<u8>,
    manifest_signature: Vec<u8>,
}

#[derive(Debug)]
pub enum Event {
    Checked(Result<Option<Release>, String>),
    Progress { received: u64, total: u64 },
    /// Individual decisions, computed on the download worker, for the console log.
    FileDecision(String),
    Staged(Result<PathBuf, String>),
    /// The restart helper was started (Ok) and the game may quit.
    Applied(Result<(), String>),
}

#[derive(Default)]
pub struct State {
    pub auto_check: bool,
    pub started: bool,
    pub checking: bool,
    pub release: Option<Release>,
    pub notice: bool,
    pub downloading: bool,
    pub applying: bool,
    pub(crate) notice_hovered: Option<crate::ui::update_notice::Action>,
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
        // Surface what the previous update helper reported, exactly once.
        let error = staging_root().ok().and_then(|root| take_last_error(&root))
            .map(|text| format!("The last update did not install: {text}"));
        Self { auto_check, settings_path, error, ..Self::default() }
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
    files: BTreeMap<String, FileInfo>,
}

fn parse_signed_manifest(raw: &[u8], signature: &[u8]) -> Result<SignedManifest, String> {
    parse_signed_manifest_with_key(raw, signature, &trusted_key()?)
}

fn parse_signed_manifest_with_key(raw: &[u8], signature: &[u8], key: &VerifyingKey) -> Result<SignedManifest, String> {
    verify_signature_with_key(raw, signature, key)?;
    let manifest: SignedManifest = serde_json::from_slice(raw)
        .map_err(|_| "invalid signed release manifest JSON")?;
    if manifest.format != 2 || stable_version(&manifest.tag).is_none() {
        return Err("unsupported signed per-file release manifest format or tag".into());
    }
    if !manifest.files.contains_key("DinurdoJK.exe") || manifest.files.len() > DEPENDENCY_RELEASE_FILES.len() + 2 {
        return Err("signed file allowlist must contain DinurdoJK.exe".into());
    }
    let mut total = 0u64;
    for (name, info) in &manifest.files {
        // The updater is intentionally runtime-only. License and attribution
        // notices belong to the manual full ZIP, never signed install targets.
        let allowed_root = matches!(name.as_str(), "DinurdoJK.exe"
            | "DinurdoJK/shared/dinurdo-assets.pk4");
        let allowed_dependency = name.strip_prefix("DinurdoJK/dependencies/")
            .is_some_and(|file| DEPENDENCY_RELEASE_FILES.contains(&file));
        if !allowed_root && !allowed_dependency {
            return Err(format!("signed manifest contains an unexpected file: {name:?}"));
        }
        check_digest(&info.sha256)?;
        if info.size == 0 || info.size > MAX_DOWNLOAD {
            return Err("signed manifest has an empty or oversized file".into());
        }
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

// GitHub release asset names are flat; signed install paths may be nested.
// The asset PK4 and runtime dependencies have fixed, validated destinations.
fn release_asset_name(installed_name: &str) -> &str {
    installed_name.strip_prefix("DinurdoJK/dependencies/")
        .or_else(|| installed_name.strip_prefix("DinurdoJK/shared/"))
        .unwrap_or(installed_name)
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
    if !url.eq_ignore_ascii_case(&expected) {
        return Err("release asset URL differs from expected trusted repository path".into());
    }
    Ok(url.to_owned())
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

    let manifest_url = exact_asset_url(&value, tag, MANIFEST_NAME)?;
    let signature_url = exact_asset_url(&value, tag, SIGNATURE_NAME)?;
    // Verify the publisher signature before trusting any file, SHA-256 or
    // destination path, and bind every asset to this exact GitHub release.
    let manifest_bytes = fetch_small(&manifest_url, MAX_MANIFEST)?;
    let manifest_signature = fetch_small(&signature_url, 64)?;
    let manifest = parse_signed_manifest(&manifest_bytes, &manifest_signature)?;
    if manifest.tag != tag {
        return Err("signed release tag does not match GitHub release".into());
    }
    for (name, info) in &manifest.files {
        let asset = release_asset_name(name);
        exact_asset_url(&value, tag, asset)?;
        let listed = value["assets"].as_array().ok_or("no GitHub release assets")?.iter()
            .find(|item| item["name"].as_str() == Some(asset)).ok_or("missing signed release asset")?;
        if listed["size"].as_u64() != Some(info.size) {
            return Err(format!("GitHub asset {asset} size differs from signed manifest"));
        }
        if let Some(digest) = listed["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")) {
            if !digest.eq_ignore_ascii_case(&info.sha256) {
                return Err(format!("GitHub asset {asset} digest differs from signed manifest"));
            }
        }
    }
    Ok(Some(Release {
        tag: tag.to_owned(),
        notes: value["body"].as_str().unwrap_or("").chars().take(12_000).collect(),
        url: value["html_url"].as_str().unwrap_or("").to_owned(),
        total_size: manifest.files.values().map(|info| info.size).sum(),
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

// Update state is per installation directory: two installs must not share
// pending.txt or last-error.txt. The helper is a copy of the EXE in the same
// directory, so it derives the same key.
fn staging_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("game directory is unavailable")?;
    let root = PathBuf::from(home).join("DinurdoJK").join("updates").join(install_key(dir));
    assert_no_reparse(&root)?;
    Ok(root)
}

fn install_key(dir: &Path) -> String {
    let digest = Sha256::digest(dir.to_string_lossy().to_lowercase().as_bytes());
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

fn take_last_error(root: &Path) -> Option<String> {
    let path = root.join(LAST_ERROR_NAME);
    let text = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    Some(text.chars().take(2000).collect())
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

// Signed installation targets are restricted by parse_signed_manifest_with_key.
// Keep the update staging directory flat, even for nested install paths.
fn stage_filename(signed_name: &str) -> String {
    if let Some(name) = signed_name.strip_prefix("DinurdoJK/dependencies/") {
        format!("deps__{name}")
    } else if signed_name == "DinurdoJK/shared/dinurdo-assets.pk4" {
        "shared__dinurdo-assets.pk4".into()
    } else {
        signed_name.into()
    }
}

fn signed_name_from_stage(staged_name: &str) -> String {
    if let Some(name) = staged_name.strip_prefix("deps__") {
        if DEPENDENCY_RELEASE_FILES.contains(&name) {
            return format!("DinurdoJK/dependencies/{name}");
        }
    }
    if staged_name == "shared__dinurdo-assets.pk4" {
        "DinurdoJK/shared/dinurdo-assets.pk4".into()
    } else { staged_name.into() }
}

fn ensure_install_parent(root: &Path, signed_name: &str) -> Result<(), String> {
    let destination = if signed_name == "DinurdoJK/shared/dinurdo-assets.pk4" {
        Some("shared")
    } else if signed_name.strip_prefix("DinurdoJK/dependencies/")
        .is_some_and(|file| DEPENDENCY_RELEASE_FILES.contains(&file)) {
        Some("dependencies")
    } else {
        None
    };
    if let Some(directory) = destination {
        let client = root.join("DinurdoJK");
        assert_no_reparse(&client)?;
        if !client.exists() { fs::create_dir(&client).map_err(|e| e.to_string())?; }
        if !client.is_dir() { return Err("client data directory is not a directory".into()); }
        assert_no_reparse(&client)?;
        let child = client.join(directory);
        assert_no_reparse(&child)?;
        if !child.exists() { fs::create_dir(&child).map_err(|e| e.to_string())?; }
        if !child.is_dir() { return Err("client destination directory is not a directory".into()); }
        assert_no_reparse(&child)?;
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum InstalledFileStatus {
    Identical,
    Missing,
    NotARegularFile,
    SizeMismatch { local: u64, release: u64 },
    HashMismatch { local: String, release: String },
}
impl InstalledFileStatus {
    fn is_identical(&self) -> bool { matches!(self, Self::Identical) }
    fn reason(&self) -> String {
        match self {
            Self::Identical => "identical SHA-256 - skipping".into(),
            Self::Missing => "missing - downloading".into(),
            Self::NotARegularFile => "not a regular file - downloading".into(),
            Self::SizeMismatch { local, release } =>
                format!("size mismatch ({local} local vs {release} release bytes) - downloading"),
            Self::HashMismatch { local, release } =>
                format!("SHA-256 mismatch (local {local}, release {release}) - downloading"),
        }
    }
}
fn installed_file_status(root: &Path, name: &str, info: &FileInfo) -> Result<InstalledFileStatus, String> {
    let path = root.join(name);
    assert_no_reparse(&path)?;
    if !path.is_file() {
        return Ok(if path.exists() { InstalledFileStatus::NotARegularFile } else { InstalledFileStatus::Missing });
    }
    let local = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if local != info.size { return Ok(InstalledFileStatus::SizeMismatch {local, release:info.size}); }
    let hash = file_sha256(&path)?;
    if hash != info.sha256 { return Ok(InstalledFileStatus::HashMismatch { local:hash, release:info.sha256.clone() }); }
    Ok(InstalledFileStatus::Identical)
}
fn installed_matches(root: &Path, name: &str, info: &FileInfo) -> Result<bool, String> {
    Ok(installed_file_status(root, name, info)?.is_identical())
}

fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    target.with_file_name(name)
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a.parent().zip(b.parent()).is_some_and(|(x, y)|
        x.to_string_lossy().eq_ignore_ascii_case(&y.to_string_lossy()))
}

// Authorization and structure of a stage: publisher signature, tag, ready
// marker, and that every staged file is a signed name with its signed size.
// File CONTENTS are hashed where they are consumed (download, the verified
// copy into the install directory, and just before the swap) instead of being
// re-read on every call. Files absent from the stage must already be installed
// with the signed size; their hashes were checked when the stage was created.
fn validate_stage(stage: &Path, install_root: &Path) -> Result<SignedManifest, String> {
    assert_no_reparse(stage)?;
    assert_no_reparse(install_root)?;
    let raw = read_bounded(&stage.join(MANIFEST_NAME), MAX_MANIFEST)?;
    let sig = read_bounded(&stage.join(SIGNATURE_NAME), 64)?;
    let manifest = parse_signed_manifest(&raw, &sig)?;
    if stage != stage_for(&manifest.tag)? { return Err("stage not under trusted updater root".into()); }
    if !stage.join("ready").is_file() { return Err("update stage is not marked ready".into()); }
    let dir = stage.join("files");
    assert_no_reparse(&dir)?;
    for item in fs::read_dir(&dir).map_err(|e| io_error("read", &dir, e))? {
        let path = item.map_err(|e| e.to_string())?.path();
        assert_no_reparse(&path)?;
        let name = path.file_name().and_then(|n| n.to_str()).ok_or("invalid staged name")?;
        let info = manifest.files.get(&signed_name_from_stage(name)).ok_or("unexpected staged file")?;
        if !fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == info.size) {
            return Err(format!("staged {name} does not match signed manifest"));
        }
    }
    for (name, info) in &manifest.files {
        if dir.join(stage_filename(name)).is_file() { continue; }
        let installed = install_root.join(name);
        assert_no_reparse(&installed)?;
        if !fs::metadata(&installed).is_ok_and(|m| m.is_file() && m.len() == info.size) {
            return Err(format!("required update file {name} is neither installed nor staged"));
        }
    }
    Ok(manifest)
}

/// Copy `source` to `target` while hashing the bytes actually written, and
/// refuse the result unless it equals the signed SHA-256.
fn copy_verified(source: &Path, target: &Path, expected: &str) -> Result<(), String> {
    assert_no_reparse(source)?;
    assert_no_reparse(target)?;
    if target.exists() { fs::remove_file(target).map_err(|e| io_error("remove", target, e))?; }
    let result = (|| -> Result<(), String> {
        let mut input = File::open(source).map_err(|e| io_error("open", source, e))?;
        let mut output = File::create_new(target).map_err(|e| io_error("create", target, e))?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            let n = input.read(&mut buffer).map_err(|e| io_error("read", source, e))?;
            if n == 0 { break; }
            hasher.update(&buffer[..n]);
            output.write_all(&buffer[..n]).map_err(|e| io_error("write", target, e))?;
        }
        // Flush through the handle that has write access. A read-only
        // File::open(..).sync_all() fails with "Access is denied" (os error 5).
        output.sync_all().map_err(|e| io_error("flush", target, e))?;
        if format!("{:x}", hasher.finalize()) != expected {
            return Err(format!("{} does not match the publisher-signed SHA-256", source.display()));
        }
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(target); }
    result
}

fn remove_preinstalled(root: &Path, manifest: &SignedManifest) {
    for name in manifest.files.keys() {
        let _ = fs::remove_file(sibling(&root.join(name), TMP_SUFFIX));
    }
}

/// Runs on the download worker while the game is still running: put every
/// changed file next to its target as a verified temporary copy, so that the
/// restart only has to rename.
fn preinstall(stage: &Path, root: &Path, manifest: &SignedManifest) -> Result<(), String> {
    remove_preinstalled(root, manifest);
    for (name, info) in &manifest.files {
        let staged = stage.join("files").join(stage_filename(name));
        if !staged.is_file() { continue; }
        ensure_install_parent(root, name)?;
        copy_verified(&staged, &sibling(&root.join(name), TMP_SUFFIX), &info.sha256)?;
    }
    Ok(())
}

pub fn stage<F, G>(release: &Release, mut report: F, mut report_file: G) -> Result<PathBuf, String>
where F: FnMut(u64, u64), G: FnMut(String) {
    let manifest = parse_signed_manifest(&release.manifest_bytes, &release.manifest_signature)?;
    if manifest.tag != release.tag ||
        release.total_size != manifest.files.values().map(|info| info.size).sum::<u64>() {
        return Err("release changed since signed metadata was verified".into());
    }
    if stable_version(&manifest.tag) <= stable_version(env!("CARGO_PKG_VERSION")) {
        return Err("refusing to install the same or an older release".into());
    }
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    if current.file_name().and_then(|n| n.to_str()) != Some("DinurdoJK.exe") {
        return Err("unexpected game executable path".into());
    }
    let install_root = current.parent().ok_or("game directory is unavailable")?;
    assert_no_reparse(install_root)?;
    let mut needed = Vec::new();
    let mut total = 0u64;
    report_file(format!("[UPDATE] Comparing {} signed files in {}", manifest.files.len(), install_root.display()));
    for (name, info) in &manifest.files {
        let status = installed_file_status(install_root, name, info)?;
        report_file(format!("[UPDATE] {} -> {}: {}", name, install_root.join(name).display(), status.reason()));
        if !status.is_identical() {
            total = total.checked_add(info.size).ok_or("download size overflow")?;
            needed.push((name, info));
        }
    }
    report_file(format!("[UPDATE] {} file(s) require download; {} unchanged", needed.len(), manifest.files.len()-needed.len()));
    report(0, total);
    let stage = stage_for(&release.tag)?;
    fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    assert_no_reparse(&stage)?;
    if stage.join("installing").exists() { return Err("an installation is already in progress".into()); }
    let _ = fs::remove_file(stage.join("ready"));
    let _ = fs::remove_file(stage.join("attempts"));
    let result = (|| {
        let dir = stage.join("files");
        assert_no_reparse(&dir)?;
        if dir.exists() { fs::remove_dir_all(&dir).map_err(|e| e.to_string())?; }
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let mut completed = 0u64;
        for (name, info) in needed {
            let asset = release_asset_name(name);
            report_file(format!("[UPDATE] Downloading {asset} -> {name} ({} bytes)", info.size));
            let url = asset_url(&release.tag, asset);
            let path = dir.join(stage_filename(name));
            let partial = dir.join(format!("{}.part", stage_filename(name)));
            let _ = fs::remove_file(&partial);
            let mut output = File::create_new(&partial).map_err(|e| e.to_string())?;
            let mut hasher = Sha256::new();
            let mut count = 0u64;
            let status = http_get(&url, info.size, |chunk, _| {
                output.write_all(chunk).map_err(|e| e.to_string())?;
                hasher.update(chunk);
                count += chunk.len() as u64;
                report(completed + count, total);
                Ok(())
            })?;
            output.sync_all().map_err(|e| e.to_string())?;
            drop(output);
            if status != 200 || count != info.size || format!("{:x}", hasher.finalize()) != info.sha256 {
                return Err(format!("downloaded {asset} does not match publisher-signed SHA-256"));
            }
            fs::rename(&partial, &path).map_err(|e| e.to_string())?;
            completed += count;
            report_file(format!("[UPDATE] Verified {asset}: size and signed SHA-256 matched"));
        }
        for name in [MANIFEST_NAME, SIGNATURE_NAME, "ready"] {
            let path = stage.join(name);
            if fs::symlink_metadata(&path).is_ok() {
                fs::remove_file(&path).map_err(|e| e.to_string())?;
            }
        }
        let mut raw = File::create_new(stage.join(MANIFEST_NAME)).map_err(|e| e.to_string())?;
        raw.write_all(&release.manifest_bytes).map_err(|e| e.to_string())?;
        raw.sync_all().map_err(|e| e.to_string())?;
        let mut sig = File::create_new(stage.join(SIGNATURE_NAME)).map_err(|e| e.to_string())?;
        sig.write_all(&release.manifest_signature).map_err(|e| e.to_string())?;
        sig.sync_all().map_err(|e| e.to_string())?;
        let mut ready = File::create_new(stage.join("ready")).map_err(|e| e.to_string())?;
        ready.write_all(b"ready-v3-per-file").map_err(|e| e.to_string())?;
        ready.sync_all().map_err(|e| e.to_string())?;
        let signed = validate_stage(&stage, install_root)?;
        preinstall(&stage, install_root, &signed)?;
        let pending = staging_root()?.join("pending.txt");
        assert_no_reparse(&pending)?;
        if fs::symlink_metadata(&pending).is_ok() { fs::remove_file(&pending).map_err(|e| e.to_string())?; }
        let mut pending_file = File::create_new(pending).map_err(|e| e.to_string())?;
        pending_file.write_all(release.tag.as_bytes()).map_err(|e| e.to_string())?;
        pending_file.sync_all().map_err(|e| e.to_string())?;
        Ok(stage.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(stage.join("ready"));
        remove_preinstalled(install_root, &manifest);
    }
    result
}

/// A copy of the running executable acts as the helper: no separate unsigned
/// binary is downloaded. The helper re-verifies staged publisher authorization.
fn launch_helper(exe: &Path, stage: &Path, pid: u32) -> Result<(), String> {
    if exe.file_name().and_then(|n| n.to_str()) != Some("DinurdoJK.exe") {
        return Err("updater will only replace DinurdoJK.exe".into());
    }
    assert_no_reparse(exe)?;
    let dir = exe.parent().ok_or("game directory not found")?;
    validate_stage(stage, dir)?;
    // Keep the helper alongside the installed game, NOT inside the writable
    // downloaded stage: Windows searches the EXE directory for dependent DLLs
    // before our Rust signature validation runs.
    assert_no_reparse(dir)?;
    let helper = dir.join(format!("DinurdoJK-Updater-{pid}.exe"));
    assert_no_reparse(&helper)?;
    if fs::symlink_metadata(&helper).is_ok() {
        fs::remove_file(&helper).map_err(|e| format!("remove stale updater helper: {e}"))?;
    }
    // A hard link is instant and is not new data for the antivirus to scan;
    // fall back to a copy where links are unavailable.
    if fs::hard_link(exe, &helper).is_err() {
        fs::copy(exe, &helper).map_err(|e| format!("copy updater helper {}: {e}", helper.display()))?;
    }
    let mut command = std::process::Command::new(&helper);
    command.arg("--djk-apply-update").arg(pid.to_string()).arg(exe).arg(stage);
    #[cfg(windows)] {
        // The helper is a console-subsystem copy of the game: without this it
        // shares (and keeps open) the game's black console window.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    if let Err(e) = command.spawn() {
        let _ = fs::remove_file(&helper);
        return Err(format!("launch updater helper {}: {e}", helper.display()));
    }
    Ok(())
}

/// Helper copies are deleted by the next game start (a running image cannot
/// delete itself). A copy that is still running simply fails to delete.
fn remove_stale_helpers() {
    let Ok(current) = std::env::current_exe() else { return };
    let Some(dir) = current.parent() else { return };
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if path != current && name.starts_with("dinurdojk-updater-") && name.ends_with(".exe")
            && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Start the game for the user. A fresh console is requested because the
/// helper's own (hidden) console would otherwise be shared, and the game only
/// shows its console when it is alone in it.
fn relaunch_game(exe: &Path, skip_pending: bool) -> Result<(), String> {
    let mut command = std::process::Command::new(exe);
    if let Some(dir) = exe.parent() { command.current_dir(dir); }
    if skip_pending { command.env(SKIP_PENDING_ENV, "1"); }
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        command.creation_flags(CREATE_NEW_CONSOLE);
    }
    command.spawn().map(|_| ()).map_err(|e| format!("relaunch {}: {e}", exe.display()))
}

pub fn restart_with_update(stage: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = exe.parent().ok_or("missing install directory")?;
    let manifest = validate_stage(stage, root)?;
    if stable_version(&manifest.tag) <= stable_version(env!("CARGO_PKG_VERSION")) {
        return Err("not a newer signed release".into());
    }
    launch_helper(&exe, stage, std::process::id())
}

pub fn apply_pending_on_launch() -> Result<bool, String> {
    remove_stale_helpers();
    if std::env::var_os(SKIP_PENDING_ENV).is_some() { return Ok(false); }
    let updater_root = match staging_root() { Ok(root) => root, Err(_) => return Ok(false) };
    if let Some(dir) = std::env::current_exe().ok().as_deref().and_then(Path::parent) {
        cleanup_leftovers(&updater_root, dir);
    }
    let result = launch_pending(&updater_root);
    if let Err(error) = &result {
        // An unusable pending marker must never be retried by every launch.
        record_failure(&updater_root, None, error);
    }
    result
}

/// Remove what a finished update leaves behind: renamed originals, unused
/// temporary copies and stage folders that are not newer than this build.
/// Nothing is touched while an update is still pending.
fn cleanup_leftovers(updater_root: &Path, install_dir: &Path) {
    if updater_root.join("pending.txt").exists() { return; }
    for dir in [install_dir.to_path_buf(), install_dir.join("DinurdoJK").join("dependencies"),
                install_dir.join("DinurdoJK").join("shared")] {
        if assert_no_reparse(&dir).is_err() { continue; }
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if (name.ends_with(OLD_SUFFIX) || name.ends_with(TMP_SUFFIX))
                && entry.file_type().is_ok_and(|kind| kind.is_file()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let Some(running) = stable_version(env!("CARGO_PKG_VERSION")) else { return };
    let Ok(entries) = fs::read_dir(updater_root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry.file_name().to_str().and_then(stable_version).is_some_and(|tag| tag <= running);
        if old && entry.file_type().is_ok_and(|kind| kind.is_dir()) && assert_no_reparse(&path).is_ok() {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

fn launch_pending(updater_root: &Path) -> Result<bool, String> {
    let tag = match fs::read_to_string(updater_root.join("pending.txt")) { Ok(tag) => tag, Err(_) => return Ok(false) };
    let stage = match stage_for(&tag) {
        Ok(stage) => stage,
        Err(_) => { let _ = fs::remove_file(updater_root.join("pending.txt")); return Ok(false); }
    };
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let install_root = exe.parent().ok_or("missing install directory")?;
    let manifest = validate_stage(&stage, install_root)?;
    let local = stable_version(env!("CARGO_PKG_VERSION")).ok_or("invalid running version")?;
    let remote = stable_version(&manifest.tag).ok_or("invalid release version")?;
    if remote <= local {
        // After a power loss, the EXE may have been atomically replaced but
        // the DLL not yet replaced. Resume ONLY when a transaction was begun
        // and this EXE matches the publisher-signed bytes exactly.
        if remote != local || !stage.join("installing").is_file() ||
            file_sha256(&std::env::current_exe().map_err(|e| e.to_string())?)? !=
                manifest.files.get("DinurdoJK.exe").ok_or("signed EXE missing")?.sha256 {
            let _ = fs::remove_file(updater_root.join("pending.txt"));
            return Ok(false);
        }
    }
    launch_helper(&exe, &stage, std::process::id())?;
    Ok(true)
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> String {
    format!("{action} {}: {error}", path.display())
}

fn write_sync(path: &Path, contents: &[u8]) -> Result<(), String> {
    assert_no_reparse(path)?;
    let mut file = File::create(path).map_err(|e| io_error("create", path, e))?;
    file.write_all(contents).map_err(|e| io_error("write", path, e))?;
    file.sync_all().map_err(|e| io_error("flush", path, e))
}

/// Rename `from` over `to` (MoveFileExW on Windows: write-through, replacing).
/// A RUNNING executable or loaded DLL cannot be overwritten or deleted, but it
/// can be renamed, which is what lets the swap work without a copy.
#[cfg(windows)]
fn move_replace(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let wide = |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() };
    let (a, b) = (wide(from), wide(to));
    if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(format!("move {} to {}: {}", from.display(), to.display(), std::io::Error::last_os_error()));
    }
    Ok(())
}
#[cfg(not(windows))]
fn move_replace(from: &Path, to: &Path) -> Result<(), String> {
    fs::rename(from, to).map_err(|e| format!("move {} to {}: {e}", from.display(), to.display()))
}

/// (target, displaced original) for each file swapped in by this run.
type Swapped = Vec<(PathBuf, Option<PathBuf>)>;

/// Put every changed file in place: dependencies first, DinurdoJK.exe last, so
/// the EXE (which decides whether a retry is still needed) is the commit point.
/// Each file is hashed once more here, right before it becomes live, closing the
/// window since it was written. Unchanged installed files are never read.
fn install_files(stage: &Path, root: &Path, manifest: &SignedManifest, swapped: &mut Swapped) -> Result<(), String> {
    let mut order: Vec<_> = manifest.files.iter().collect();
    order.sort_by_key(|(name, _)| name.as_str() == "DinurdoJK.exe");
    for (name, info) in order {
        let staged = stage.join("files").join(stage_filename(name));
        if !staged.is_file() { continue; }
        let target = root.join(name);
        let tmp = sibling(&target, TMP_SUFFIX);
        assert_no_reparse(&target)?;
        assert_no_reparse(&tmp)?;
        if !(tmp.is_file() && file_sha256(&tmp)? == info.sha256) {
            // No usable pre-installed copy: either an interrupted earlier run
            // already finished this file, or it must be rebuilt from the stage.
            if fs::metadata(&target).is_ok_and(|m| m.is_file() && m.len() == info.size)
                && file_sha256(&target)? == info.sha256 {
                let _ = fs::remove_file(&tmp);
                continue;
            }
            ensure_install_parent(root, name)?;
            copy_verified(&staged, &tmp, &info.sha256)?;
        }
        let old = if target.exists() {
            let old = sibling(&target, OLD_SUFFIX);
            if old.exists() { fs::remove_file(&old).map_err(|e| io_error("remove", &old, e))?; }
            move_replace(&target, &old)?;
            Some(old)
        } else { None };
        if let Err(error) = move_replace(&tmp, &target) {
            if let Some(old) = &old { let _ = move_replace(old, &target); }
            return Err(error);
        }
        swapped.push((target, old));
    }
    for (name, info) in &manifest.files {
        if !stage.join("files").join(stage_filename(name)).is_file() { continue; }
        if !fs::metadata(root.join(name)).is_ok_and(|m| m.len() == info.size) {
            return Err(format!("installed {name} has the wrong size"));
        }
    }
    Ok(())
}

/// Undo this run's swaps, newest first, by renaming the originals back.
fn undo_swaps(swapped: &Swapped) -> Result<(), String> {
    let mut failure = None;
    for (target, old) in swapped.iter().rev() {
        let result = match old {
            Some(old) => move_replace(old, target),
            None => fs::remove_file(target).map_err(|e| io_error("remove", target, e)),
        };
        if let Err(error) = result { failure = Some(error); }
    }
    failure.map_or(Ok(()), Err)
}

fn apply_update(exe: &Path, stage: &Path, pid: u32) -> Result<(), String> {
    if exe.file_name().and_then(|n| n.to_str()) != Some("DinurdoJK.exe") {
        return Err("unexpected installation target".into());
    }
    let root = exe.parent().ok_or("missing install directory")?;
    // The helper is a link/copy of the game image placed beside it. It acts
    // only on the installation it lives in, whatever the command line says.
    let helper_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if !same_dir(&helper_exe, exe) { return Err("updater helper is not in the game directory".into()); }
    assert_no_reparse(&helper_exe)?;
    assert_no_reparse(exe)?;
    assert_no_reparse(root)?;
    let manifest = validate_stage(stage, root)?;
    let installed = stable_version(env!("CARGO_PKG_VERSION")).ok_or("invalid helper version")?;
    if stable_version(&manifest.tag) <= Some(installed) {
        // Existing helper is a copy of the pre-update EXE; a retry can carry
        // the post-update version only if a journal exists.
        if stable_version(&manifest.tag) != Some(installed) || !stage.join("installing").is_file() {
            return Err("refusing same-version or downgrade installation".into());
        }
    }
    wait_for_process(pid)?;
    // Recheck trust after the game exits, not only when Restart was clicked.
    let manifest = validate_stage(stage, root)?;
    // Crash recovery is roll-forward with the publisher-signed files; nothing
    // is ever restored from a disk-only journal.
    let resumed = stage.join("installing").is_file();
    write_sync(&stage.join("installing"), b"signed-update-transaction-v2")?;
    let mut swapped = Swapped::new();
    if let Err(error) = install_files(stage, root, &manifest, &mut swapped) {
        return Err(match undo_swaps(&swapped) {
            Ok(()) => {
                if !resumed {
                    let _ = fs::remove_file(stage.join("installing"));
                    if let Ok(updates) = staging_root() { let _ = fs::remove_file(updates.join("pending.txt")); }
                }
                error
            }
            // Leave pending + journal so a later launch can roll forward.
            Err(undo) => format!("{error}; rollback also failed: {undo}"),
        });
    }
    // Commit before starting the new process. Otherwise its startup could
    // race pending.txt and launch a second installer for the same release.
    if let Ok(updates) = staging_root() { let _ = fs::remove_file(updates.join("pending.txt")); }
    let _ = fs::remove_file(stage.join("installing"));
    let _ = fs::remove_file(stage.join("ready"));
    relaunch_game(exe, false)
        .map_err(|e| format!("update installed; start DinurdoJK manually: {e}"))
}

/// Called by the helper after a failed install. Records the error for the game
/// to show and makes sure the failure cannot repeat on every launch:
/// - nothing was modified (no journal): the pending marker is dropped;
/// - a journalled install is rolled forward on a later launch, at most
///   MAX_RESUME_ATTEMPTS times, then the stage is abandoned for a fresh download.
fn record_failure(root: &Path, stage: Option<&Path>, error: &str) {
    let _ = fs::create_dir_all(root);
    let _ = fs::write(root.join(LAST_ERROR_NAME), error);
    let pending = root.join("pending.txt");
    let stage = stage.filter(|stage| stage.parent() == Some(root));
    let Some(stage) = stage.filter(|stage| stage.join("installing").is_file()) else {
        let _ = fs::remove_file(&pending);
        return;
    };
    let attempts_path = stage.join("attempts");
    let attempts = fs::read_to_string(&attempts_path).ok()
        .and_then(|text| text.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
    let _ = fs::write(&attempts_path, attempts.to_string());
    if attempts >= MAX_RESUME_ATTEMPTS {
        let _ = fs::remove_file(&pending);
        for name in ["installing", "ready", "attempts"] {
            let _ = fs::remove_file(stage.join(name));
        }
    }
}

pub fn helper_entry() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--djk-apply-update")) { return None; }
    let parsed = (|| -> Result<(u32, PathBuf, PathBuf), String> {
        let pid = args.next().and_then(|p| p.to_string_lossy().parse::<u32>().ok()).ok_or("missing process ID")?;
        let exe = PathBuf::from(args.next().ok_or("missing game path")?);
        let stage = PathBuf::from(args.next().ok_or("missing stage path")?);
        if args.next().is_some() { return Err("unexpected helper arguments".into()); }
        Ok((pid, exe, stage))
    })();
    let result = match &parsed {
        Ok((pid, exe, stage)) => apply_update(exe, stage, *pid),
        Err(error) => Err(error.clone()),
    };
    if let Err(error) = &result {
        if let Ok(root) = staging_root() {
            record_failure(&root, parsed.as_ref().ok().map(|(_, _, stage)| stage.as_path()), error);
        }
        // Never leave the user without a game. Only the sibling DinurdoJK.exe
        // of this helper copy is started, and it must skip the updater once.
        if let (Ok((pid, exe, _)), Ok(helper)) = (&parsed, std::env::current_exe()) {
            if process_exited(*pid) && exe.file_name().and_then(|n| n.to_str()) == Some("DinurdoJK.exe")
                && exe.parent() == helper.parent() && exe.is_file() {
                let _ = relaunch_game(exe, true);
            }
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
/// True when the process is gone (or cannot be opened): a hung game must not
/// get a second instance started next to it.
#[cfg(windows)]
fn process_exited(pid: u32) -> bool {
    use windows_sys::Win32::{Foundation::{CloseHandle, WAIT_OBJECT_0}, System::Threading::{OpenProcess, WaitForSingleObject}};
    let handle = unsafe { OpenProcess(0x0010_0000, 0, pid) };
    if handle.is_null() { return true; }
    let status = unsafe { WaitForSingleObject(handle, 0) };
    unsafe { CloseHandle(handle) };
    status == WAIT_OBJECT_0
}
#[cfg(not(windows))]
fn process_exited(_pid: u32) -> bool { true }

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
            "format": 2,
            "tag": "v0.2.0",
            "files": {"DinurdoJK.exe": {"size": 3, "sha256": "b".repeat(64)}}
        });
        let raw = serde_json::to_vec(&manifest).unwrap();
        let signature = key.sign(&raw).to_bytes().to_vec();
        (raw, signature, key.verifying_key())
    }

    #[test]
    fn unexpected_signed_filename_is_reported_without_weakening_the_allowlist() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["files"]["unapproved/secret.dll"] = serde_json::json!({"size":3,"sha256":"b".repeat(64)});
        let raw = serde_json::to_vec(&value).unwrap();
        let error = parse_signed_manifest_with_key(&raw, &signing.sign(&raw).to_bytes(), &key).unwrap_err();
        assert!(error.contains("unapproved/secret.dll"), "{error}");
    }

    #[test]
    fn file_decisions_distinguish_missing_size_hash_and_identical() {
        let root=std::env::temp_dir().join(format!("djk-file-reason-{}-{:?}",std::process::id(),std::thread::current().id()));
        let name="DinurdoJK/dependencies/dxil.dll";
        let path=root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _=fs::remove_file(&path);
        let expected=b"expected data";
        let info=FileInfo {size:expected.len() as u64,sha256:format!("{:x}",Sha256::digest(expected))};
        assert_eq!(installed_file_status(&root,name,&info).unwrap(),InstalledFileStatus::Missing);
        fs::write(&path,b"short").unwrap();
        assert!(matches!(installed_file_status(&root,name,&info).unwrap(),InstalledFileStatus::SizeMismatch{..}));
        fs::write(&path,b"differentdata").unwrap();
        assert!(matches!(installed_file_status(&root,name,&info).unwrap(),InstalledFileStatus::HashMismatch{..}));
        fs::write(&path,expected).unwrap();
        assert_eq!(installed_file_status(&root,name,&info).unwrap(),InstalledFileStatus::Identical);
        fs::remove_dir_all(root).unwrap();
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
    fn signed_runtime_dependencies_are_accepted_without_widening_install_paths() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        for name in DEPENDENCY_RELEASE_FILES {
            value["files"][format!("DinurdoJK/dependencies/{name}")] =
                serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        }
        let raw = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&raw, &signing.sign(&raw).to_bytes(), &key).is_ok());
        value["files"].as_object_mut().unwrap().remove("DinurdoJK/dependencies/phonon.dll");
        value["files"]["../OpenALSoft.x86_64.dll"] = serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        let raw = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&raw, &signing.sign(&raw).to_bytes(), &key).is_err());
    }

    #[test]
    fn signed_dxc_files_are_allowed_only_under_dependencies() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        for name in ["dxcompiler.dll", "dxil.dll"] {
            value["files"][format!("DinurdoJK/dependencies/{name}")] = serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        }
        let valid = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&valid, &signing.sign(&valid).to_bytes(), &key).is_ok());
        assert_eq!(stage_filename("DinurdoJK/dependencies/dxcompiler.dll"), "deps__dxcompiler.dll");
        assert_eq!(signed_name_from_stage("deps__dxcompiler.dll"), "DinurdoJK/dependencies/dxcompiler.dll");
        assert_eq!(release_asset_name("DinurdoJK/dependencies/dxcompiler.dll"), "dxcompiler.dll");
        for bad in ["dxcompiler.dll", "bin/dxcompiler.dll", "DinurdoJK/dependencies/../phonon.dll",
                    "DXC-LICENSE.txt", "DXC-ThirdPartyNotices.txt"] {
            let mut rejected = value.clone();
            rejected["files"][bad] = serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
            let invalid = serde_json::to_vec(&rejected).unwrap();
            assert!(parse_signed_manifest_with_key(&invalid, &signing.sign(&invalid).to_bytes(), &key).is_err());
        }
    }

    #[test]
    fn packager_file_layout_is_accepted_without_allowing_arbitrary_paths() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut manifest: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        for name in DEPENDENCY_RELEASE_FILES {
            manifest["files"][format!("DinurdoJK/dependencies/{name}")] =
                serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        }
        manifest["files"]["DinurdoJK/shared/dinurdo-assets.pk4"] =
            serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
        let encoded = serde_json::to_vec(&manifest).unwrap();
        assert!(parse_signed_manifest_with_key(&encoded, &signing.sign(&encoded).to_bytes(), &key).is_ok());
        for bad in ["DXC-LICENSE.txt", "OpenAL-Soft-LGPL-2.0.txt", "NVIDIA-DLSS-SDK-LICENSE.txt",
            "EalMan-LGPL-2.1.txt", "Bevy-Solari-MIT.txt", "DXC-ThirdPartyNotices.txt",
            "FFmpeg-GPL-3.0.txt", "FFmpeg-SOURCE-NOTICE.txt", "licenses/DXC-LICENSE.txt",
            "DXC-LICENSE.txt/evil", "DinurdoJK/dependencies/DXC-LICENSE.txt",
            "dxcompiler.dll", "vcruntime140.dll", "DinurdoJK/shared/../evil.pk4"] {
            let mut invalid = manifest.clone();
            // Keep the same file count so rejection checks the path, not the limit.
            invalid["files"].as_object_mut().unwrap().remove("DinurdoJK/dependencies/phonon.dll");
            invalid["files"][bad] = serde_json::json!({"size": 3, "sha256": "b".repeat(64)});
            let data = serde_json::to_vec(&invalid).unwrap();
            assert!(parse_signed_manifest_with_key(&data, &signing.sign(&data).to_bytes(), &key).is_err());
        }
    }
    #[test]
    fn dependencies_require_new_install_directory() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["files"]["DinurdoJK/dependencies/ffmpeg.exe"] =
            serde_json::json!({"size": 5, "sha256": "c".repeat(64)});
        let valid = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&valid, &signing.sign(&valid).to_bytes(), &key).is_ok());
        for old in ["dxcompiler.dll", "nvngx_dlss.dll", "ffmpeg.exe", "DinurdoJK/other/phonon.dll"] {
            let mut outdated = value.clone();
            outdated["files"][old] = serde_json::json!({"size": 5, "sha256": "c".repeat(64)});
            let raw = serde_json::to_vec(&outdated).unwrap();
            assert!(parse_signed_manifest_with_key(&raw, &signing.sign(&raw).to_bytes(), &key).is_err());
        }
    }

    #[test]
    fn signed_project_asset_is_allowed_and_other_subdirectories_are_rejected() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["files"]["DinurdoJK/shared/dinurdo-assets.pk4"] =
            serde_json::json!({"size": 5, "sha256": "c".repeat(64)});
        let valid = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&valid, &signing.sign(&valid).to_bytes(), &key).is_ok());
        assert_eq!(stage_filename("DinurdoJK/shared/dinurdo-assets.pk4"), "shared__dinurdo-assets.pk4");
        assert_eq!(release_asset_name("DinurdoJK/shared/dinurdo-assets.pk4"), "dinurdo-assets.pk4");
        assert_eq!(signed_name_from_stage("shared__dinurdo-assets.pk4"), "DinurdoJK/shared/dinurdo-assets.pk4");
        assert_eq!(release_asset_name("DinurdoJK/dependencies/dxcompiler.dll"), "dxcompiler.dll");
        assert_eq!(stage_filename("DinurdoJK/dependencies/nvngx_dlss.dll"), "deps__nvngx_dlss.dll");
        assert_eq!(signed_name_from_stage("deps__OpenALSoft.x86_64.dll"), "DinurdoJK/dependencies/OpenALSoft.x86_64.dll");
        // Do not re-allow previous destinations; no migration or fallback.
        for old_path in ["DinurdoJK/dinurdo-assets.pk3", "base/dinurdo-assets.pk3", "DinurdoJK/shared/dinurdo-assets.pk3"] {
            let mut outdated = value.clone();
            outdated["files"].as_object_mut().unwrap().remove("DinurdoJK/shared/dinurdo-assets.pk4");
            outdated["files"][old_path] =
                serde_json::json!({"size": 5, "sha256": "c".repeat(64)});
            let invalid = serde_json::to_vec(&outdated).unwrap();
            assert!(parse_signed_manifest_with_key(&invalid, &signing.sign(&invalid).to_bytes(), &key).is_err());
        }
        value["files"]["base/../../evil.dll"] =
            serde_json::json!({"size": 5, "sha256": "c".repeat(64)});
        let invalid = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&invalid, &signing.sign(&invalid).to_bytes(), &key).is_err());
    }

    #[test]
    fn old_archive_manifest_format_is_not_accepted_as_per_file_release() {
        let signing = SigningKey::from_bytes(&[41; 32]);
        let (raw, _, key) = signed_fixture();
        let mut value: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        value["format"] = serde_json::json!(1);
        let invalid = serde_json::to_vec(&value).unwrap();
        assert!(parse_signed_manifest_with_key(&invalid, &signing.sign(&invalid).to_bytes(), &key).is_err());
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

#[cfg(test)]
mod install_tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(name: &str) -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "djk-updater-test-{}-{}-{name}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }

    fn info(bytes: &[u8]) -> FileInfo { FileInfo { size: bytes.len() as u64, sha256: sha(bytes) } }

    // An install root with an old EXE and dependency, plus a stage holding the new ones.
    fn fixture(name: &str) -> (PathBuf, PathBuf, SignedManifest) {
        let root = scratch(name);
        let install = root.join("game");
        let stage = root.join("updates").join("v0.2.0");
        fs::create_dir_all(install.join("DinurdoJK").join("dependencies")).unwrap();
        fs::create_dir_all(stage.join("files")).unwrap();
        fs::write(install.join("DinurdoJK.exe"), b"old exe").unwrap();
        fs::write(install.join("DinurdoJK/dependencies/phonon.dll"), b"old dll").unwrap();
        fs::write(stage.join("files").join("DinurdoJK.exe"), b"new exe").unwrap();
        fs::write(stage.join("files").join("deps__phonon.dll"), b"new dll").unwrap();
        let manifest = SignedManifest {
            format: 2,
            tag: "v0.2.0".into(),
            files: BTreeMap::from([
                ("DinurdoJK.exe".to_owned(), info(b"new exe")),
                ("DinurdoJK/dependencies/phonon.dll".to_owned(), info(b"new dll")),
            ]),
        };
        (install, stage, manifest)
    }

    #[test]
    fn verified_copy_flushes_and_rejects_a_wrong_hash() {
        let dir = scratch("copy");
        let (source, target) = (dir.join("new.bin"), dir.join("new.bin.djk-update-tmp"));
        fs::write(&source, b"new").unwrap();
        copy_verified(&source, &target, &sha(b"new")).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert!(copy_verified(&source, &target, &sha(b"other")).is_err());
        assert!(!target.exists(), "a rejected copy must not be left behind");
    }

    #[test]
    fn preinstalled_files_are_swapped_in_with_the_exe_last_and_originals_renamed() {
        let (install, stage, manifest) = fixture("swap");
        preinstall(&stage, &install, &manifest).unwrap();
        assert!(install.join("DinurdoJK.exe.djk-update-tmp").is_file());
        assert_eq!(fs::read(install.join("DinurdoJK.exe")).unwrap(), b"old exe", "nothing is live yet");
        let mut swapped = Swapped::new();
        install_files(&stage, &install, &manifest, &mut swapped).unwrap();
        assert_eq!(fs::read(install.join("DinurdoJK.exe")).unwrap(), b"new exe");
        assert_eq!(fs::read(install.join("DinurdoJK/dependencies/phonon.dll")).unwrap(), b"new dll");
        assert_eq!(fs::read(install.join("DinurdoJK.exe.djk-old")).unwrap(), b"old exe");
        assert_eq!(swapped.len(), 2);
        assert_eq!(swapped.last().unwrap().0, install.join("DinurdoJK.exe"));
        assert!(!install.join("DinurdoJK.exe.djk-update-tmp").exists());
        undo_swaps(&swapped).unwrap();
        assert_eq!(fs::read(install.join("DinurdoJK.exe")).unwrap(), b"old exe");
        assert_eq!(fs::read(install.join("DinurdoJK/dependencies/phonon.dll")).unwrap(), b"old dll");
    }

    #[test]
    fn missing_or_tampered_preinstalled_copy_is_rebuilt_from_the_stage() {
        let (install, stage, manifest) = fixture("rebuild");
        // The EXE copy was never written; the dependency copy was tampered with.
        fs::write(install.join("DinurdoJK/dependencies/phonon.dll.djk-update-tmp"), b"evil").unwrap();
        let mut swapped = Swapped::new();
        install_files(&stage, &install, &manifest, &mut swapped).unwrap();
        assert_eq!(fs::read(install.join("DinurdoJK.exe")).unwrap(), b"new exe");
        assert_eq!(fs::read(install.join("DinurdoJK/dependencies/phonon.dll")).unwrap(), b"new dll");
    }

    #[test]
    fn a_failed_install_leaves_the_old_executable_untouched() {
        let (install, stage, manifest) = fixture("failed");
        // Corrupt the staged dependency: it is installed before the EXE, so the
        // EXE must not have been swapped when this fails.
        fs::write(stage.join("files").join("deps__phonon.dll"), b"bad dll").unwrap();
        let mut swapped = Swapped::new();
        assert!(install_files(&stage, &install, &manifest, &mut swapped).is_err());
        assert!(swapped.is_empty());
        assert_eq!(fs::read(install.join("DinurdoJK.exe")).unwrap(), b"old exe");
        assert_eq!(fs::read(install.join("DinurdoJK/dependencies/phonon.dll")).unwrap(), b"old dll");
    }

    #[test]
    fn a_file_finished_by_an_interrupted_run_is_not_installed_twice() {
        let (install, stage, manifest) = fixture("resume");
        fs::write(install.join("DinurdoJK/dependencies/phonon.dll"), b"new dll").unwrap();
        let mut swapped = Swapped::new();
        install_files(&stage, &install, &manifest, &mut swapped).unwrap();
        assert_eq!(swapped.len(), 1, "only the EXE still needed swapping");
        assert_eq!(fs::read(install.join("DinurdoJK.exe")).unwrap(), b"new exe");
    }

    #[test]
    fn cleanup_removes_leftovers_only_when_no_update_is_pending() {
        let (install, stage, _) = fixture("cleanup");
        let updates = stage.parent().unwrap().to_path_buf();
        fs::create_dir_all(updates.join("v0.0.1")).unwrap();
        fs::create_dir_all(updates.join("v99.0.0")).unwrap();
        fs::write(install.join("DinurdoJK.exe.djk-old"), b"x").unwrap();
        fs::write(install.join("DinurdoJK/dependencies/phonon.dll.djk-update-tmp"), b"x").unwrap();
        fs::write(updates.join("pending.txt"), "v99.0.0").unwrap();
        cleanup_leftovers(&updates, &install);
        assert!(install.join("DinurdoJK.exe.djk-old").exists(), "pending update keeps its files");
        fs::remove_file(updates.join("pending.txt")).unwrap();
        cleanup_leftovers(&updates, &install);
        assert!(!install.join("DinurdoJK.exe.djk-old").exists());
        assert!(!install.join("DinurdoJK/dependencies/phonon.dll.djk-update-tmp").exists());
        assert!(install.join("DinurdoJK.exe").exists() && install.join("DinurdoJK/dependencies/phonon.dll").exists());
        assert!(!updates.join("v0.0.1").exists(), "a stage older than this build is removed");
        assert!(updates.join("v99.0.0").exists(), "a newer stage is kept");
    }

    #[test]
    fn failure_before_any_change_drops_the_pending_marker() {
        let (_, stage, _) = fixture("nojournal");
        let root = stage.parent().unwrap().to_path_buf();
        fs::write(root.join("pending.txt"), "v0.2.0").unwrap();
        record_failure(&root, Some(&stage), "Access is denied. (os error 5)");
        assert!(!root.join("pending.txt").exists());
        assert_eq!(take_last_error(&root).unwrap(), "Access is denied. (os error 5)");
        assert!(take_last_error(&root).is_none(), "the error is shown once");
    }

    #[test]
    fn interrupted_install_is_resumed_a_bounded_number_of_times() {
        let (_, stage, _) = fixture("journal");
        let root = stage.parent().unwrap().to_path_buf();
        fs::write(root.join("pending.txt"), "v0.2.0").unwrap();
        fs::write(stage.join("installing"), b"journal").unwrap();
        fs::write(stage.join("ready"), b"ready").unwrap();
        record_failure(&root, Some(&stage), "first");
        assert!(root.join("pending.txt").exists(), "first failure keeps the roll-forward marker");
        record_failure(&root, Some(&stage), "second");
        assert!(!root.join("pending.txt").exists());
        assert!(!stage.join("installing").exists() && !stage.join("ready").exists());
    }

    #[test]
    fn a_stage_outside_the_updater_root_is_never_touched() {
        let (_, stage, _) = fixture("outside");
        let root = scratch("other-root");
        fs::write(root.join("pending.txt"), "v0.2.0").unwrap();
        fs::write(stage.join("installing"), b"journal").unwrap();
        record_failure(&root, Some(&stage), "boom");
        assert!(stage.join("installing").exists());
        assert!(!root.join("pending.txt").exists());
    }

    #[test]
    fn state_is_separate_per_installation_directory() {
        assert_ne!(install_key(Path::new(r"C:\Games\JK")), install_key(Path::new(r"D:\Games\JK")));
        assert_eq!(install_key(Path::new(r"C:\Games\JK")), install_key(Path::new(r"c:\games\jk")));
    }
}
