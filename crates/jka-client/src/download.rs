//! Shared JKA/JK2 package autodownload support.
//!
//! The server advertises referenced PK3 checksums/names in systeminfo.  We
//! download missing packages rather than a bare BSP so the map arrives with its
//! shaders, textures and other referenced assets, matching OpenJK/JK2MV's
//! CL_InitDownloads / FS_ComparePaks model.

use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Component, Path, PathBuf},
    collections::HashSet,
    sync::mpsc::Sender,
    time::Duration,
};

use jka_protocol::commands::info_value;

const MAX_HTTP_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct DownloadSpec {
    /// Server qpath without the implicit .pk3 suffix, e.g. `japro/maps1`.
    pub referenced_name: String,
    /// Requested qpath, e.g. `japro/maps1.pk3`.
    pub remote_name: String,
    /// Destination under GameData. Downloaded files deliberately use dl_ names,
    /// like OpenJK, so an existing user-installed PK3 is never overwritten.
    pub local_path: PathBuf,
    pub checksum: i32,
}

#[derive(Debug)]
pub enum HttpEvent {
    Started { total: Option<u64> },
    Progress { received: u64, total: Option<u64> },
    Finished(Result<(), String>),
}

/// Match OpenJK FS_PureServerSetReferencedPaks: pairs of checksum and name
/// delimit the authoritative list. Ignore excess checksums without a name;
/// use signed 32-bit values because the wire protocol prints them as integers.
/// This same list drives JKA/JK2 mount gating (also when sv_pure is off).
fn server_referenced_packages(systeminfo: &[u8]) -> Vec<(i32, String)> {
    let checksums = info_value(systeminfo, b"sv_referencedPaks").unwrap_or_default();
    let names = info_value(systeminfo, b"sv_referencedPakNames").unwrap_or_default();
    std::str::from_utf8(checksums).unwrap_or_default().split_whitespace()
        .zip(std::str::from_utf8(names).unwrap_or_default().split_whitespace())
        .filter_map(|(checksum, name)| checksum.parse::<i32>().ok()
            .map(|checksum| (checksum, name.to_owned())))
        .collect()
}

pub fn server_referenced_checksums(systeminfo: &[u8]) -> Vec<i32> {
    server_referenced_packages(systeminfo).into_iter().map(|(checksum, _)| checksum).collect()
}

pub fn referenced_downloads(
    base_dir: &Path,
    active_game_dir: Option<&Path>,
    systeminfo: &[u8],
) -> Result<Vec<DownloadSpec>, String> {
    let referenced_packages = server_referenced_packages(systeminfo);
    if referenced_packages.is_empty() {
        return Ok(Vec::new());
    }

    // PK3 downloads follow the *active game's* writable root. JK2 must never
    // fall through to JKA's GameData/base or fs_homepath; both games share the
    // downloader, not their asset namespace. This also respects fs_jk2homepath
    // (and fs_homepath for JKA), but never writes into CD or fs_forcegame.
    let roots = jka_assets::pk3::configured_filesystem_roots(base_dir);
    let download_root = download_root_for(base_dir, &roots);
    // The server's mod name is its literal directory name (no prefixes).
    let active_name = jka_assets::pk3::fs_game_name(active_game_dir);
    let active_name = active_name.as_str();

    let local_checksums = local_pk3_checksums(base_dir, active_game_dir)?;

    let mut result = Vec::new();
    for (checksum, referenced) in referenced_packages {
        let Some((game, pak)) = validate_referenced_name(&referenced) else {
            continue;
        };
        // Never fetch stock engine assets. JKA and JK2 have different
        // asset packs; JK2MV additionally protects assets5 and assetsmv.
        if game.eq_ignore_ascii_case("base") && is_stock_assets(pak, roots.active_jk2.is_some()) {
            continue;
        }
        // A remote server may only write into base or its active fs_game. This
        // prevents a malicious referencedPakNames value from creating arbitrary
        // GameData subdirectories.
        if !game.eq_ignore_ascii_case("base") && !game.eq_ignore_ascii_case(active_name) {
            continue;
        }

        // OpenJK matches server referenced packages by checksum rather than
        // filename.  The right PK3 may already be installed under another
        // name, while a same-named local PK3 may be the wrong version.
        if local_checksums.contains(&checksum) {
            continue;
        }

        let game_dir = download_root.join(game);
        let downloaded = game_dir.join(format!("dl_{pak}.pk3"));
        let local_path = if downloaded.exists() {
            game_dir.join(format!("dl_{pak}.{:08x}.pk3", checksum as u32))
        } else {
            downloaded
        };
        result.push(DownloadSpec {
            referenced_name: format!("{game}/{pak}"),
            remote_name: format!("{game}/{pak}.pk3"),
            local_path,
            checksum,
        });
    }
    Ok(result)
}


