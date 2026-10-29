//! Source VTF 7.2 color-map decoder for ordinary VMT $basetexture.
//! Large level zero images are decoded only once per cached Source player.
use crate::materials::TextureData;

fn uint(bytes: &[u8], at: usize) -> Result<u32, String> {
    let b = bytes.get(at..at + 4).ok_or("truncated VTF header")?;
    Ok(u32::from_le_bytes(b.try_into().unwrap()))
}
fn word(bytes: &[u8], at: usize) -> Result<usize, String> {
    let b = bytes.get(at..at + 2).ok_or("truncated VTF header")?;
    Ok(u16::from_le_bytes(b.try_into().unwrap()) as usize)
}
fn rgb565(c: u16) -> [u8; 3] {
    let r = (c >> 11) as u8 & 31;
    let g = (c >> 5) as u8 & 63;
    let b = c as u8 & 31;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}
fn block_pixels(bytes: &[u8], alpha: bool) -> [[u8;4];16] {
    let color = if alpha { &bytes[8..] } else { bytes };
    let c0 = u16::from_le_bytes([color[0],color[1]]);
    let c1 = u16::from_le_bytes([color[2],color[3]]);
    let [a,b] = [rgb565(c0), rgb565(c1)];
    let mut palette = [[0u8;4];4];
    palette[0] = [a[0],a[1],a[2],255];
    palette[1] = [b[0],b[1],b[2],255];
    if c0 > c1 || alpha {
        for k in 0..3 { palette[2][k] = ((2*u16::from(a[k])+u16::from(b[k]))/3) as u8;
            palette[3][k] = ((u16::from(a[k])+2*u16::from(b[k]))/3) as u8; }
        palette[2][3] = 255; palette[3][3] = 255;
    } else {
        for k in 0..3 {palette[2][k] = ((u16::from(a[k])+u16::from(b[k]))/2) as u8;}
        palette[2][3] = 255;
    }
    let bits = u32::from_le_bytes(color[4..8].try_into().unwrap());
    let mut out = [[0u8;4];16];
    for i in 0..16 {out[i] = palette[((bits >> (i*2)) & 3) as usize];}
    if alpha {
        let a0 = bytes[0]; let a1 = bytes[1];
        let mut ap = [0u8;8]; ap[0]=a0; ap[1]=a1;
        if a0>a1 {for i in 2..8 {ap[i]=(((8-i) as u16*a0 as u16+(i-1) as u16*a1 as u16)/7) as u8;}}
        else {for i in 2..6 {ap[i]=(((6-i) as u16*a0 as u16+(i-1) as u16*a1 as u16)/5) as u8;}
            ap[6]=0;ap[7]=255;}
        let mut packed = 0u64;
        for (i, v) in bytes[2..8].iter().enumerate() {packed |= (*v as u64) << (i*8);}
        for i in 0..16 {out[i][3]=ap[((packed >> (i*3)) & 7) as usize];}
    }
    out
}

