//! Native parser for Quake 3 / Jedi Academy `.map` source files.
//!
//! The parser preserves convex brush planes and per-face texture projection data.
//! Geometry reconstruction is intentionally left to the client scene layer.

use std::collections::BTreeMap;

pub const MAX_MAP_FILE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    pub normal: [f64; 3],
    pub distance: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BrushStyle {
    Legacy,
    Valve220,
    BrushDef,
    BrushDef3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TextureProjection {
    Legacy {
        shift: [f64; 2],
        rotate: f64,
        scale: [f64; 2],
    },
    Valve220 {
        u_axis: [f64; 3],
        u_shift: f64,
        v_axis: [f64; 3],
        v_shift: f64,
        rotate: f64,
        scale: [f64; 2],
    },
    BrushPrimitive {
        matrix: [[f64; 3]; 2],
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapFace {
    pub plane: Plane,
    pub shader: String,
    pub projection: TextureProjection,
    /// Optional trailing `contents surface_flags value` numbers, retained exactly
    /// as numeric source data because writers are not perfectly consistent about
    /// whether they are present or integral.
    pub trailing: Vec<f64>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MapBrush {
    pub faces: Vec<MapFace>,
    pub style: BrushStyle,
    /// Editor-only brush key/value pairs accepted by brush primitive writers.
    pub properties: BTreeMap<String, String>,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapEntity {
    pub properties: BTreeMap<String, String>,
    pub brushes: Vec<MapBrush>,
    pub patches_skipped: usize,
    pub unsupported_objects_skipped: usize,
}

impl MapEntity {
    pub fn classname(&self) -> Option<&str> {
        self.properties.get("classname").map(String::as_str)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapStats {
    pub entities: usize,
    pub brushes: usize,
    pub degenerate_faces: usize,
    pub degenerate_brushes: usize,
    pub patches_skipped: usize,
    pub unsupported_objects_skipped: usize,
    pub styles: BTreeMap<BrushStyle, usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MapDocument {
    pub entities: Vec<MapEntity>,
    pub stats: MapStats,
}

#[derive(Clone)]
struct Tokenizer<'a> {
    text: &'a str,
    index: usize,
    line: usize,
}

impl<'a> Tokenizer<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            index: 0,
            line: 1,
        }
    }

    fn skip_trivia(&mut self) -> Result<(), String> {
        let bytes = self.text.as_bytes();
        while self.index < bytes.len() {
            match bytes[self.index] {
                b'\n' => {
                    self.line += 1;
                    self.index += 1;
                }
                b' ' | b'\t' | b'\r' => self.index += 1,
                b'/' if bytes.get(self.index + 1) == Some(&b'/') => {
                    while self.index < bytes.len() && bytes[self.index] != b'\n' {
                        self.index += 1;
                    }
                }
                b'/' if bytes.get(self.index + 1) == Some(&b'*') => {
                    self.index += 2;
                    let mut closed = false;
                    while self.index + 1 < bytes.len() {
                        if bytes[self.index] == b'\n' {
                            self.line += 1;
                        }
                        if bytes[self.index] == b'*' && bytes[self.index + 1] == b'/' {
                            self.index += 2;
                            closed = true;
                            break;
                        }
                        self.index += 1;
                    }
                    if !closed {
                        return Err(format!(
                            "Unterminated block comment near line {}.",
                            self.line
                        ));
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    fn next(&mut self) -> Result<Option<String>, String> {
        self.skip_trivia()?;
        let bytes = self.text.as_bytes();
        if self.index >= bytes.len() {
            return Ok(None);
        }

        if bytes[self.index] == b'"' {
            let start_line = self.line;
            self.index += 1;
            let start = self.index;
            while self.index < bytes.len() && bytes[self.index] != b'"' {
                if bytes[self.index] == b'\n' {
                    self.line += 1;
                }
                self.index += 1;
            }
            if self.index >= bytes.len() {
                return Err(format!(
                    "Unterminated quoted string starting on line {start_line}."
                ));
            }
            let token = self.text[start..self.index].to_owned();
            self.index += 1;
            return Ok(Some(token));
        }

        if matches!(bytes[self.index], b'(' | b')' | b'{' | b'}' | b'[' | b']') {
            let token = char::from(bytes[self.index]).to_string();
            self.index += 1;
            return Ok(Some(token));
        }

        let start = self.index;
        while self.index < bytes.len() {
            let current = bytes[self.index];
            if current.is_ascii_whitespace()
                || matches!(current, b'(' | b')' | b'{' | b'}' | b'[' | b']' | b'"')
            {
                break;
            }
            self.index += 1;
        }
        if self.index == start {
            return Err(format!("Unexpected byte on line {}.", self.line));
        }
        Ok(Some(self.text[start..self.index].to_owned()))
    }

    fn peek(&mut self) -> Result<Option<String>, String> {
        let saved_index = self.index;
        let saved_line = self.line;
        let token = self.next();
        self.index = saved_index;
        self.line = saved_line;
        token
    }

    fn expect(&mut self, expected: &str) -> Result<(), String> {
        let token = self.next()?;
        if token.as_deref() != Some(expected) {
            return Err(format!(
                "Expected \"{expected}\" but found {} on line {}.",
                token
                    .as_deref()
                    .map(|value| format!("\"{value}\""))
                    .unwrap_or_else(|| "end of file".into()),
                self.line
            ));
        }
        Ok(())
    }
}

fn read_number(tokenizer: &mut Tokenizer<'_>) -> Result<f64, String> {
    let token = tokenizer.next()?.ok_or_else(|| {
        format!(
            "Expected a number at end of file near line {}.",
            tokenizer.line
        )
    })?;
    let value = token.parse::<f64>().map_err(|_| {
        format!(
            "Expected a number but found \"{token}\" on line {}.",
            tokenizer.line
        )
    })?;
    if !value.is_finite() {
        return Err(format!(
            "Non-finite number \"{token}\" on line {}.",
            tokenizer.line
        ));
    }
    Ok(value)
}

fn read_point(tokenizer: &mut Tokenizer<'_>) -> Result<[f64; 3], String> {
    tokenizer.expect("(")?;
    let point = [
        read_number(tokenizer)?,
        read_number(tokenizer)?,
        read_number(tokenizer)?,
    ];
    tokenizer.expect(")")?;
    Ok(point)
}

fn normalize_shader(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn plane_from_points(p0: [f64; 3], p1: [f64; 3], p2: [f64; 3]) -> Option<Plane> {
    // q3map2 PlaneFromPoints convention. The brush interior is the negative
    // half-space: dot(normal, point) - distance <= 0.
    let mut normal = cross(sub(p2, p0), sub(p1, p0));
    let length = dot(normal, normal).sqrt();
    if !length.is_finite() || length <= 1e-9 {
        return None;
    }
    for component in &mut normal {
        *component /= length;
    }
    Some(Plane {
        normal,
        distance: dot(p0, normal),
    })
}

fn read_brush_def3_plane(tokenizer: &mut Tokenizer<'_>) -> Result<Option<Plane>, String> {
    tokenizer.expect("(")?;
    let mut normal = [
        read_number(tokenizer)?,
        read_number(tokenizer)?,
        read_number(tokenizer)?,
    ];
    let source_distance = read_number(tokenizer)?;
    tokenizer.expect(")")?;
    let length = dot(normal, normal).sqrt();
    if !length.is_finite() || length <= 1e-9 {
        return Ok(None);
    }
    for component in &mut normal {
        *component /= length;
    }
    // brushDef3 stores dot(n, x) + d = 0.
    Ok(Some(Plane {
        normal,
        distance: -source_distance / length,
    }))
}

fn read_texture_matrix(tokenizer: &mut Tokenizer<'_>) -> Result<[[f64; 3]; 2], String> {
    tokenizer.expect("(")?;
    let mut matrix = [[0.0; 3]; 2];
    for row in &mut matrix {
        tokenizer.expect("(")?;
        for value in row.iter_mut() {
            *value = read_number(tokenizer)?;
        }
        tokenizer.expect(")")?;
    }
    tokenizer.expect(")")?;
    Ok(matrix)
}

fn read_optional_numbers(
    tokenizer: &mut Tokenizer<'_>,
    maximum: usize,
) -> Result<Vec<f64>, String> {
    let mut result = Vec::new();
    for _ in 0..maximum {
        let Some(token) = tokenizer.peek()? else {
            break;
        };
        if matches!(token.as_str(), "(" | "{" | "}") {
            break;
        }
        if token.parse::<f64>().is_err() {
            break;
        }
        result.push(read_number(tokenizer)?);
    }
    Ok(result)
}

fn parse_brush(tokenizer: &mut Tokenizer<'_>, stats: &mut MapStats) -> Result<MapBrush, String> {
    let line = tokenizer.line;
    let mut style = BrushStyle::Legacy;
    match tokenizer.peek()?.as_deref() {
        Some("brushDef") => {
            tokenizer.next()?;
            tokenizer.expect("{")?;
            style = BrushStyle::BrushDef;
        }
        Some("brushDef3") => {
            tokenizer.next()?;
            tokenizer.expect("{")?;
            style = BrushStyle::BrushDef3;
        }
        _ => {}
    }

    let mut faces = Vec::new();
    let mut properties = BTreeMap::new();
    loop {
        let Some(token) = tokenizer.peek()? else {
            return Err(format!("Unterminated brush starting on line {line}."));
        };
        if token == "}" {
            tokenizer.next()?;
            break;
        }
        if matches!(style, BrushStyle::BrushDef | BrushStyle::BrushDef3) && token != "(" {
            let key = tokenizer
                .next()?
                .ok_or_else(|| format!("Brush key missing near line {}.", tokenizer.line))?;
            let value = tokenizer
                .next()?
                .ok_or_else(|| format!("Brush key \"{key}\" is missing its value."))?;
            properties.insert(key.to_ascii_lowercase(), value);
            continue;
        }
        let face_line = tokenizer.line;

        let (plane, primitive_matrix) = if style == BrushStyle::BrushDef3 {
            (
                read_brush_def3_plane(tokenizer)?,
                Some(read_texture_matrix(tokenizer)?),
            )
        } else {
            let p0 = read_point(tokenizer)?;
            let p1 = read_point(tokenizer)?;
            let p2 = read_point(tokenizer)?;
            let matrix = if style == BrushStyle::BrushDef {
                Some(read_texture_matrix(tokenizer)?)
            } else {
                None
            };
            (plane_from_points(p0, p1, p2), matrix)
        };

        let shader =
            normalize_shader(&tokenizer.next()?.ok_or_else(|| {
                format!("Brush face on line {face_line} is missing a shader name.")
            })?);

        let (projection, trailing) = if let Some(matrix) = primitive_matrix {
            (
                TextureProjection::BrushPrimitive { matrix },
                read_optional_numbers(tokenizer, 3)?,
            )
        } else if matches!(style, BrushStyle::Legacy | BrushStyle::Valve220)
            && tokenizer.peek()?.as_deref() == Some("[")
        {
            let mut axes = [[0.0; 4]; 2];
            for axis in &mut axes {
                tokenizer.expect("[")?;
                for value in axis.iter_mut() {
                    *value = read_number(tokenizer)?;
                }
                tokenizer.expect("]")?;
            }
            let rotate = read_number(tokenizer)?;
            let scale = [read_number(tokenizer)?, read_number(tokenizer)?];
            style = BrushStyle::Valve220;
            (
                TextureProjection::Valve220 {
                    u_axis: [axes[0][0], axes[0][1], axes[0][2]],
                    u_shift: axes[0][3],
                    v_axis: [axes[1][0], axes[1][1], axes[1][2]],
                    v_shift: axes[1][3],
                    rotate,
                    scale,
                },
                read_optional_numbers(tokenizer, 3)?,
            )
        } else {
            let shift = [read_number(tokenizer)?, read_number(tokenizer)?];
            let rotate = read_number(tokenizer)?;
            let scale = [read_number(tokenizer)?, read_number(tokenizer)?];
            (
                TextureProjection::Legacy {
                    shift,
                    rotate,
                    scale,
                },
                read_optional_numbers(tokenizer, 3)?,
            )
        };

        if let Some(plane) = plane {
            faces.push(MapFace {
                plane,
                shader,
                projection,
                trailing,
                line: face_line,
            });
        } else {
            stats.degenerate_faces += 1;
        }
    }

    if style == BrushStyle::BrushDef || style == BrushStyle::BrushDef3 {
        tokenizer.expect("}")?;
    }
    if faces.len() < 4 {
        stats.degenerate_brushes += 1;
    }
    *stats.styles.entry(style).or_default() += 1;
    Ok(MapBrush {
        faces,
        style,
        properties,
        line,
    })
}

fn skip_object_body(tokenizer: &mut Tokenizer<'_>) -> Result<(), String> {
    // The object's opening brace has already been consumed.
    let mut depth = 1usize;
    while depth > 0 {
        let token = tokenizer
            .next()?
            .ok_or_else(|| "Unbalanced braces at end of .map file.".to_string())?;
        match token.as_str() {
            "{" => depth += 1,
            "}" => depth -= 1,
            _ => {}
        }
    }
    Ok(())
}

fn skip_patch(tokenizer: &mut Tokenizer<'_>) -> Result<(), String> {
    // patchDef2/patchDef3 has been consumed. The next balanced block is the patch
    // payload; after it comes the outer object brace that the entity parser saw.
    tokenizer.expect("{")?;
    let mut depth = 1usize;
    while depth > 0 {
        let token = tokenizer
            .next()?
            .ok_or_else(|| "Unterminated patch definition.".to_string())?;
        match token.as_str() {
            "{" => depth += 1,
            "}" => depth -= 1,
            _ => {}
        }
    }
    tokenizer.expect("}")?;
    Ok(())
}

pub fn parse(text: &str) -> Result<MapDocument, String> {
    if text.len() > MAX_MAP_FILE_BYTES {
        return Err(format!(
            ".map source exceeds {} MiB limit",
            MAX_MAP_FILE_BYTES / (1024 * 1024)
        ));
    }

    let mut tokenizer = Tokenizer::new(text);
    let mut document = MapDocument::default();

    while let Some(token) = tokenizer.next()? {
        if token != "{" {
            continue;
        }

        document.stats.entities += 1;
        let mut entity = MapEntity::default();
        loop {
            let Some(inner) = tokenizer.peek()? else {
                return Err("Unterminated entity at end of .map file.".into());
            };
            if inner == "}" {
                tokenizer.next()?;
                break;
            }

            if inner == "{" {
                tokenizer.next()?;
                let body_token = tokenizer.peek()?.ok_or("Unterminated object in entity.")?;
                if body_token == "patchDef2" || body_token == "patchDef3" {
                    tokenizer.next()?;
                    skip_patch(&mut tokenizer)?;
                    entity.patches_skipped += 1;
                    document.stats.patches_skipped += 1;
                    continue;
                }
                if body_token == "(" || body_token == "brushDef" || body_token == "brushDef3" {
                    let brush = parse_brush(&mut tokenizer, &mut document.stats)?;
                    document.stats.brushes += 1;
                    entity.brushes.push(brush);
                    continue;
                }

                skip_object_body(&mut tokenizer)?;
                entity.unsupported_objects_skipped += 1;
                document.stats.unsupported_objects_skipped += 1;
                continue;
            }

            let key = tokenizer
                .next()?
                .ok_or_else(|| "Entity key missing at end of .map file.".to_string())?;
            let value = tokenizer
                .next()?
                .ok_or_else(|| format!("Entity key \"{key}\" is missing its value."))?;
            entity.properties.insert(key.to_ascii_lowercase(), value);
        }
        document.entities.push(entity);
    }

    Ok(document)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: &str = r#"
{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall 0 0 0 1 1
( 64 0 0 ) ( 64 0 64 ) ( 64 64 0 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 0 0 64 ) ( 64 0 0 ) test/wall 0 0 0 1 1
( 0 64 0 ) ( 64 64 0 ) ( 0 64 64 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 64 0 ) test/floor 0 0 0 1 1
( 0 0 64 ) ( 0 64 64 ) ( 64 0 64 ) test/ceiling 0 0 0 1 1
}
}
"#;

    #[test]
    fn legacy_box_and_entity_are_parsed() {
        let map = parse(BOX).unwrap();
        assert_eq!(map.entities.len(), 1);
        assert_eq!(map.entities[0].classname(), Some("worldspawn"));
        assert_eq!(map.entities[0].brushes.len(), 1);
        assert_eq!(map.entities[0].brushes[0].faces.len(), 6);
        assert_eq!(map.stats.brushes, 1);
        assert_eq!(map.entities[0].brushes[0].style, BrushStyle::Legacy);
    }

    #[test]
    fn plane_winding_matches_q3map2_negative_halfspace() {
        let plane = plane_from_points([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]).unwrap();
        assert_eq!(plane.normal, [-1.0, 0.0, 0.0]);
        assert!(dot(plane.normal, [1.0, 0.0, 0.0]) - plane.distance <= 0.0);
        assert!(dot(plane.normal, [-1.0, 0.0, 0.0]) - plane.distance > 0.0);
    }

    #[test]
    fn brush_def_retains_texture_matrix() {
        let source = r#"{
"classname" "worldspawn"
{
brushDef
{
"editor_note" "retained"
( 0 0 0 ) ( 0 0 64 ) ( 0 64 0 ) ( ( 0.01 0 0.25 ) ( 0 0.02 -0.5 ) ) test/a 1 2 3
( 64 0 0 ) ( 64 64 0 ) ( 64 0 64 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
( 0 0 0 ) ( 64 0 0 ) ( 0 0 64 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
( 0 64 0 ) ( 0 64 64 ) ( 64 64 0 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
}
}
}"#;
        let map = parse(source).unwrap();
        let brush = &map.entities[0].brushes[0];
        assert_eq!(brush.style, BrushStyle::BrushDef);
        assert_eq!(
            brush.properties.get("editor_note").map(String::as_str),
            Some("retained")
        );
        assert!(matches!(
            brush.faces[0].projection,
            TextureProjection::BrushPrimitive { matrix }
                if matrix == [[0.01, 0.0, 0.25], [0.0, 0.02, -0.5]]
        ));
        assert_eq!(brush.faces[0].trailing, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn brush_def3_flips_direct_plane_distance() {
        let source = r#"{
"classname" "worldspawn"
{
brushDef3
{
( 1 0 0 -64 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
( -1 0 0 0 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
( 0 1 0 -64 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
( 0 -1 0 0 ) ( ( 1 0 0 ) ( 0 1 0 ) ) test/a
}
}
}"#;
        let map = parse(source).unwrap();
        let brush = &map.entities[0].brushes[0];
        assert_eq!(brush.style, BrushStyle::BrushDef3);
        assert_eq!(brush.faces[0].plane.distance, 64.0);
    }

    #[test]
    fn valve_220_axes_are_retained() {
        let source = r#"{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 0 64 ) ( 0 64 0 ) test/a [ 0 1 0 16 ] [ 0 0 -1 -8 ] 30 0.5 0.25 1 2 3
( 64 0 0 ) ( 64 64 0 ) ( 64 0 64 ) test/a [ 0 1 0 0 ] [ 0 0 -1 0 ] 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 0 64 ) test/a [ 1 0 0 0 ] [ 0 0 -1 0 ] 0 1 1
( 0 64 0 ) ( 0 64 64 ) ( 64 64 0 ) test/a [ 1 0 0 0 ] [ 0 0 -1 0 ] 0 1 1
}
}"#;
        let map = parse(source).unwrap();
        let brush = &map.entities[0].brushes[0];
        assert_eq!(brush.style, BrushStyle::Valve220);
        assert!(matches!(
            brush.faces[0].projection,
            TextureProjection::Valve220 { u_shift, v_shift, rotate, scale, .. }
                if u_shift == 16.0 && v_shift == -8.0 && rotate == 30.0 && scale == [0.5, 0.25]
        ));
    }

    #[test]
    fn degenerate_face_is_counted_without_losing_the_brush_object() {
        let source = BOX.replace(
            "( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall",
            "( 0 0 0 ) ( 1 1 1 ) ( 2 2 2 ) test/wall",
        );
        let map = parse(&source).unwrap();
        assert_eq!(map.stats.degenerate_faces, 1);
        assert_eq!(map.entities[0].brushes[0].faces.len(), 5);
    }

    #[test]
    fn patches_and_nonworld_entities_preserve_boundaries() {
        let source = format!(
            "{}\n{{\n\"classname\" \"func_static\"\n{{\npatchDef2\n{{\ntest/a\n( 3 3 0 0 0 )\n(\n)\n}}\n}}\n}}",
            BOX
        );
        let map = parse(&source).unwrap();
        assert_eq!(map.entities.len(), 2);
        assert_eq!(map.entities[1].classname(), Some("func_static"));
        assert_eq!(map.stats.patches_skipped, 1);
    }

    #[test]
    fn malformed_input_reports_an_error() {
        assert!(parse("{ \"classname\" \"worldspawn\" {").is_err());
        assert!(parse("{ \"classname\" \"worldspawn").is_err());
    }
}