fn local_pk3_checksums(base_dir: &Path, active_game_dir: Option<&Path>) -> Result<HashSet<i32>, String> {
    // All installed, eligible PK3s count as locally available, including
    // unmounted `dl_` archives. Avoid re-downloading from fs_cdpath/other roots.
    let roots = jka_assets::pk3::configured_filesystem_roots(base_dir);
    let dirs = jka_assets::pk3::filesystem_search_dirs(base_dir, active_game_dir, &roots);

    let mut checksums = HashSet::new();
    for dir in dirs {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("scan {} for PK3s: {error}", dir.display())),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|kind| kind.is_file())
                || !jka_assets::pk3::package_available_for_download(&path)
                || !path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("pk3"))
            {
                continue;
            }
            match jka_assets::pak_checksum::pk3_checksum(&path) {
                Ok(checksum) => { checksums.insert(checksum); }
                Err(error) => crate::logging::write_line_with_path(
                    crate::logging::Level::Error,
                    format_args!("[DOWNLOAD] ignoring unreadable PK3 {}: {error}", path.display()),
                    path.clone(),
                ),
            }
        }
    }
    Ok(checksums)
}

pub fn verify_pk3_checksum(path: &Path, expected: i32) -> Result<(), String> {
    let actual = jka_assets::pak_checksum::pk3_checksum(path)
        .map_err(|error| format!("could not validate downloaded PK3 {}: {error}", path.display()))?;
    if actual != expected {
        return Err(format!(
            "downloaded PK3 checksum mismatch for {} (server {:08x}, local {:08x})",
            path.display(), expected as u32, actual as u32
        ));
    }
    Ok(())
}

fn validate_referenced_name(name: &str) -> Option<(&str, &str)> {
    if name.is_empty() || name.contains('\\') || name.contains(':') || name.contains('@') {
        return None;
    }
    let path = Path::new(name);
    if path.components().any(|part| !matches!(part, Component::Normal(_))) {
        return None;
    }
    let mut parts = name.split('/');
    let game = parts.next()?;
    let pak = parts.next()?;
    if parts.next().is_some() || game.is_empty() || pak.is_empty() || pak.ends_with(".pk3") {
        return None;
    }
    Some((game, pak))
}

fn is_stock_assets(name: &str, jk2: bool) -> bool {
    let name = name.to_ascii_lowercase();
    if jk2 {
        // JK2MV FS_idPakPath: never fetch stock JK2 assets or assetsmv.
        matches!(name.as_str(), "assets0" | "assets1" | "assets2" | "assets5" | "assetsmv")
    } else {
        matches!(name.as_str(), "assets0" | "assets1" | "assets2" | "assets3")
    }
}

fn download_root_for(base_dir: &Path, roots: &jka_assets::pk3::FilesystemRoots) -> PathBuf {
    // filesystem_write_directory owns the JKA/JK2 root, including optional
    // game-specific homepath. Asking for base avoids deriving an install root
    // from self.game (which may point at a physically different mod layer).
    let base_write = jka_assets::pk3::filesystem_write_directory(base_dir, None, roots);
    base_write.parent().unwrap_or(base_write.as_path()).to_path_buf()
}

/// JK2MV reads sv_allowDownload from CS_SERVERINFO and requires a nonzero
/// value. OpenJK/JKA's historical fallback is preserved for servers which
/// omit the key. HTTP is advertised separately by getinfo's mvhttp keys.
pub fn server_allows_udp(serverinfo: &[u8], jk2: bool) -> bool {
    match info_value(serverinfo, b"sv_allowDownload") {
        Some(value) => jka_protocol::commands::atoi(value) != 0,
        None => !jk2,
    }
}