/// Decode one mip level of a VTF image into tightly packed RGBA8.
fn decode_vtf_level(
    label: &str,
    data: &[u8],
    format: u32,
    width: usize,
    height: usize,
) -> Result<Vec<u8>, String> {
    let level_bytes = width * height * 4;
    let mut rgba = vec![0u8; level_bytes];
    if format == 0 || format == 12 {
        if data.len() < level_bytes {
            return Err(format!("{label}: truncated {width}x{height} level"));
        }
        rgba.copy_from_slice(&data[..level_bytes]);
        if format == 12 {
            for p in rgba.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        return Ok(rgba);
    }
    let block = if format == 15 { 16 } else { 8 };
    let blocks_x = width.div_ceil(4);
    for by in 0..height.div_ceil(4) {
        for bx in 0..blocks_x {
            let start = (by * blocks_x + bx) * block;
            let bytes = data
                .get(start..start + block)
                .ok_or_else(|| format!("{label}: truncated {width}x{height} level"))?;
            let pixels = block_pixels(bytes, format == 15);
            for dy in 0..4 {
                for dx in 0..4 {
                    let (x, y) = (bx * 4 + dx, by * 4 + dy);
                    if x < width && y < height {
                        rgba[(y * width + x) * 4..(y * width + x) * 4 + 4]
                            .copy_from_slice(&pixels[dy * 4 + dx]);
                    }
                }
            }
        }
    }
    Ok(rgba)
}

/// Decode a VTF colour map, including its authored mip chain.
///
/// VTF stores levels smallest-first, while `TextureData` wants them packed
/// largest-first, so the level offsets are walked in reverse. Using the file's own
/// mips rather than box-filtering level 0 keeps the artist's filtering and costs
/// only a third more decode work than level 0 alone. Without a chain a 2048-square
/// player texture minifies by point-sampling, which both aliases badly and thrashes
/// the GPU's texture cache at any distance.
/// `VTF_LEGACY_RSRC_IMAGE`: the high-resolution mip chain.
const VTF_RESOURCE_IMAGE: [u8; 3] = [0x30, 0x00, 0x00];
/// `RSRCF_HAS_NO_DATA_CHUNK`: the entry's `offset` field is an inline value.
const VTF_RESOURCE_NO_DATA: u8 = 0x02;
const TEXTUREFLAGS_ENVMAP: u32 = 0x4000;

/// `$basetexture` and every other colour map: decoded as sRGB.
pub(super) fn decode_color_vtf(label: &str, bytes: &[u8]) -> Result<TextureData, String> {
    decode_vtf(label, bytes, true)
}

/// `$bumpmap`: an ordinary tangent-space RGB normal map. It **must** be decoded
/// linear - an sRGB view would bend every normal - and its alpha is Valve's
/// phong mask, so the channel has to survive too.
pub(super) fn decode_normal_vtf(label: &str, bytes: &[u8]) -> Result<TextureData, String> {
    decode_vtf(label, bytes, false)
}

fn decode_vtf(label: &str, bytes: &[u8], srgb: bool) -> Result<TextureData, String> {
    if bytes.get(..4) != Some(b"VTF\0") {return Err(format!("{label}: invalid VTF signature"));}
    let (major, minor) = (uint(bytes,4)?, uint(bytes,8)?);
    if major != 7 || minor > 5 {return Err(format!("{label}: unsupported VTF version {major}.{minor}"));}
    let header = uint(bytes,12)? as usize;
    let w = word(bytes,16)?;
    let h = word(bytes,18)?;
    if w==0 || h==0 || w>4096 || h>4096 {return Err(format!("{label}: invalid image dimensions"));}
    let flags = uint(bytes,20)?;
    if flags & TEXTUREFLAGS_ENVMAP != 0 {
        return Err(format!("{label}: cubemap VTFs have six faces per level and are not a player texture"));
    }
    // Animated textures interleave every frame inside each mip level; frame 0 is
    // first, so striding past the rest shows the first frame rather than garbage.
    let frames = word(bytes,24)?.max(1);
    // `depth` only exists from 7.2 onwards; a volume texture would add a third
    // dimension to every level.
    let depth = if minor >= 2 { word(bytes,63)?.max(1) } else { 1 };
    if depth != 1 {
        return Err(format!("{label}: volume VTFs (depth {depth}) are not supported"));
    }
    let format = uint(bytes,52)?;
    if !matches!(format, 0|12|13|15) {return Err(format!("{label}: unsupported VTF color format {format}"));}
    let mips = *bytes.get(56).ok_or("truncated VTF mip count")? as usize;
    if mips==0 || mips>13 {return Err("invalid VTF mip chain".into());}
    // A chain longer than the dimensions allow would both desynchronize the level
    // offsets below and be rejected by wgpu, so refuse rather than silently clamp.
    let max_levels = (usize::BITS - w.max(h).leading_zeros()) as usize;
    if mips > max_levels {
        return Err(format!(
            "{label}: VTF declares {mips} mip levels but {w}x{h} allows at most {max_levels}"
        ));
    }
    let stride = |width:usize,height:usize| match format {13=>width.div_ceil(4)*height.div_ceil(4)*8,
        15=>width.div_ceil(4)*height.div_ceil(4)*16, _=>width*height*4};

    // Where the high-resolution mip chain starts.
    //
    // Up to 7.2 it implicitly follows the header and the low-res thumbnail. From
    // 7.3 the header carries a resource dictionary instead, every chunk is found
    // through it, and the thumbnail is just another entry - so the old
    // header-plus-thumbnail arithmetic silently lands in the wrong place.
    let image_start = if minor >= 3 {
        let count = uint(bytes,68)? as usize;
        if count > 64 {
            return Err(format!("{label}: implausible VTF resource count {count}"));
        }
        let mut image = None;
        for index in 0..count {
            let at = 80 + index * 8;
            let tag = bytes.get(at..at+3).ok_or_else(|| format!("{label}: truncated VTF resource table"))?;
            if tag != VTF_RESOURCE_IMAGE {
                continue;
            }
            if bytes[at+3] & VTF_RESOURCE_NO_DATA != 0 {
                return Err(format!("{label}: VTF high-res image resource carries no data chunk"));
            }
            image = Some(uint(bytes,at+4)? as usize);
        }
        image.ok_or_else(|| format!("{label}: VTF {major}.{minor} has no high-res image resource"))?
    } else {
        let low_format = uint(bytes,57)?;
        let lw = *bytes.get(61).ok_or("truncated VTF low width")? as usize;
        let lh = *bytes.get(62).ok_or("truncated VTF low height")? as usize;
        let low_bytes = if lw==0 || lh==0 {0} else {
            match low_format {13|20 => lw.div_ceil(4)*lh.div_ceil(4)*8,
                14|15 => lw.div_ceil(4)*lh.div_ceil(4)*16,
                0|12 => lw*lh*4, other => return Err(format!("unsupported VTF thumbnail format {other}")) }
        };
        header.checked_add(low_bytes).ok_or("VTF header overflow")?
    };
    let level_size = |level: usize| ((w >> level).max(1), (h >> level).max(1));

    // Levels are stored smallest-first, so walk them in reverse to find each one's
    // offset; level 0 therefore ends up last in the file. Each level holds every
    // frame, and frame 0 comes first, so only the frame stride has to be skipped.
    let mut offsets = vec![0usize; mips];
    let mut cursor = image_start;
    for level in (0..mips).rev() {
        offsets[level] = cursor;
        let (lw, lh) = level_size(level);
        cursor = stride(lw, lh)
            .checked_mul(frames)
            .and_then(|level_bytes| cursor.checked_add(level_bytes))
            .ok_or("VTF mip size overflow")?;
    }
    if cursor > bytes.len() {
        return Err(format!("{label}: VTF mip chain extends past end of file"));
    }

    let mut rgba = Vec::with_capacity(w * h * 4 * 4 / 3);
    for level in 0..mips {
        let (lw, lh) = level_size(level);
        let end = offsets[level] + stride(lw, lh);
        let data = bytes
            .get(offsets[level]..end)
            .ok_or_else(|| format!("{label}: truncated mip {level}"))?;
        rgba.extend_from_slice(&decode_vtf_level(label, data, format, lw, lh)?);
    }
    Ok(TextureData {
        label: label.to_owned(),
        source: None,
        width: w as u32,
        height: h as u32,
        rgba: (rgba).into(),
        rgba16f: None,
        mip_level_count: mips as u32,
        clamp: false,
        srgb,
    })
}

/// The VMT parameters the Source player path consumes.
#[derive(Default, Debug, PartialEq)]
pub(super) struct VmtParameters {
    /// `$basetexture`, without `materials/` or `.vtf`.
    pub(super) base_texture: Option<String>,
    /// `$bumpmap`, same form as `base_texture`.
    pub(super) bump_map: Option<String>,
    pub(super) translucent: bool,
    pub(super) alpha_test: bool,
    /// Present only when `$phong` is set; see [`VmtPhong`].
    pub(super) phong: Option<VmtPhong>,
    /// `$halflambert`. Independent of `$phong` in Source, so it is kept out of
    /// `VmtPhong`.
    pub(super) half_lambert: bool,
}

/// `$phong` and friends, with Valve's own defaults for anything the material
/// leaves out (`skin_dx9_helper.cpp`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VmtPhong {
    /// `$phongboost`.
    pub(super) boost: f32,
    /// `$phongexponent`. Valve instead reads the exponent out of
    /// `$phongexponenttexture` when this is absent; that texture is not
    /// supported here, so an absent value takes the documented VMT default.
    pub(super) exponent: f32,
    /// `$phongfresnelranges`, already in the packed form the shader wants:
    /// `((mid-min)*2, mid, (max-mid)*2)`, exactly as the helper packs it so
    /// that `Fresnel(N, V, ranges)` costs two instructions.
    pub(super) fresnel_ranges: [f32; 3],
    /// `$rimlight`.
    pub(super) rimlight: bool,
    /// `$rimlightexponent` (the helper's "rim light power"), at least 1.
    pub(super) rimlight_exponent: f32,
    /// `$rimlightboost`.
    pub(super) rimlight_boost: f32,
}

