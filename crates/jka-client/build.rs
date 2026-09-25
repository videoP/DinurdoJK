use std::{
    env, fs,
    io::{BufReader, Write},
    path::{Path, PathBuf},
    process::Command,
};

const ICON_SIZE: usize = 256;

fn main() {
    println!("cargo:rerun-if-changed=../../icon.png");

    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    if let Err(error) = build_windows_icon() {
        panic!("failed to embed ../../icon.png as the DinurdoJK executable icon: {error}");
    }

    if let Err(error) = copy_steam_audio_runtime_dll() {
        println!("cargo:warning=could not stage phonon.dll next to the executable: {error}");
    }
}

/// audionimbus-sys auto-installs Steam Audio's `phonon.dll` into its own
/// build-script OUT_DIR, but Cargo never copies a dependency's runtime DLL
/// next to the final executable. Without this, the built binary starts and
/// immediately fails with "the code execution cannot proceed because
/// phonon.dll was not found" the moment it touches the audionimbus FFI.
fn copy_steam_audio_runtime_dll() -> Result<(), Box<dyn std::error::Error>> {
    // OUT_DIR is target/<profile>/build/DinurdoJK-<hash>/out; the profile
    // directory (where the .exe lands) is three levels up from there.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
    let build_dir = out_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("OUT_DIR has no build directory ancestor")?;
    let profile_dir = build_dir
        .parent()
        .ok_or("build directory has no profile directory ancestor")?;

    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(build_dir)?.filter_map(Result::ok) {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("audionimbus-sys-") {
            continue;
        }
        let candidate = entry.path().join("out").join("lib").join("phonon.dll");
        if !candidate.is_file() {
            continue;
        }
        let modified = fs::metadata(&candidate)?.modified()?;
        if newest.as_ref().is_none_or(|(seen, _)| modified > *seen) {
            newest = Some((modified, candidate));
        }
    }

    let Some((_, source)) = newest else {
        // Not an error: audionimbus-sys may not have run yet on a fresh
        // `cargo check`, or Steam Audio was supplied another way.
        return Ok(());
    };
    let destination = profile_dir.join("phonon.dll");
    fs::copy(&source, &destination)?;
    println!("cargo:rerun-if-changed={}", source.display());
    Ok(())
}

fn build_windows_icon() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?);
    let icon_png = manifest_dir.join("..\\..\\icon.png");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?);
    let icon_ico = out_dir.join("DinurdoJK.ico");
    let resource_script = out_dir.join("DinurdoJK.rc");
    let resource_file = out_dir.join("DinurdoJK.res");

    write_ico(&icon_png, &icon_ico)?;
    let icon_ico_for_rc = icon_ico.to_string_lossy().replace('\\', "/");
    fs::write(
        &resource_script,
        format!("1 ICON \"{icon_ico_for_rc}\"\r\n"),
    )?;

    let rc = find_resource_compiler()?;
    let status = Command::new(rc)
        .args([
            "/nologo",
            &format!("/fo{}", resource_file.display()),
            resource_script
                .to_str()
                .ok_or("invalid resource script path")?,
        ])
        .status()?;
    if !status.success() {
        return Err(format!("rc.exe exited with {status}").into());
    }

    println!(
        "cargo:rustc-link-arg-bin=DinurdoJK={}",
        resource_file.display()
    );
    Ok(())
}

fn find_resource_compiler() -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Ok(path) = which::which("rc.exe") {
        return Ok(path);
    }

    let program_files = env::var_os("ProgramFiles(x86)")
        .or_else(|| env::var_os("ProgramFiles"))
        .ok_or("Program Files directory is unavailable")?;
    let sdk_bin = PathBuf::from(program_files).join("Windows Kits\\10\\bin");
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| "x86_64".into());
    let sdk_arch = match target_arch.as_str() {
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };

    let mut versions = fs::read_dir(sdk_bin)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    versions.sort();
    versions.reverse();
    for version in versions {
        let candidate = version.join(sdk_arch).join("rc.exe");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("could not locate Windows SDK rc.exe".into())
}

fn write_ico(png_path: &Path, ico_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let file = fs::File::open(png_path)?;
    let decoder = png::Decoder::new(BufReader::new(file));
    let mut reader = decoder.read_info()?;
    let mut source = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or("PNG has no output buffer")?
    ];
    let frame = reader.next_frame(&mut source)?;
    let source = &source[..frame.buffer_size()];
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut rgba = vec![[0_u8; 4]; ICON_SIZE * ICON_SIZE];

    for y in 0..ICON_SIZE {
        let source_y = y * height / ICON_SIZE;
        for x in 0..ICON_SIZE {
            let source_x = x * width / ICON_SIZE;
            let source_index = source_y * width + source_x;
            rgba[y * ICON_SIZE + x] = match frame.color_type {
                png::ColorType::Rgba => {
                    let index = source_index * 4;
                    [
                        source[index],
                        source[index + 1],
                        source[index + 2],
                        source[index + 3],
                    ]
                }
                png::ColorType::Rgb => {
                    let index = source_index * 3;
                    [source[index], source[index + 1], source[index + 2], 255]
                }
                png::ColorType::GrayscaleAlpha => {
                    let index = source_index * 2;
                    [
                        source[index],
                        source[index],
                        source[index],
                        source[index + 1],
                    ]
                }
                png::ColorType::Grayscale => {
                    let value = source[source_index];
                    [value, value, value, 255]
                }
                png::ColorType::Indexed => {
                    return Err("indexed PNG icons are not supported".into());
                }
            };
        }
    }

    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, ICON_SIZE as u32, ICON_SIZE as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        let rgba_data = rgba
            .iter()
            .flat_map(|pixel| pixel.iter().copied())
            .collect::<Vec<_>>();
        writer.write_image_data(&rgba_data)?;
    }

    let mut output = Vec::with_capacity(6 + 16 + png_data.len());
    output.write_all(&0_u16.to_le_bytes())?;
    output.write_all(&1_u16.to_le_bytes())?;
    output.write_all(&1_u16.to_le_bytes())?;
    output.push(0);
    output.push(0);
    output.push(0);
    output.push(0);
    output.write_all(&1_u16.to_le_bytes())?;
    output.write_all(&32_u16.to_le_bytes())?;
    output.write_all(&(png_data.len() as u32).to_le_bytes())?;
    output.write_all(&22_u32.to_le_bytes())?;
    output.extend_from_slice(&png_data);
    fs::write(ico_path, output)?;
    Ok(())
}

mod which {
    use std::{env, path::PathBuf, process::Command};

    pub fn which(command: &str) -> Result<PathBuf, ()> {
        let path = env::var_os("PATH").ok_or(())?;
        for directory in env::split_paths(&path) {
            let candidate = directory.join(command);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        let output = Command::new("where.exe")
            .arg(command)
            .output()
            .map_err(|_| ())?;
        if !output.status.success() {
            return Err(());
        }
        String::from_utf8(output.stdout)
            .ok()
            .and_then(|line| line.lines().next().map(PathBuf::from))
            .ok_or(())
    }
}