/// TaystJK advertises its usable HTTP endpoint in the connectionless
/// `infoResponse`: `mvhttp` is the actual bound TCP port on the game-server
/// host, while `mvhttpurl` is an external absolute `http://` root.  The server
/// cvar `sv_httpServerPort` is configuration input and is not itself advertised.
pub fn advertised_http_base(
    info: &jka_protocol::ServerInfo,
    server: SocketAddr,
) -> Option<String> {
    if let Some(port) = info
        .get(b"mvhttp")
        .and_then(|value| String::from_utf8_lossy(value).trim().parse::<u16>().ok())
        .filter(|port| *port != 0)
    {
        let host = match server.ip() {
            std::net::IpAddr::V4(ip) => ip.to_string(),
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        };
        return Some(format!("http://{host}:{port}"));
    }

    if let Some(endpoint) = info.get(b"mvhttpurl") {
        let endpoint = String::from_utf8_lossy(endpoint);
        let endpoint = endpoint.trim().trim_end_matches('/');
        if endpoint
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
        {
            return Some(format!("http://{}", &endpoint[7..]));
        }
    }

    // Compatibility with experimental/third-party servers that expose their
    // configuration value directly. Current TaystJK does not use this key in
    // infoResponse, so it must never be required for HTTP autodownload.
    let endpoint = String::from_utf8_lossy(info.get(b"sv_httpServerPort")?)
        .trim()
        .trim_end_matches('/')
        .to_owned();
    if endpoint
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
    {
        return Some(format!("http://{}", &endpoint[7..]));
    }

    let port = endpoint.parse::<u16>().ok().filter(|port| *port != 0)?;
    let host = match server.ip() {
        std::net::IpAddr::V4(ip) => ip.to_string(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
    };
    Some(format!("http://{host}:{port}"))
}

pub fn spawn_http_download(base_url: String, spec: DownloadSpec, tx: Sender<HttpEvent>) {
    let _ = std::thread::Builder::new()
        .name("map-http-download".into())
        .spawn(move || {
            let result = http_download(&base_url, &spec, &tx);
            let _ = tx.send(HttpEvent::Finished(result));
        });
}

fn http_download(base_url: &str, spec: &DownloadSpec, tx: &Sender<HttpEvent>) -> Result<(), String> {
    let target = format!("{}/{}", base_url.trim_end_matches('/'), encode_qpath(&spec.remote_name));
    let (host, port, path) = parse_http_url(&target)?;
    let mut stream = TcpStream::connect((host.as_str(), port))
        .map_err(|error| format!("HTTP connect to {host}:{port} failed: {error}"))?;
    stream.set_read_timeout(Some(HTTP_TIMEOUT)).map_err(|error| error.to_string())?;
    stream.set_write_timeout(Some(HTTP_TIMEOUT)).map_err(|error| error.to_string())?;
    let header_host = if host.contains(':') { format!("[{host}]") } else { host.clone() };
    let host_header = if port == 80 { header_host } else { format!("{header_host}:{port}") };
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: DinurdoJK/0.1\r\nAccept: application/octet-stream\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| format!("HTTP request failed: {error}"))?;
    stream.flush().map_err(|error| format!("HTTP request failed: {error}"))?;

    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).map_err(|error| format!("HTTP status failed: {error}"))?;
    let status_code = status.split_whitespace().nth(1).and_then(|code| code.parse::<u16>().ok());
    if status_code != Some(200) {
        return Err(format!("HTTP server returned {} for {}", status.trim(), spec.remote_name));
    }

    let mut content_length = None;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|error| format!("HTTP headers failed: {error}"))?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse::<u64>().ok();
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    if content_length.is_some_and(|len| len > MAX_HTTP_DOWNLOAD_BYTES) {
        return Err("HTTP package exceeds the 2 GiB download safety limit".into());
    }

    if let Some(parent) = spec.local_path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let temp_path = temp_path_for(&spec.local_path);
    let mut file = File::create(&temp_path).map_err(|error| format!("create {}: {error}", temp_path.display()))?;
    let _ = tx.send(HttpEvent::Started { total: content_length });
    let mut received = 0u64;

    let transfer = if chunked {
        copy_chunked(&mut reader, &mut file, &mut received, tx)
    } else {
        copy_body(&mut reader, &mut file, content_length, &mut received, tx)
    };
    if let Err(error) = transfer {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    file.flush().map_err(|error| format!("flush {}: {error}", temp_path.display()))?;
    drop(file);
    if let Some(expected) = content_length {
        if received != expected {
            let _ = fs::remove_file(&temp_path);
            return Err(format!("HTTP package was truncated ({received}/{expected} bytes)"));
        }
    }
    if let Err(error) = verify_pk3_checksum(&temp_path, spec.checksum) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    fs::rename(&temp_path, &spec.local_path)
        .map_err(|error| format!("install {}: {error}", spec.local_path.display()))?;
    Ok(())
}