impl Default for VmtPhong {
    fn default() -> Self {
        Self {
            boost: 1.0,
            // Valve's documented default for a material with no exponent texture.
            exponent: 5.0,
            // {1, 0.5, 1} packed is [0, 0.5, 1] authored: the helper's default.
            fresnel_ranges: [1.0, 0.5, 1.0],
            rimlight: false,
            rimlight_exponent: 4.0,
            rimlight_boost: 1.0,
        }
    }
}

/// Everything after the key, trimmed. Quoted values may contain spaces.
fn vmt_raw_value(line: &str) -> Option<&str> {
    // Values may be quoted and contain spaces; keys never do.
    let rest = line.trim_start();
    let rest = if let Some(tail) = rest.strip_prefix('"') {
        tail.split_once('"')?.1
    } else {
        rest.split_once(char::is_whitespace)?.1
    };
    Some(rest.trim())
}

fn vmt_value(line: &str) -> Option<&str> {
    let rest = vmt_raw_value(line)?;
    Some(match rest.strip_prefix('"') {
        Some(tail) => tail.split('"').next()?,
        None => rest.split_whitespace().next()?,
    })
}

/// A `[a b c]` (or `{a b c}`) VMT vector. The braced form is Valve's 0-255
/// integer spelling, which no parameter read here uses, so both are taken at
/// face value.
fn vmt_vector3(line: &str) -> Option<[f32; 3]> {
    let rest = vmt_raw_value(line)?.trim_matches('"').trim();
    let inner = rest
        .trim_start_matches(['[', '{'])
        .trim_end_matches([']', '}']);
    let mut values = inner.split_whitespace();
    let mut next = || -> Option<f32> {
        values
            .next()?
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
    };
    Some([next()?, next()?, next()?])
}

