//! Build-time extraction of Settings controls. Source labels remain the single
//! authority; search never requires maintaining a second manual settings list.
use std::{collections::HashSet, fs, path::Path};

const SOURCES: &[(&str, usize, &str)] = &[
    ("src/app/egui_settings/game.rs", 0, "None"),
    ("src/app/egui_menu/movement.rs", 1, "None"),
    ("src/app/egui_pages.rs", 2, "None"),
    ("src/app/egui_pages.rs", 5, "None"),
    ("src/app/egui_pages.rs", 6, "None"),
    ("src/app/egui_pages.rs", 9, "None"),
    ("src/app/egui_settings/mod_options.rs", 1, "None"),
    ("src/app/egui_settings/mod_options.rs", 9, "None"),
    ("src/app/egui_settings/display.rs", 3, "Display"),
    ("src/app/egui_settings/image.rs", 3, "Display"),
    ("src/app/egui_settings/image.rs", 3, "Geometry"),
    ("src/app/egui_settings/geometry.rs", 3, "Geometry"),
    ("src/app/egui_settings/models.rs", 3, "Geometry"),
    ("src/app/egui_settings/lighting.rs", 3, "Lighting"),
    ("src/app/egui_settings/materials.rs", 3, "Materials"),
    ("src/app/egui_settings/effects.rs", 3, "Effects"),
    ("src/app/egui_settings/reflections.rs", 3, "Effects"),
    ("src/app/egui_settings/shadows.rs", 3, "Lighting"),
    ("src/app/egui_settings/post.rs", 3, "PostProcessing"),
    ("src/app/egui_settings/color.rs", 3, "Color"),
    ("src/app/egui_settings/debug.rs", 3, "DebugTools"),
    ("src/app/egui_settings/debug.rs", 3, "Lighting"),
    ("src/app/egui_settings/physics.rs", 3, "Physics"),
    ("src/app/egui_settings/sun.rs", 3, "Sun"),
    ("src/app/egui_settings/clouds.rs", 3, "Clouds"),
    ("src/app/egui_settings/weather.rs", 3, "Weather"),
    ("src/app/egui_settings/native_environment.rs", 3, "Clouds"),
    ("src/app/egui_settings/native_environment.rs", 3, "Sun"),
    ("src/app/egui_settings/native_environment.rs", 3, "Weather"),
    ("src/app/egui_settings/water.rs", 3, "Surface"),
    ("src/app/egui_settings/water.rs", 3, "Water"),
    ("src/app/egui_settings/blue_water.rs", 3, "Water"),
    ("src/app/egui_settings/fog.rs", 3, "Weather"),
    ("src/app/egui_settings/glass.rs", 3, "Glass"),
    ("src/app/egui_settings/optimizations.rs", 3, "Optimizations"),
    ("src/app/egui_menu/audio.rs", 4, "None"),
    ("src/app/egui_settings/interface.rs", 6, "None"),
    ("src/app/egui_settings/extensions.rs", 7, "None"),
    ("src/app/egui_settings/streamer.rs", 8, "None"),
];
// Every item here is a setting-row constructor, not the UI's text display.
const FUNCTIONS: &[&str] = &[
    "theme::row", "theme::row_with_note", "segmented_row", "quality_row",
    "quality_table_row", "optional_quality_table_row", "mode_row", "color_row",
    "ocean_slider", "ocean_slider_d", "ocean_slider_dd", "ocean_slider_metres_d",
    "linked_ocean_slider", "audio_slider!", "camera_number!",
    "self.egui_physics_toggle", "self.egui_physics_slider", "self.egui_physics_number",
    "self.egui_toggle_row", "self.egui_environment_toggle", "self.audio_choice_row",
    "self.cvar_choice_row", "self.cvar_switch_row",
    "sky_slider", "locked_slider", "slider", "slider_with", "choice",
];

fn literal(s: &str) -> Option<&str> {
    let trimmed = s.trim();
    if !trimmed.starts_with('"') { return None; }
    let mut escaped = false;
    for (index, &byte) in trimmed.as_bytes().iter().enumerate().skip(1) {
        if escaped { escaped = false; }
        else if byte == b'\\' { escaped = true; }
        else if byte == b'"' {
            return (index == trimmed.len() - 1).then_some(trimmed);
        }
    }
    None
}

/// Split only the leading arguments of a Rust call; skip nested expressions and
/// whitespace inside string literals. No third-party parser/build dependency.
fn leading_args(s: &str, needed: usize) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let (mut paren, mut square, mut curly) = (0i32, 0i32, 0i32);
    let (mut in_string, mut escape) = (false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            if escape { escape = false; }
            else if b == b'\\' { escape = true; }
            else if b == b'"' { in_string = false; }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'(' => paren += 1,
            b')' if paren == 0 && square == 0 && curly == 0 => break,
            b')' => paren -= 1,
            b'[' => square += 1,
            b']' => square -= 1,
            b'{' => curly += 1,
            b'}' => curly -= 1,
            b',' if paren == 0 && square == 0 && curly == 0 => {
                out.push(s[start..i].trim().to_owned());
                if out.len() == needed { break; }
                start = i + 1;
            }
            _ => {}
        }
    }
    out
}