fn copy_body<R: Read>(
    reader: &mut R,
    file: &mut File,
    expected: Option<u64>,
    received: &mut u64,
    tx: &Sender<HttpEvent>,
) -> Result<(), String> {
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(|error| format!("HTTP body failed: {error}"))?;
        if count == 0 { break; }
        *received += count as u64;
        if *received > MAX_HTTP_DOWNLOAD_BYTES {
            return Err("HTTP package exceeds the 2 GiB download safety limit".into());
        }
        file.write_all(&buffer[..count]).map_err(|error| format!("write download: {error}"))?;
        let _ = tx.send(HttpEvent::Progress { received: *received, total: expected });
    }
    Ok(())
}

fn copy_chunked<R: BufRead>(reader: &mut R, file: &mut File, received: &mut u64, tx: &Sender<HttpEvent>) -> Result<(), String> {
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|error| format!("HTTP chunk header failed: {error}"))?;
        let size = line.trim().split(';').next().and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .ok_or_else(|| "HTTP server sent an invalid chunk length".to_owned())?;
        if size == 0 { break; }
        if *received + size > MAX_HTTP_DOWNLOAD_BYTES {
            return Err("HTTP package exceeds the 2 GiB download safety limit".into());
        }
        let mut limited = (&mut *reader).take(size);
        let copied = std::io::copy(&mut limited, file).map_err(|error| format!("HTTP chunk failed: {error}"))?;
        if copied != size { return Err("HTTP package was truncated in a chunk".into()); }
        *received += copied;
        let _ = tx.send(HttpEvent::Progress { received: *received, total: None });
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(|error| format!("HTTP chunk trailer failed: {error}"))?;
        if crlf != *b"\r\n" { return Err("HTTP server sent an invalid chunk trailer".into()); }
    }
    Ok(())
}

fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("http://"))
        .map(|_| &url[7..])
        .ok_or_else(|| "only http:// autodownload URLs are supported".to_owned())?;
    let (authority, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(a, p)| (a, format!("/{p}")));
    if authority.is_empty() { return Err("HTTP autodownload URL has no host".into()); }

    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let close = bracketed.find(']').ok_or_else(|| "HTTP autodownload URL has an invalid IPv6 host".to_owned())?;
        let host = &bracketed[..close];
        let suffix = &bracketed[close + 1..];
        let port = if suffix.is_empty() {
            80
        } else {
            suffix
                .strip_prefix(':')
                .ok_or_else(|| "HTTP autodownload URL has an invalid IPv6 authority".to_owned())?
                .parse::<u16>()
                .map_err(|_| "HTTP autodownload URL has an invalid port".to_owned())?
        };
        (host.to_owned(), port)
    } else if authority.matches(':').count() == 1 {
        let (host, port) = authority.rsplit_once(':').expect("one colon");
        let port = port.parse::<u16>().map_err(|_| "HTTP autodownload URL has an invalid port".to_owned())?;
        (host.to_owned(), port)
    } else if authority.contains(':') {
        return Err("IPv6 HTTP autodownload hosts must use [address] syntax".into());
    } else {
        (authority.to_owned(), 80)
    };
    if host.is_empty() { return Err("HTTP autodownload URL has no host".into()); }
    Ok((host, port, path))
}