fn vmt_number(line: &str) -> Option<f32> {
    vmt_value(line)?
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
}

fn vmt_texture_path(value: &str) -> Option<String> {
    let path = value.trim().replace('\\', "/");
    let path = path.trim_matches('/').to_owned();
    // The descriptor path is joined under `materials/`, so refuse traversal.
    if path.is_empty() || path.contains("..") {
        return None;
    }
    Some(path)
}

fn vmt_flag(value: &str) -> bool {
    // Source treats any non-zero numeric value as set.
    value
        .trim()
        .parse::<f32>()
        .map(|v| v != 0.0)
        .unwrap_or(false)
}

/// Read the parameters this path needs out of a VMT. Only top-level keys are
/// read: nested blocks are `Proxies` or shader fallbacks, whose keys must not be
/// mistaken for the material's own.
pub(super) fn parse_vmt(bytes: &[u8]) -> VmtParameters {
    let text = String::from_utf8_lossy(bytes);
    let mut parameters = VmtParameters::default();
    // The phong parameters may be authored before `$phong` itself, so they are
    // collected unconditionally and kept only if the material enables phong.
    let mut phong = VmtPhong::default();
    let mut phong_enabled = false;
    let mut depth = 0i32;
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let opens = line.matches('{').count() as i32;
        let closes = line.matches('}').count() as i32;
        // The shader name and its opening brace put the material body at depth 1.
        if depth == 1 {
            let key = line
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches('"')
                .to_ascii_lowercase();
            if let Some(value) = vmt_value(line) {
                match key.as_str() {
                    "$basetexture" => parameters.base_texture = vmt_texture_path(value),
                    "$bumpmap" => parameters.bump_map = vmt_texture_path(value),
                    "$translucent" => parameters.translucent = vmt_flag(value),
                    "$alphatest" => parameters.alpha_test = vmt_flag(value),
                    "$halflambert" => parameters.half_lambert = vmt_flag(value),
                    // `$phong` gates the whole specular path in Valve's helper.
                    "$phong" => phong_enabled = vmt_flag(value),
                    "$phongboost" => {
                        if let Some(boost) = vmt_number(line) {
                            phong.boost = boost.max(0.0);
                        }
                    }
                    // The helper only overrides the exponent when the material's
                    // own value is positive.
                    "$phongexponent" => {
                        if let Some(exponent) = vmt_number(line).filter(|value| *value > 0.0) {
                            phong.exponent = exponent;
                        }
                    }
                    "$phongfresnelranges" => {
                        if let Some([min, mid, max]) = vmt_vector3(line) {
                            phong.fresnel_ranges = [(mid - min) * 2.0, mid, (max - mid) * 2.0];
                        }
                    }
                    "$rimlight" => phong.rimlight = vmt_flag(value),
                    "$rimlightexponent" => {
                        if let Some(exponent) = vmt_number(line) {
                            phong.rimlight_exponent = exponent.max(1.0);
                        }
                    }
                    "$rimlightboost" => {
                        if let Some(boost) = vmt_number(line) {
                            phong.rimlight_boost = boost.max(0.0);
                        }
                    }
                    _ => {}
                }
            }
        }
        depth += opens - closes;
    }
    if phong_enabled {
        parameters.phong = Some(phong);
    }
    parameters
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_base_texture_and_alpha_flags() {
        let vmt = br#""VertexLitGeneric"
{
    "$basetexture" "models\hev2\mk2chest_dif"   // comment
    "$bumpmap" "models/hev2/mk2chest_nrm"
    "$alphatest" "1"
    "Proxies"
    {
        "$basetexture" "should/not/win"
    }
}
"#;
        let parsed = parse_vmt(vmt);
        assert_eq!(
            parsed.base_texture.as_deref(),
            Some("models/hev2/mk2chest_dif")
        );
        assert_eq!(parsed.bump_map.as_deref(), Some("models/hev2/mk2chest_nrm"));
        assert!(parsed.alpha_test);
        assert!(!parsed.translucent);
    }

    /// Every VMT in the HEV MK2 test pack declares exactly this set.
    #[test]
    fn reads_the_phong_block() {
        let parsed = parse_vmt(
            br#""VertexLitGeneric"
{
    "$basetexture" "models/hev2/mk2chest_dif"
    "$bumpmap"     "models/hev2/mk2chest_nrm"
    "$phong" "1"
    "$phongboost" "2"
    "$phongexponent" "10"
    "$phongfresnelranges" "[.5 .5 .5]"
    "$rimlight" "1"
    "$rimlightexponent" "15"
    "$rimlightboost" "5"
    "$halflambert" "1"
}
"#,
        );
        let phong = parsed.phong.expect("$phong 1");
        assert_eq!(phong.boost, 2.0);
        assert_eq!(phong.exponent, 10.0);
        // [.5 .5 .5] packs to ((mid-min)*2, mid, (max-mid)*2) = (0, .5, 0).
        assert_eq!(phong.fresnel_ranges, [0.0, 0.5, 0.0]);
        assert!(phong.rimlight);
        assert_eq!(phong.rimlight_exponent, 15.0);
        assert_eq!(phong.rimlight_boost, 5.0);
        assert!(parsed.half_lambert);
    }

    #[test]
    fn phong_parameters_without_phong_are_dropped() {
        // A material that tunes phong but never enables it is not a phong
        // material, and must stay on the pipeline variant that costs nothing.
        let parsed = parse_vmt(
            b"VertexLitGeneric
{
$basetexture models/a/b
$phongexponent 20
$rimlight 1
}
",
        );
        assert_eq!(parsed.phong, None);
    }

    #[test]
    fn phong_defaults_match_the_source_helper() {
        let parsed = parse_vmt(
            b"VertexLitGeneric
{
$basetexture models/a/b
$phong 1
}
",
        );
        let phong = parsed.phong.expect("$phong 1");
        assert_eq!(phong.boost, 1.0);
        assert_eq!(phong.fresnel_ranges, [1.0, 0.5, 1.0]);
        assert!(!phong.rimlight);
        assert_eq!(phong.rimlight_exponent, 4.0);
        assert!(!parsed.half_lambert);
    }

    #[test]
    fn a_nonpositive_phong_exponent_keeps_the_default() {
        // The helper only overrides the exponent when the authored value is
        // positive; zero means "read it from the exponent texture".
        let parsed = parse_vmt(
            b"VertexLitGeneric
{
$phong 1
$phongexponent 0
}
",
        );
        assert_eq!(parsed.phong.expect("$phong 1").exponent, 5.0);
    }

    #[test]
    fn a_normal_map_decodes_linear() {
        let decoded = decode_normal_vtf("synthetic", &synthetic_vtf(2, 2, 1)).unwrap();
        assert!(!decoded.srgb, "an sRGB view would bend every normal");
        assert!(
            decode_color_vtf("synthetic", &synthetic_vtf(2, 2, 1))
                .unwrap()
                .srgb
        );
    }

    #[test]
    fn unquoted_keys_and_zero_flags_work() {
        let parsed = parse_vmt(b"VertexLitGeneric
{
$basetexture models/a/b
$translucent 0
}
");
        assert_eq!(parsed.base_texture.as_deref(), Some("models/a/b"));
        assert!(!parsed.translucent);
    }

    #[test]
    fn refuses_traversal() {
        let parsed = parse_vmt(b"VertexLitGeneric
{
\"$basetexture\" \"../../etc/passwd\"
}
");
        assert_eq!(parsed.base_texture, None);
    }

    #[test]
    fn rejects_truncated_vtf() {
        assert!(decode_color_vtf("bad", b"VTF").is_err());
    }

    /// Minimal uncompressed VTF: `width` x `height`, `mips` levels, no thumbnail.
    /// Level L is filled with the byte value `0x10 + L` so the packing order is
    /// observable.
    fn synthetic_vtf(width: usize, height: usize, mips: usize) -> Vec<u8> {
        const HEADER: usize = 80;
        let mut bytes = vec![0u8; HEADER];
        bytes[..4].copy_from_slice(b"VTF\0");
        bytes[4..8].copy_from_slice(&7u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&2u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER as u32).to_le_bytes());
        bytes[16..18].copy_from_slice(&(width as u16).to_le_bytes());
        bytes[18..20].copy_from_slice(&(height as u16).to_le_bytes());
        // Format 0 = RGBA8888; thumbnail width/height 0 means no thumbnail.
        bytes[52..56].copy_from_slice(&0u32.to_le_bytes());
        bytes[56] = mips as u8;
        // VTF stores levels smallest-first.
        for level in (0..mips).rev() {
            let (lw, lh) = ((width >> level).max(1), (height >> level).max(1));
            bytes.extend_from_slice(&vec![0x10 + level as u8; lw * lh * 4]);
        }
        bytes
    }

    #[test]
    fn decodes_the_whole_mip_chain_largest_first() {
        let decoded = decode_color_vtf("synthetic", &synthetic_vtf(2, 2, 2)).unwrap();
        assert_eq!(decoded.mip_level_count, 2);
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        // 2x2 then 1x1, tightly packed: exactly what mip_upload_window walks.
        assert_eq!(decoded.rgba.len(), 2 * 2 * 4 + 4);
        // Level 0 must come first even though the file stores it last.
        assert!(decoded.rgba[..16].iter().all(|b| *b == 0x10));
        assert!(decoded.rgba[16..].iter().all(|b| *b == 0x11));
    }

    #[test]
    fn a_single_level_vtf_still_decodes() {
        let decoded = decode_color_vtf("synthetic", &synthetic_vtf(4, 4, 1)).unwrap();
        assert_eq!(decoded.mip_level_count, 1);
        assert_eq!(decoded.rgba.len(), 4 * 4 * 4);
    }

    #[test]
    fn rejects_a_mip_chain_longer_than_the_dimensions_allow() {
        // 4x4 allows 3 levels (4, 2, 1); claiming 6 would desync the offsets.
        let mut bytes = synthetic_vtf(4, 4, 3);
        bytes[56] = 6;
        let error = decode_color_vtf("synthetic", &bytes).unwrap_err();
        assert!(error.contains("at most 3"), "{error}");
    }
}