pub fn generate(manifest: &Path, out: &Path) -> std::io::Result<()> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for &(path, tab, section) in SOURCES {
        println!("cargo:rerun-if-changed={path}");
        let text = fs::read_to_string(manifest.join(path))?;
        // Shared impl files contain multiple pages. Bound each extraction to
        // the actual page to avoid misleading category/search destinations.
        let text: &str = if path.ends_with("egui_pages.rs") {
            let camera = text.find("fn egui_camera_page(").unwrap();
            let network = text.find("fn egui_network_page(").unwrap();
            let vote = text.find("fn egui_vote_page(").unwrap();
            let hud = text.find("fn egui_japro_hud_settings(").unwrap();
            let game = text.find("fn egui_japro_game_settings(").unwrap();
            let companion = text.find("fn egui_companion_window_selector(").unwrap();
            match tab {
                2 => &text[camera..network],
                5 => &text[network..vote],
                6 => &text[hud..game],
                9 => &text[game..companion],
                _ => "",
            }
        } else if path.ends_with("mod_options.rs") {
            let movement = text.find("fn egui_mod_movement_settings(").unwrap();
            let setup = text.find("fn egui_mod_setup_settings(").unwrap();
            match tab { 1 => &text[movement..setup], 9 => &text[setup..], _ => "" }
        } else if path.ends_with("native_environment.rs") {
            let clouds = text.find("fn egui_native_clouds(").unwrap();
            let moon = text.find("fn egui_native_moon(").unwrap();
            let weather = text.find("fn egui_native_weather(").unwrap();
            match section {
                "Clouds" => &text[clouds..moon],
                "Sun" => &text[moon..weather],
                "Weather" => &text[weather..],
                _ => "",
            }
        } else if path.ends_with("water.rs") && !path.ends_with("blue_water.rs") {
            let surface = text.find("fn egui_surface(").unwrap();
            let water = text.find("fn egui_water(").unwrap();
            if section == "Surface" { &text[surface..water] } else { &text[water..] }
        } else if path.ends_with("image.rs") {
            let visibility = text.find("fn egui_visibility(").unwrap();
            if section == "Display" { &text[..visibility] } else { &text[visibility..] }
        } else if path.ends_with("debug.rs") {
            let baked = text.find("fn egui_baked_ao(").unwrap();
            if section == "DebugTools" { &text[..baked] } else { &text[baked..] }
        } else { &text };
        for &function in FUNCTIONS {
            let needle = format!("{function}(");
            for (start, _) in text.match_indices(&needle) {
                // Avoid matching a suffix of another method's name.
                if start > 0 && (text.as_bytes()[start - 1].is_ascii_alphanumeric()
                    || text.as_bytes()[start - 1] == b'_') { continue; }
                let args = leading_args(&text[start + needle.len()..], 4);
                if args.len() < 3 { continue; }
                let label_index = if function.ends_with('!') { 0 } else { 1 };
                if label_index == 1 && args[0] != "ui" { continue; }
                let Some(label) = literal(&args[label_index]) else { continue; };
                let hint = literal(&args[label_index + 1]).unwrap_or("\"\"");
                let alias = if matches!(function, "self.cvar_choice_row" | "self.cvar_switch_row") {
                    args.get(3).and_then(|s| literal(s)).unwrap_or("\"\"")
                } else { "\"\"" };
                if seen.insert((tab, section, label.to_owned())) {
                    let section = if section == "None" { "None".to_owned() }
                                  else { format!("Some(VideoSection::{section})") };
                    rows.push(format!("    SearchItem {{ tab: {tab}, section: {section}, label: {label}, hint: {hint}, alias: {alias} }},\n"));
                }
            }
        }
        // Some pages draw rows from inline (label, tooltip, field, ...) tables
        // instead of invoking the row helper once per literal. Those entries
        // are just as searchable and jump to the same runtime theme::row.
        for (pos, _) in text.match_indices("for (label, tip,") {
            let rest = &text[pos..];
            let Some(table_start) = rest.find(" in [") else { continue };
            let rest = &rest[table_start + " in [".len()..];
            let Some(table_end) = rest.find("] {") else { continue };
            let table = &rest[..table_end];
            for (start, _) in table.match_indices("(\"") {
                let args = leading_args(&table[start + 1..], 3);
                if args.len() < 3 { continue; }
                let (Some(label), Some(hint)) = (literal(&args[0]), literal(&args[1])) else { continue; };
                if seen.insert((tab, section, label.to_owned())) {
                    let section = if section == "None" { "None".to_owned() }
                                  else { format!("Some(VideoSection::{section})") };
                    rows.push(format!("    SearchItem {{ tab: {tab}, section: {section}, label: {label}, hint: {hint}, alias: \"\" }},\n"));
                }
            }
        }
    }
    let mut output = String::from("// Generated at build-time from settings row labels. Do not edit.\nconst SEARCH_ITEMS: &[SearchItem] = &[\n");
    for row in rows { output.push_str(&row); }
    output.push_str("];\n");
    fs::write(out, output)
}