fn encode_qpath(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'_' | b'-' | b'.' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn temp_path_for(final_path: &Path) -> PathBuf {
    let mut name = final_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("download.pk3"))
        .to_os_string();
    name.push(".tmp");
    final_path.with_file_name(name)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn referenced_packages_match_checksum_to_name_and_clamp_missing_names() {
        let info = br"\sv_referencedPaks\1 -2 3 4\sv_referencedPakNames\base/a base/b";
        assert_eq!(server_referenced_checksums(info), vec![1, -2]);
        assert_eq!(server_referenced_packages(info), vec![
            (1, "base/a".to_owned()), (-2, "base/b".to_owned()),
        ]);
        let malformed = br"\sv_referencedPaks\1 bad 3\sv_referencedPakNames\base/a base/b base/c";
        assert_eq!(server_referenced_packages(malformed), vec![
            (1, "base/a".to_owned()), (3, "base/c".to_owned()),
        ]);
        assert!(server_referenced_checksums(b"").is_empty());
    }

    fn server_info(fields: &str) -> jka_protocol::ServerInfo {
        let mut packet = b"\xff\xff\xff\xffinfoResponse\n".to_vec();
        packet.extend_from_slice(format!("\\challenge\\dltest\\protocol\\26{fields}").as_bytes());
        jka_protocol::parse_info_response(&packet, "dltest").unwrap()
    }

    #[test]
    fn tayst_http_numeric_port_uses_game_server_host() {
        let info = server_info("\\mvhttp\\18200");
        let server: SocketAddr = "203.0.113.7:29070".parse().unwrap();
        assert_eq!(advertised_http_base(&info, server).as_deref(), Some("http://203.0.113.7:18200"));
    }

    #[test]
    fn tayst_http_external_root_is_preserved() {
        let info = server_info("\\mvhttpurl\\http://downloads.example.org/jka/");
        let server: SocketAddr = "203.0.113.7:29070".parse().unwrap();
        assert_eq!(
            advertised_http_base(&info, server).as_deref(),
            Some("http://downloads.example.org/jka")
        );
    }

    #[test]
    fn tayst_http_mvhttp_takes_priority_over_compatibility_key() {
        let info = server_info("\\mvhttp\\18200\\sv_httpServerPort\\19000");
        let server: SocketAddr = "203.0.113.7:29070".parse().unwrap();
        assert_eq!(
            advertised_http_base(&info, server).as_deref(),
            Some("http://203.0.113.7:18200")
        );
    }

    #[test]
    fn direct_http_server_port_key_remains_supported_for_compatibility() {
        let info = server_info("\\sv_httpServerPort\\18200");
        let server: SocketAddr = "203.0.113.7:29070".parse().unwrap();
        assert_eq!(
            advertised_http_base(&info, server).as_deref(),
            Some("http://203.0.113.7:18200")
        );
    }

    #[test]
    fn jk2_downloads_use_the_jk2_write_root_and_never_jka_or_cd() {
        let base = Path::new("/game/jka/base");
        let roots = jka_assets::pk3::FilesystemRoots {
            jk2basepath: Some("/games/jk2".into()),
            jk2cdpath: Some("/disc/jk2".into()),
            active_jk2: Some(2),
            ..Default::default()
        };
        assert_eq!(download_root_for(base, &roots), PathBuf::from("/games/jk2"));
        let roots = jka_assets::pk3::FilesystemRoots {
            jk2homepath: Some("/user/jk2".into()), ..roots
        };
        assert_eq!(download_root_for(base, &roots), PathBuf::from("/user/jk2"));
        let roots = jka_assets::pk3::FilesystemRoots {
            active_jk2: None, homepath: Some("/user/jka".into()), ..roots
        };
        assert_eq!(download_root_for(base, &roots), PathBuf::from("/user/jka"));
    }

    #[test]
    fn stock_packages_and_udp_policy_follow_each_game() {
        assert!(is_stock_assets("assets5", true));
        assert!(is_stock_assets("assetsmv", true));
        assert!(!is_stock_assets("assets3", true));
        assert!(is_stock_assets("assets3", false));
        assert!(!is_stock_assets("assets5", false));
        assert!(!server_allows_udp(b"", true));
        assert!(server_allows_udp(b"", false));
        assert!(server_allows_udp(br"\sv_allowDownload\1", true));
        assert!(!server_allows_udp(br"\sv_allowDownload\0", false));
    }

    #[test]
    fn http_url_parser_accepts_bracketed_ipv6() {
        assert_eq!(
            parse_http_url("http://[2001:db8::1]:18200/base/maps.pk3").unwrap(),
            ("2001:db8::1".to_owned(), 18200, "/base/maps.pk3".to_owned())
        );
    }
}
