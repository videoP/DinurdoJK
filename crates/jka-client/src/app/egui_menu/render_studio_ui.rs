//! Render Studio's editor chrome: panes, sections, property rows and vector
//! icons. Layout follows NLE conventions (Premiere / Resolve: media browser,
//! program viewer with a transport bar, inspector, timeline with a fixed
//! ruler). Colors come from `crate::palette`: warm Stone Brown surfaces,
//! Celadon as the primary accent, Smoky Rose as the secondary.
use self::palette as theme;
use egui::{pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui};

/// Studio palette. Same token names as the global menu theme, so Studio
/// files import it as `theme`; a shade darker than the menus so footage in
/// the viewer stays the brightest thing on screen.
pub(super) mod palette {
    use crate::palette::{mix, CELADON, INK, SILVER, SMOKY_ROSE, STONE_BROWN, WHITE_SMOKE};
    use egui::Color32;
    const fn rgb(c: [u8; 3]) -> Color32 { Color32::from_rgb(c[0], c[1], c[2]) }
    /// Editor background between panes.
    pub(in crate::app::egui_menu) const CHROME: Color32 = rgb(mix(INK, [0, 0, 0], 0.15));
    pub(in crate::app::egui_menu) const SURFACE: Color32 = rgb(mix(INK, STONE_BROWN, 0.14));
    pub(in crate::app::egui_menu) const SURFACE_ALT: Color32 = rgb(mix(INK, STONE_BROWN, 0.26));
    pub(in crate::app::egui_menu) const INSET: Color32 = rgb(mix(INK, [0, 0, 0], 0.30));
    pub(in crate::app::egui_menu) const CONTROL: Color32 = rgb(mix(INK, STONE_BROWN, 0.40));
    pub(in crate::app::egui_menu) const CONTROL_HOVER: Color32 = rgb(mix(INK, STONE_BROWN, 0.60));
    /// Deep Celadon: selected rows and clips.
    pub(in crate::app::egui_menu) const CONTROL_SELECTED: Color32 = rgb(mix(INK, CELADON, 0.30));
    pub(in crate::app::egui_menu) const HAIRLINE: Color32 = rgb(mix(INK, STONE_BROWN, 0.36));
    pub(in crate::app::egui_menu) const LINE: Color32 = rgb(mix(INK, STONE_BROWN, 0.62));
    pub(in crate::app::egui_menu) const LINE_STRONG: Color32 = rgb(STONE_BROWN);
    pub(in crate::app::egui_menu) const ACCENT: Color32 = rgb(CELADON);
    pub(in crate::app::egui_menu) const ACCENT_DEEP: Color32 = rgb(mix(CELADON, INK, 0.55));
    pub(in crate::app::egui_menu) const ACCENT_HOVER: Color32 = rgb(mix(CELADON, INK, 0.35));
    /// Smoky Rose, lifted for contrast: secondary accent (title ticks, playhead).
    pub(in crate::app::egui_menu) const MAGENTA: Color32 = rgb(mix(SMOKY_ROSE, WHITE_SMOKE, 0.25));
    pub(in crate::app::egui_menu) const TEXT: Color32 = rgb(mix(WHITE_SMOKE, SILVER, 0.25));
    pub(in crate::app::egui_menu) const TEXT_BRIGHT: Color32 = rgb(WHITE_SMOKE);
    pub(in crate::app::egui_menu) const TEXT_DIM: Color32 = rgb(SILVER);
    pub(in crate::app::egui_menu) const TEXT_FAINT: Color32 = rgb(mix(SILVER, INK, 0.30));
    pub(in crate::app::egui_menu) const TEXT_DISABLED: Color32 = rgb(mix(SILVER, INK, 0.55));
    pub(in crate::app::egui_menu) const WARNING: Color32 = rgb([0xE6, 0xC2, 0x6E]);
    pub(in crate::app::egui_menu) const DANGER: Color32 = rgb(mix(SMOKY_ROSE, WHITE_SMOKE, 0.30));
}

/// Editor background; shows as the thin gutters between panes.
pub(super) const GUTTER: Color32 = theme::CHROME;
pub(super) const PANE: Color32 = theme::SURFACE;
pub(super) const PANE_HEADER: Color32 = theme::SURFACE_ALT;
/// Recessed wells: viewer letterbox, ruler, track lanes, text fields.
pub(super) const WELL: Color32 = theme::INSET;
pub(super) const LANE_A: Color32 = Color32::from_rgb(0x2A, 0x26, 0x21);
pub(super) const LANE_B: Color32 = Color32::from_rgb(0x22, 0x1F, 0x1B);
pub(super) const PLAYHEAD: Color32 = theme::MAGENTA;
pub(super) const DEMO: Color32 = Color32::from_rgb(168, 124, 255);
pub(super) const AUDIO: Color32 = Color32::from_rgb(78, 226, 168);
pub(super) const SPEED: Color32 = Color32::from_rgb(255, 104, 196);
pub(super) const CAMERA: Color32 = theme::WARNING;
pub(super) const SCENE: Color32 = Color32::from_rgb(110, 106, 214);
pub(super) const AXIS: [Color32; 3] = [
    Color32::from_rgb(255, 92, 112),
    Color32::from_rgb(96, 232, 140),
    Color32::from_rgb(80, 170, 255),
];

// ---------------------------------------------------------------- metrics --

pub(super) const PANE_GAP: f32 = 3.0;
pub(super) const PANE_HEADER_H: f32 = 30.0;
pub(super) const PANE_FOOTER_H: f32 = 24.0;
pub(super) const TRANSPORT_H: f32 = 40.0;
pub(super) const COLLAPSED_W: f32 = 30.0;
pub(super) const PAD: f32 = 10.0;
pub(super) const ROW_H: f32 = 24.0;
pub(super) const TIMELINE_HEADER_H: f32 = 32.0;
pub(super) const RULER_H: f32 = 28.0;
pub(super) const SCROLLBAR_H: f32 = 14.0;
pub(super) const TRACK_HEADER_W: f32 = 184.0;
pub(super) const LANE_H: f32 = 34.0;

/// Darkened clip body for a track color; the saturated color is kept for the
/// clip's top band, so clips read by type without glaring.
pub(super) fn clip_body(color: Color32, selected: bool) -> Color32 {
    WELL.lerp_to_gamma(color, if selected { 0.62 } else { 0.40 })
}

/// Studio panes use slightly rounded controls and wrapped text. Called once
/// per pane scope; the global menu theme stays square elsewhere.
pub(super) fn editor_style(ui: &mut Ui) {
    let style = ui.style_mut();
    style.wrap_mode = Some(egui::TextWrapMode::Wrap);
    style.spacing.item_spacing = vec2(6.0, 5.0);
    style.spacing.button_padding = vec2(8.0, 3.0);
    style.spacing.interact_size = vec2(22.0, 22.0);
    style.spacing.indent = 12.0;
    let radius = egui::CornerRadius::same(3);
    for w in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        w.corner_radius = radius;
    }
    apply_palette(&mut style.visuals);
}

fn widget(fill: Color32, stroke: Color32, text: Color32) -> egui::style::WidgetVisuals {
    egui::style::WidgetVisuals {
        bg_fill: fill, weak_bg_fill: fill,
        bg_stroke: Stroke::new(1.0, stroke),
        corner_radius: egui::CornerRadius::same(3),
        fg_stroke: Stroke::new(1.0, text),
        expansion: 0.0,
    }
}

/// Console / neon palette for every egui widget inside a Studio scope.
pub(super) fn apply_palette(v: &mut egui::Visuals) {
    v.panel_fill = theme::SURFACE;
    v.window_fill = theme::SURFACE;
    v.window_stroke = Stroke::new(1.0, theme::LINE_STRONG);
    v.extreme_bg_color = WELL;
    v.faint_bg_color = theme::SURFACE_ALT;
    v.code_bg_color = WELL;
    v.hyperlink_color = theme::ACCENT;
    v.warn_fg_color = theme::WARNING;
    v.error_fg_color = theme::DANGER;
    v.weak_text_color = Some(theme::TEXT_FAINT);
    v.selection.bg_fill = theme::CONTROL_SELECTED;
    v.selection.stroke = Stroke::new(1.0, theme::ACCENT);
    v.widgets.noninteractive = widget(theme::SURFACE, theme::HAIRLINE, theme::TEXT_DIM);
    v.widgets.inactive = widget(theme::CONTROL, theme::LINE, theme::TEXT_DIM);
    v.widgets.hovered = widget(theme::CONTROL_HOVER, theme::LINE_STRONG, theme::TEXT);
    v.widgets.active = widget(theme::CONTROL_SELECTED, theme::ACCENT, theme::TEXT_BRIGHT);
    v.widgets.open = widget(theme::CONTROL_HOVER, theme::LINE_STRONG, theme::TEXT);
    // Slider rails and check boxes read inactive / hovered bg_fill.
    v.widgets.inactive.bg_fill = theme::LINE;
    v.widgets.hovered.bg_fill = theme::LINE_STRONG;
}

/// Popup / combo style with the Studio palette (popups otherwise take the
/// global menu theme).
pub(super) fn popup_style() -> egui::style::StyleModifier {
    egui::style::StyleModifier::new(|style: &mut egui::Style| apply_palette(&mut style.visuals))
}

/// Menu-button popups: egui's frameless menu style plus the Studio palette.
pub(super) fn menu_style() -> egui::style::StyleModifier {
    egui::style::StyleModifier::new(|style: &mut egui::Style| {
        egui::containers::menu::menu_style(style);
        apply_palette(&mut style.visuals);
    })
}

/// A combo box whose popup uses the Studio palette.
pub(super) fn combo_box(id: impl std::hash::Hash + std::fmt::Debug) -> egui::ComboBox {
    egui::ComboBox::from_id_salt(id).popup_style(popup_style())
}

/// A settings trigger keeps its identity as earlier toolbar content changes.
pub(super) fn settings_button(ui:&mut Ui,id:impl std::hash::Hash+std::fmt::Debug,label:&str,enabled:bool)->egui::Response {
    // id_salt still includes the parent's changing automatic widget count.
    // A global child ID also stabilizes the automatic ID of the painted button.
    ui.scope_builder(egui::UiBuilder::new().id(ui.make_persistent_id(id)),|ui|
        button(ui,None,label,ButtonKind::Normal,enabled)).inner
}

/// Settings contain interactive controls, including combos. egui's single
/// memory-backed popup slot belongs to those combos, not their parent panel.
pub(super) fn settings_popup<R>(button:&egui::Response,open:&mut bool,content:impl FnOnce(&mut Ui)->R)
    ->Option<egui::InnerResponse<R>> {
    if button.clicked() {*open=!*open;}
    let child_open=egui::Popup::is_any_open(&button.ctx);
    egui::Popup::menu(button).open_bool(open).style(menu_style())
        .close_behavior(if child_open {egui::PopupCloseBehavior::IgnoreClicks}
            else {egui::PopupCloseBehavior::CloseOnClickOutside}).show(content)
}

/// Console-style neon edge: a cyan-to-magenta gradient strip.
pub(super) fn neon_edge(p: &egui::Painter, rect: Rect) {
    let mut mesh = egui::Mesh::default();
    let (l, r) = (theme::ACCENT.gamma_multiply(0.9), theme::MAGENTA.gamma_multiply(0.9));
    mesh.colored_vertex(rect.left_top(), l);
    mesh.colored_vertex(rect.right_top(), r);
    mesh.colored_vertex(rect.right_bottom(), r);
    mesh.colored_vertex(rect.left_bottom(), l);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(egui::Shape::mesh(mesh));
}

// ------------------------------------------------------------------ icons --

/// Vector icons painted with egui shapes. The bundled font atlas lacks most
/// symbol glyphs, so no icon depends on font coverage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Icon {
    Play, Pause, ToStart, ToEnd, StepBack, StepForward,
    AddKey, Frame, Snap, Undo, Redo, Open, Save, More,
    ChevronLeft, ChevronRight, ChevronDown, Eye, EyeOff, Search, Plus, Close,
    Center, Help, Anim, Emote, Cinematic, Demo, Camera, Audio, Export, Layers,
    ZoomIn, ZoomOut, Keys, Path, Still, Settings,
}

pub(super) fn paint_icon(p: &egui::Painter, c: Pos2, icon: Icon, color: Color32) {
    let s = Stroke::new(1.6, color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment([c + vec2(a.0, a.1), c + vec2(b.0, b.1)], s);
    };
    let fill = |points: &[(f32, f32)]| {
        p.add(egui::Shape::convex_polygon(
            points.iter().map(|&(x, y)| c + vec2(x, y)).collect(),
            color,
            Stroke::NONE,
        ));
    };
    let diamond = |center: Pos2, r: f32| {
        p.add(egui::Shape::convex_polygon(
            vec![center + vec2(0.0, -r), center + vec2(r, 0.0), center + vec2(0.0, r), center + vec2(-r, 0.0)],
            color,
            Stroke::NONE,
        ));
    };
    match icon {
        Icon::Play => fill(&[(-4.0, -6.5), (6.5, 0.0), (-4.0, 6.5)]),
        Icon::Pause => {
            p.rect_filled(Rect::from_center_size(c + vec2(-3.5, 0.0), vec2(3.0, 12.0)), 0.5, color);
            p.rect_filled(Rect::from_center_size(c + vec2(3.5, 0.0), vec2(3.0, 12.0)), 0.5, color);
        }
        Icon::ToStart => {
            p.rect_filled(Rect::from_center_size(c + vec2(-6.0, 0.0), vec2(2.0, 12.0)), 0.0, color);
            fill(&[(-4.0, 0.0), (6.0, -6.0), (6.0, 6.0)]);
        }
        Icon::ToEnd => {
            p.rect_filled(Rect::from_center_size(c + vec2(6.0, 0.0), vec2(2.0, 12.0)), 0.0, color);
            fill(&[(4.0, 0.0), (-6.0, -6.0), (-6.0, 6.0)]);
        }
        Icon::StepBack => {
            p.rect_filled(Rect::from_center_size(c + vec2(4.5, 0.0), vec2(2.0, 10.0)), 0.0, color);
            fill(&[(-5.5, 0.0), (2.5, -5.0), (2.5, 5.0)]);
        }
        Icon::StepForward => {
            p.rect_filled(Rect::from_center_size(c + vec2(-4.5, 0.0), vec2(2.0, 10.0)), 0.0, color);
            fill(&[(5.5, 0.0), (-2.5, -5.0), (-2.5, 5.0)]);
        }
        Icon::AddKey => {
            diamond(c + vec2(-2.5, 0.0), 5.5);
            line((4.5, -6.5), (4.5, -0.5));
            line((1.5, -3.5), (7.5, -3.5));
        }
        Icon::Frame => {
            for (a, b) in [
                ((-7.0, -3.0), (-7.0, -7.0)), ((-7.0, -7.0), (-3.0, -7.0)),
                ((3.0, -7.0), (7.0, -7.0)), ((7.0, -7.0), (7.0, -3.0)),
                ((-7.0, 3.0), (-7.0, 7.0)), ((-7.0, 7.0), (-3.0, 7.0)),
                ((3.0, 7.0), (7.0, 7.0)), ((7.0, 7.0), (7.0, 3.0)),
            ] {
                line(a, b);
            }
            p.circle_stroke(c, 2.2, s);
        }
        Icon::Snap => {
            // Magnet: a U with two pole caps.
            let arc: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 10.0;
                    c + vec2(-5.0 * a.cos(), 1.0 + 5.0 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(arc, s));
            line((-5.0, 1.0), (-5.0, -6.0));
            line((5.0, 1.0), (5.0, -6.0));
            p.rect_filled(Rect::from_min_max(c + vec2(-6.5, -7.0), c + vec2(-3.5, -4.0)), 0.0, color);
            p.rect_filled(Rect::from_min_max(c + vec2(3.5, -7.0), c + vec2(6.5, -4.0)), 0.0, color);
        }
        Icon::Undo | Icon::Redo => {
            let d = if icon == Icon::Undo { 1.0 } else { -1.0 };
            let arc: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 12.0;
                    c + vec2(d * (1.0 + 5.0 * a.cos()), 1.0 + 5.0 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(arc, s));
            line((d * 1.0, -4.0), (d * -6.0, -4.0));
            fill(&[(d * -7.0, -4.0), (d * -3.0, -7.5), (d * -3.0, -0.5)]);
        }
        Icon::Open => {
            p.add(egui::Shape::closed_line(
                [(-7.0, -6.0), (-3.0, -6.0), (-1.0, -4.0), (7.0, -4.0), (7.0, 6.0), (-7.0, 6.0)]
                    .iter().map(|&(x, y)| c + vec2(x, y)).collect(),
                s,
            ));
            line((-7.0, -1.0), (7.0, -1.0));
        }
        Icon::Save => {
            p.rect_stroke(Rect::from_center_size(c, vec2(13.0, 13.0)), 1.5, s, egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_center_size(c + vec2(0.0, -3.5), vec2(7.0, 3.5)), 0.0, color);
            p.rect_stroke(Rect::from_center_size(c + vec2(0.0, 3.5), vec2(7.0, 5.0)), 0.0, s, egui::StrokeKind::Inside);
        }
        Icon::More => {
            for x in [-5.0, 0.0, 5.0] {
                p.circle_filled(c + vec2(x, 0.0), 1.6, color);
            }
        }
        Icon::ChevronLeft => {
            line((2.5, -5.0), (-2.5, 0.0));
            line((-2.5, 0.0), (2.5, 5.0));
        }
        Icon::ChevronRight => {
            line((-2.5, -5.0), (2.5, 0.0));
            line((2.5, 0.0), (-2.5, 5.0));
        }
        Icon::ChevronDown => {
            line((-5.0, -2.5), (0.0, 2.5));
            line((0.0, 2.5), (5.0, -2.5));
        }
        Icon::Eye | Icon::EyeOff => {
            let top: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    c + vec2(-7.0 + 14.0 * t, -4.5 * (std::f32::consts::PI * t).sin())
                })
                .collect();
            let bottom: Vec<Pos2> = top.iter().map(|q| pos2(q.x, 2.0 * c.y - q.y)).collect();
            p.add(egui::Shape::line(top, s));
            p.add(egui::Shape::line(bottom, s));
            p.circle_filled(c, 2.2, color);
            if icon == Icon::EyeOff {
                line((-6.0, 6.0), (6.0, -6.0));
            }
        }
        Icon::Search | Icon::ZoomIn | Icon::ZoomOut => {
            p.circle_stroke(c + vec2(-1.5, -1.5), 4.5, s);
            line((1.8, 1.8), (6.0, 6.0));
            if icon != Icon::Search {
                line((-4.0, -1.5), (1.0, -1.5));
            }
            if icon == Icon::ZoomIn {
                line((-1.5, -4.0), (-1.5, 1.0));
            }
        }
        Icon::Plus => {
            line((-5.5, 0.0), (5.5, 0.0));
            line((0.0, -5.5), (0.0, 5.5));
        }
        Icon::Close => {
            line((-4.5, -4.5), (4.5, 4.5));
            line((-4.5, 4.5), (4.5, -4.5));
        }
        Icon::Center => {
            line((0.0, -7.0), (0.0, 7.0));
            fill(&[(-7.0, -4.0), (-2.5, 0.0), (-7.0, 4.0)]);
            fill(&[(7.0, -4.0), (2.5, 0.0), (7.0, 4.0)]);
        }
        Icon::Help => {
            p.circle_stroke(c, 6.5, Stroke::new(1.2, color));
            p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "?", FontId::proportional(10.0), color);
        }
        Icon::Anim => {
            // Film strip: frame with sprocket ticks.
            p.rect_stroke(Rect::from_center_size(c, vec2(14.0, 11.0)), 1.5, s, egui::StrokeKind::Inside);
            for x in [-3.5, 0.0, 3.5] {
                line((x, -5.5), (x, -3.5));
                line((x, 3.5), (x, 5.5));
            }
            fill(&[(-1.5, -2.0), (2.5, 0.0), (-1.5, 2.0)]);
        }
        Icon::Emote => {
            // Music note.
            p.circle_filled(c + vec2(-2.5, 4.0), 2.6, color);
            line((-0.2, 4.0), (-0.2, -6.0));
            line((-0.2, -6.0), (5.0, -4.0));
            line((5.0, -4.0), (5.0, -1.5));
        }
        Icon::Cinematic => {
            // Clapperboard.
            p.rect_stroke(Rect::from_min_max(c + vec2(-7.0, -1.5), c + vec2(7.0, 6.5)), 1.0, s, egui::StrokeKind::Inside);
            p.add(egui::Shape::convex_polygon(
                vec![c + vec2(-7.0, -2.5), c + vec2(6.0, -6.5), c + vec2(6.8, -4.0), c + vec2(-6.2, 0.0)],
                color, Stroke::NONE,
            ));
        }
        Icon::Demo => {
            // Monitor with a play triangle: recorded POV footage.
            p.rect_stroke(Rect::from_min_max(c + vec2(-7.0, -6.0), c + vec2(7.0, 3.5)), 1.5, s, egui::StrokeKind::Inside);
            line((-3.0, 6.5), (3.0, 6.5));
            line((0.0, 3.5), (0.0, 6.5));
            fill(&[(-1.5, -3.5), (2.5, -1.25), (-1.5, 1.0)]);
        }
        Icon::Camera => {
            p.rect_stroke(Rect::from_min_max(c + vec2(-7.0, -4.0), c + vec2(3.0, 4.5)), 1.5, s, egui::StrokeKind::Inside);
            fill(&[(3.0, 0.25), (7.5, -3.5), (7.5, 4.0)]);
        }
        Icon::Audio => {
            fill(&[(-6.5, -2.5), (-3.5, -2.5), (0.5, -6.0), (0.5, 6.0), (-3.5, 2.5), (-6.5, 2.5)]);
            line((3.0, -2.5), (3.0, 2.5));
            line((5.5, -5.0), (5.5, 5.0));
        }
        Icon::Export => {
            p.add(egui::Shape::line(
                [(-3.0, -4.0), (-7.0, -4.0), (-7.0, 7.0), (7.0, 7.0), (7.0, -4.0), (3.0, -4.0)]
                    .iter().map(|&(x, y)| c + vec2(x, y)).collect(),
                s,
            ));
            line((0.0, 3.0), (0.0, -7.0));
            line((0.0, -7.0), (-3.5, -3.5));
            line((0.0, -7.0), (3.5, -3.5));
        }
        Icon::Layers => {
            for dy in [-3.0, 1.0, 5.0] {
                p.add(egui::Shape::closed_line(
                    vec![c + vec2(0.0, dy - 4.0), c + vec2(7.0, dy - 1.0), c + vec2(0.0, dy + 2.0), c + vec2(-7.0, dy - 1.0)],
                    Stroke::new(1.2, color),
                ));
            }
        }
        Icon::Keys => {
            diamond(c + vec2(-4.0, 0.0), 3.5);
            diamond(c + vec2(4.0, 0.0), 3.5);
        }
        Icon::Path => {
            let curve: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let t = i as f32 / 12.0;
                    c + vec2(-7.0 + 14.0 * t, 4.0 * (std::f32::consts::TAU * t).sin() * 0.9)
                })
                .collect();
            p.add(egui::Shape::line(curve, s));
            p.circle_filled(c + vec2(-7.0, 0.0), 1.8, color);
            p.circle_filled(c + vec2(7.0, 0.0), 1.8, color);
        }
        Icon::Settings => {
            // Gear: eight teeth around a ring.
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * 4.5, c + d * 7.0], Stroke::new(2.2, color));
            }
            p.circle_stroke(c, 4.6, s);
            p.circle_filled(c, 1.6, color);
        }
        Icon::Still => {
            p.rect_stroke(Rect::from_min_max(c + vec2(-7.0, -4.5), c + vec2(7.0, 6.0)), 1.5, s, egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_min_max(c + vec2(-3.0, -6.5), c + vec2(2.0, -4.5)), 0.0, color);
            p.circle_stroke(c + vec2(0.0, 0.8), 2.8, s);
        }
    }
}

// --------------------------------------------------------------- buttons --

/// Frameless 26-pt icon button: chrome only on hover/active, like NLE toolbars.
pub(super) fn icon_button(ui: &mut Ui, icon: Icon, enabled: bool, active: bool, tip: &str) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(26.0, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let fill = if active {
        theme::CONTROL_SELECTED
    } else if enabled && r.is_pointer_button_down_on() {
        theme::CONTROL_HOVER
    } else if enabled && r.hovered() {
        theme::CONTROL
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 3.0, fill);
    let color = if !enabled {
        theme::TEXT_DISABLED
    } else if active {
        theme::ACCENT
    } else if r.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    paint_icon(p, rect.center(), icon, color);
    r.on_hover_text(tip)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ButtonKind { Normal, Primary, Danger, Ghost }

/// Text button with an optional leading icon, sized to the shared 26-pt row.
pub(super) fn button(ui: &mut Ui, icon: Option<Icon>, label: &str, kind: ButtonKind, enabled: bool) -> egui::Response {
    button_sized(ui, icon, label, kind, enabled, None)
}

/// As `button`, with an explicit width (`None` sizes to the label).
pub(super) fn button_sized(ui: &mut Ui, icon: Option<Icon>, label: &str, kind: ButtonKind,
    enabled: bool, width: Option<f32>) -> egui::Response {
    let font = FontId::proportional(12.5);
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font.clone(), theme::TEXT);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let natural = galley.size().x + icon_w + 20.0;
    let w = width.unwrap_or(natural).max(26.0);
    let (rect, r) = ui.allocate_exact_size(vec2(w, 26.0), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && r.hovered();
    let pressed = enabled && r.is_pointer_button_down_on();
    let (fill, stroke, text) = match kind {
        ButtonKind::Primary => (
            if pressed { theme::ACCENT_DEEP } else if hovered { theme::ACCENT_HOVER } else { theme::ACCENT_DEEP },
            if hovered { theme::ACCENT } else { theme::ACCENT.gamma_multiply(0.55) },
            theme::TEXT_BRIGHT,
        ),
        ButtonKind::Danger => (
            if hovered { Color32::from_rgb(0x5A, 0x34, 0x2F) } else { theme::CONTROL },
            if hovered { theme::DANGER } else { theme::LINE },
            theme::DANGER,
        ),
        ButtonKind::Ghost => (
            if hovered { theme::CONTROL } else { Color32::TRANSPARENT },
            if hovered { theme::LINE } else { Color32::TRANSPARENT },
            if hovered { theme::TEXT } else { theme::TEXT_DIM },
        ),
        ButtonKind::Normal => (
            if pressed { theme::CONTROL_SELECTED } else if hovered { theme::CONTROL_HOVER } else { theme::CONTROL },
            if hovered { theme::LINE_STRONG } else { theme::LINE },
            if hovered { theme::TEXT } else { theme::TEXT_DIM },
        ),
    };
    let (fill, stroke, text) = if enabled {
        (fill, stroke, text)
    } else {
        (theme::CONTROL.gamma_multiply(0.6), theme::HAIRLINE, theme::TEXT_DISABLED)
    };
    let p = ui.painter();
    p.rect_filled(rect, 3.0, fill);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let content_w = (galley.size().x + icon_w).min(rect.width() - 12.0);
    let mut x = rect.center().x - content_w * 0.5;
    if let Some(icon) = icon {
        paint_icon(p, pos2(x + 8.0, rect.center().y), icon, text);
        x += icon_w;
    }
    p.with_clip_rect(rect.shrink(4.0))
        .text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, font, text);
    r
}

/// Equal-width buttons filling the row. Returns the clicked index.
pub(super) fn button_row(ui: &mut Ui, buttons: &[(Option<Icon>, &str, ButtonKind, bool)]) -> Option<usize> {
    let gap = 6.0;
    let n = buttons.len().max(1) as f32;
    let w = ((ui.available_width() - gap * (n - 1.0)) / n).floor();
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (i, (icon, label, kind, enabled)) in buttons.iter().enumerate() {
            if button_sized(ui, *icon, label, *kind, *enabled, Some(w)).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Compact rounded toggle switch (no On/Off caption).
pub(super) fn toggle(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let (rect, mut r) = ui.allocate_exact_size(vec2(30.0, 16.0), Sense::click());
    if r.clicked() {
        *on = !*on;
        r.mark_changed();
    }
    let t = ui.ctx().animate_bool(r.id, *on);
    let track = theme::LINE.lerp_to_gamma(theme::ACCENT_DEEP, t);
    let p = ui.painter();
    p.rect_filled(rect, 8.0, if r.hovered() { track.gamma_multiply(1.25) } else { track });
    let knob = pos2(egui::lerp(rect.left() + 8.0..=rect.right() - 8.0, t), rect.center().y);
    p.circle_filled(knob, 5.5, if *on { theme::TEXT } else { theme::TEXT_FAINT });
    r
}

// ----------------------------------------------------------------- panes --

pub(super) fn paint_pane(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 2.0, PANE);
}

/// Paints a pane header strip with its caption. Returns the rectangle to the
/// right of the caption, for tabs / tools laid out by the caller.
pub(super) fn pane_header(p: &egui::Painter, rect: Rect, title: &str) -> Rect {
    p.rect_filled(rect, egui::CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, PANE_HEADER);
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, theme::HAIRLINE));
    if title.is_empty() {
        return rect.shrink2(vec2(6.0, 0.0));
    }
    // Magenta tick before the caption, as on the console title bar.
    p.rect_filled(Rect::from_center_size(pos2(rect.left() + PAD + 1.5, rect.center().y), vec2(3.0, 13.0)), 1.0, theme::MAGENTA);
    let galley = p.layout_no_wrap(title.to_owned(), FontId::proportional(11.5), theme::TEXT_DIM);
    let width = galley.size().x;
    p.galley(pos2(rect.left() + PAD + 9.0, rect.center().y - galley.size().y * 0.5), galley, theme::TEXT_DIM);
    Rect::from_min_max(pos2(rect.left() + PAD + 9.0 + width + 12.0, rect.top()), pos2(rect.right() - 4.0, rect.bottom()))
}

/// Child ui confined (and clipped) to `rect`, with the Studio editor style.
pub(super) fn region<R>(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash + std::fmt::Debug, layout: egui::Layout,
    add: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope_builder(egui::UiBuilder::new().id_salt(id).max_rect(rect).layout(layout), |ui| {
        ui.set_clip_rect(rect.intersect(ui.clip_rect()));
        editor_style(ui);
        add(ui)
    })
    .inner
}

/// A 1-pt hairline across the available width.
pub(super) fn divider(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 9.0), Sense::hover());
    ui.painter().line_segment(
        [pos2(rect.left(), rect.center().y), pos2(rect.right(), rect.center().y)],
        Stroke::new(1.0, theme::HAIRLINE),
    );
}

// ------------------------------------------------------------ inspector --

/// Inspector title card: colored type tag, item title and a detail line.
pub(super) fn inspector_title(ui: &mut Ui, color: Color32, kind: &str, title: &str, detail: &str) {
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let (sw, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
        ui.painter().rect_filled(sw, 2.0, color);
        ui.label(egui::RichText::new(kind.to_ascii_uppercase()).size(10.5).color(color).strong());
    });
    ui.add(egui::Label::new(egui::RichText::new(title).size(15.5).color(theme::TEXT)).truncate());
    if !detail.is_empty() {
        ui.add(egui::Label::new(egui::RichText::new(detail).size(11.5).color(theme::TEXT_FAINT)).truncate());
    }
    ui.add_space(4.0);
}

/// Collapsible property group. The header row is full-width and clickable;
/// optional help text lives behind a (?) icon instead of inline paragraphs.
pub(super) fn section<R>(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, title: &str, help: Option<&str>,
    default_open: bool, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let id = ui.make_persistent_id(id);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, default_open);
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    if r.clicked() {
        state.toggle(ui);
    }
    let open = state.is_open();
    let p = ui.painter();
    p.rect_filled(rect, 2.0, if r.hovered() { theme::CONTROL } else { PANE_HEADER });
    paint_icon(p, pos2(rect.left() + 11.0, rect.center().y),
        if open { Icon::ChevronDown } else { Icon::ChevronRight }, theme::TEXT_FAINT);
    p.with_clip_rect(rect.shrink(2.0)).text(pos2(rect.left() + 22.0, rect.center().y), Align2::LEFT_CENTER,
        title.to_ascii_uppercase(), FontId::proportional(11.0),
        if r.hovered() { theme::TEXT } else { theme::TEXT_DIM });
    if let Some(help) = help {
        let icon = Rect::from_center_size(pos2(rect.right() - 13.0, rect.center().y), vec2(18.0, 18.0));
        let hr = ui.interact(icon, id.with("help"), Sense::hover());
        paint_icon(ui.painter(), icon.center(), Icon::Help, if hr.hovered() { theme::ACCENT } else { theme::TEXT_FAINT });
        hr.on_hover_text(help);
    }
    state.store(ui.ctx());
    if !open {
        ui.add_space(2.0);
        return None;
    }
    ui.add_space(4.0);
    let inner = ui.scope(|ui| {
        ui.set_width(ui.available_width());
        add(ui)
    }).inner;
    ui.add_space(8.0);
    Some(inner)
}

fn label_width(ui: &Ui) -> f32 {
    (ui.available_width() * 0.40).clamp(74.0, 140.0)
}

/// Two-column property row: dim label on the left, the editor filling the
/// rest. Sliders and combos inside size themselves to the value column.
pub(super) fn prop_row<R>(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui) -> R) -> R {
    let lw = label_width(ui);
    ui.horizontal(|ui| {
        ui.set_min_height(ROW_H);
        let (rect, r) = ui.allocate_exact_size(vec2(lw, ROW_H), Sense::hover());
        let galley = ui.painter().layout_no_wrap(label.to_owned(), FontId::proportional(12.0), theme::TEXT_DIM);
        let clipped = galley.size().x > rect.width();
        ui.painter().with_clip_rect(rect).galley(
            pos2(rect.left(), rect.center().y - galley.size().y * 0.5), galley, theme::TEXT_DIM);
        if clipped {
            r.on_hover_text(label);
        }
        let vw = ui.available_width();
        ui.spacing_mut().slider_width = (vw - 64.0).max(36.0);
        ui.spacing_mut().combo_width = vw;
        add(ui)
    })
    .inner
}

pub(super) fn prop_value(ui: &mut Ui, label: &str, value: &str) {
    prop_row(ui, label, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(value).size(12.0).color(theme::TEXT)).truncate());
    });
}

pub(super) fn prop_drag(ui: &mut Ui, label: &str, drag: egui::DragValue) -> egui::Response {
    prop_row(ui, label, |ui| ui.add_sized(vec2(ui.available_width(), 20.0), drag))
}

pub(super) fn prop_slider(ui: &mut Ui, label: &str, slider: egui::Slider) -> egui::Response {
    prop_row(ui, label, |ui| ui.add(slider))
}

pub(super) fn prop_toggle(ui: &mut Ui, label: &str, value: &mut bool) -> egui::Response {
    prop_row(ui, label, |ui| toggle(ui, value))
}

pub(super) fn prop_text(ui: &mut Ui, label: &str, value: &mut String, hint: &str) -> egui::Response {
    prop_row(ui, label, |ui| {
        ui.add(egui::TextEdit::singleline(value).hint_text(hint).desired_width(ui.available_width()))
    })
}

/// Full-width combo box from a value/label list. Returns true on change.
pub(super) fn prop_combo<T: PartialEq + Clone>(ui: &mut Ui, label: &str, id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T, options: &[(T, &str)]) -> bool {
    prop_row(ui, label, |ui| combo(ui, id, value, options))
}

pub(super) fn combo<T: PartialEq + Clone>(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug, value: &mut T,
    options: &[(T, &str)]) -> bool {
    let before = value.clone();
    let current = options.iter().find(|(v, _)| v == value).map_or("-", |(_, l)| *l);
    combo_box(id)
        .width(ui.available_width())
        .wrap_mode(egui::TextWrapMode::Truncate)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (v, l) in options {
                ui.selectable_value(value, v.clone(), *l);
            }
        });
    *value != before
}

/// X/Y/Z drag values in equal cells, each underlined in its gizmo axis color.
pub(super) fn prop_vec3(ui: &mut Ui, label: &str, value: &mut [f32; 3], speed: f64,
    axes: [&str; 3], enabled: bool) -> bool {
    prop_row(ui, label, |ui| {
        let gap = 4.0;
        ui.spacing_mut().item_spacing.x = gap;
        let cell = ((ui.available_width() - gap * 2.0) / 3.0).floor();
        let mut changed = false;
        for a in 0..3 {
            let r = ui.add_enabled_ui(enabled, |ui| {
                ui.add_sized(vec2(cell, 20.0), egui::DragValue::new(&mut value[a]).speed(speed)
                    .max_decimals(2).prefix(format!("{} ", axes[a])))
            }).inner;
            ui.painter().line_segment(
                [pos2(r.rect.left() + 2.0, r.rect.bottom() - 1.0), pos2(r.rect.right() - 2.0, r.rect.bottom() - 1.0)],
                Stroke::new(2.0, AXIS[a].gamma_multiply(if enabled { 0.9 } else { 0.35 })));
            changed |= r.changed();
        }
        changed
    })
}

/// Equal-width segmented choice filling the row. Returns true on change.
pub(super) fn segmented<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let h = 24.0;
    let (rect, base) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 3.0, WELL);
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
    let n = options.len().max(1) as f32;
    let cell = rect.width() / n;
    let mut changed = false;
    for (i, (v, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + cell * i as f32, rect.top()), vec2(cell, h)).shrink(2.0);
        let resp = ui.interact(r, base.id.with(i), if ui.is_enabled() { Sense::click() } else { Sense::hover() });
        let selected = v == value;
        let fill = if selected {
            theme::CONTROL_SELECTED
        } else if resp.hovered() && ui.is_enabled() {
            theme::CONTROL
        } else {
            Color32::TRANSPARENT
        };
        p.rect_filled(r, 2.0, fill);
        let color = if !ui.is_enabled() {
            theme::TEXT_DISABLED
        } else if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        p.with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, *label, FontId::proportional(12.0), color);
        if resp.clicked() && !selected {
            *value = v.clone();
            changed = true;
        }
    }
    changed
}

pub(super) fn prop_segmented<T: PartialEq + Clone>(ui: &mut Ui, label: &str, value: &mut T,
    options: &[(T, &str)]) -> bool {
    prop_row(ui, label, |ui| segmented(ui, value, options))
}

/// Faint wrapped note. Reserve for state the user must know right now;
/// background explanations belong in a section's (?) help.
pub(super) fn hint(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(theme::TEXT_FAINT)).wrap());
}

/// Callout with a colored left bar (warnings, blocked states).
pub(super) fn callout(ui: &mut Ui, text: &str, color: Color32) {
    let frame = egui::Frame::new().fill(color.gamma_multiply(0.10)).corner_radius(3)
        .inner_margin(egui::Margin { left: 10, right: 8, top: 6, bottom: 6 });
    let r = frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(egui::RichText::new(text).size(11.5).color(theme::TEXT_DIM)).wrap());
    }).response;
    ui.painter().rect_filled(Rect::from_min_size(r.rect.left_top(), vec2(3.0, r.rect.height())), 1.0, color);
}

// ---------------------------------------------------------------- browser --

/// Underlined tab strip with equal cells (Premiere's panel tabs).
pub(super) fn tab_strip<T: PartialEq + Clone>(ui: &mut Ui, value: &mut T, options: &[(T, &str)]) -> bool {
    let h = 28.0;
    let (rect, base) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter().clone();
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, theme::LINE));
    let cell = rect.width() / options.len().max(1) as f32;
    let mut changed = false;
    for (i, (v, label)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + cell * i as f32, rect.top()), vec2(cell, h));
        let resp = ui.interact(r, base.id.with(i), Sense::click());
        let selected = v == value;
        if resp.hovered() && !selected {
            p.rect_filled(r.shrink2(vec2(1.0, 3.0)), 3.0, theme::CONTROL);
        }
        let color = if selected { theme::TEXT } else if resp.hovered() { theme::TEXT_DIM } else { theme::TEXT_FAINT };
        p.with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, *label, FontId::proportional(12.0), color);
        if selected {
            p.rect_filled(Rect::from_min_max(pos2(r.left() + 6.0, r.bottom() - 2.0), pos2(r.right() - 6.0, r.bottom())),
                1.0, theme::ACCENT);
        }
        if resp.clicked() && !selected {
            *value = v.clone();
            changed = true;
        }
    }
    changed
}

/// Search box with a magnifier and a clear button.
pub(super) fn search_field(ui: &mut Ui, text: &mut String, hint_text: &str) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    // Fill first: the text edit paints over it, the border and icons after.
    ui.painter().rect_filled(rect, 3.0, WELL);
    let inner = Rect::from_min_max(pos2(rect.left() + 26.0, rect.top()),
        pos2(rect.right() - if text.is_empty() { 6.0 } else { 24.0 }, rect.bottom()));
    let r = ui.put(inner, egui::TextEdit::singleline(text).frame(egui::Frame::NONE).hint_text(hint_text)
        .vertical_align(egui::Align::Center).desired_width(inner.width()));
    let focused = r.has_focus();
    let p = ui.painter();
    p.rect_stroke(rect, 3.0, Stroke::new(1.0, if focused { theme::ACCENT_DEEP } else { theme::LINE }), egui::StrokeKind::Inside);
    paint_icon(p, pos2(rect.left() + 13.0, rect.center().y), Icon::Search, theme::TEXT_FAINT);
    if !text.is_empty() {
        let clear = Rect::from_center_size(pos2(rect.right() - 12.0, rect.center().y), vec2(18.0, 18.0));
        let c = ui.interact(clear, r.id.with("clear"), Sense::click());
        paint_icon(ui.painter(), clear.center(), Icon::Close, if c.hovered() { theme::TEXT } else { theme::TEXT_FAINT });
        if c.clicked() {
            text.clear();
        }
    }
    r
}

/// Browser row: type icon, title and right-aligned metadata.
pub(super) fn list_row(ui: &mut Ui, icon: Icon, tint: Color32, title: &str, meta: &str,
    selected: bool, sense: Sense) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), sense);
    let p = ui.painter();
    let fill = if selected {
        theme::CONTROL_SELECTED
    } else if r.hovered() {
        theme::CONTROL
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 3.0, fill);
    paint_icon(p, pos2(rect.left() + 14.0, rect.center().y), icon, tint);
    let meta_w = if meta.is_empty() {
        0.0
    } else {
        let g = p.layout_no_wrap(meta.to_owned(), FontId::proportional(11.0), theme::TEXT_FAINT);
        let w = g.size().x;
        p.galley(pos2(rect.right() - 8.0 - w, rect.center().y - g.size().y * 0.5), g, theme::TEXT_FAINT);
        w + 12.0
    };
    let text_rect = Rect::from_min_max(pos2(rect.left() + 30.0, rect.top()), pos2(rect.right() - 6.0 - meta_w, rect.bottom()));
    p.with_clip_rect(text_rect).text(pos2(text_rect.left(), rect.center().y), Align2::LEFT_CENTER, title,
        FontId::proportional(12.5), if r.hovered() || selected { theme::TEXT } else { theme::TEXT_DIM });
    r
}

/// Small caps group label inside a browser list.
pub(super) fn group_label(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text.to_ascii_uppercase()).size(10.5).color(theme::TEXT_FAINT).strong());
}

// ------------------------------------------------------------- overlays --

/// Readable text over live 3D: a rounded dark pill behind the caption.
pub(super) fn overlay_pill(p: &egui::Painter, anchor: Pos2, align: Align2, text: &str, color: Color32, size: f32) -> Rect {
    let galley = p.layout_no_wrap(text.to_owned(), FontId::proportional(size), color);
    let rect = align.anchor_size(anchor, galley.size() + vec2(16.0, 8.0));
    p.rect_filled(rect, rect.height() * 0.5, Color32::from_black_alpha(178));
    p.galley(rect.min + vec2(8.0, 4.0), galley, color);
    rect
}

/// Paint the shared empty-state chrome into a caller-sized card.
fn empty_state_card(p: &egui::Painter, card: Rect, icon: Icon, title: &str, detail: &str) {
    p.rect_filled(card, 6.0, PANE.gamma_multiply(0.94));
    p.rect_stroke(card, 6.0, Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
    paint_icon(p, card.center_top() + vec2(0.0, 20.0), icon, theme::TEXT_DIM);
    p.text(card.center_top() + vec2(0.0, 44.0), Align2::CENTER_CENTER, title, FontId::proportional(14.0), theme::TEXT);
    let galley = p.layout(detail.to_owned(), FontId::proportional(11.5), theme::TEXT_DIM, card.width() - 28.0);
    p.galley(pos2(card.center().x - galley.size().x * 0.5, card.top() + 58.0), galley, theme::TEXT_DIM);
}

/// Centered two-line empty state.
pub(super) fn empty_state(p: &egui::Painter, rect: Rect, icon: Icon, title: &str, detail: &str) {
    let card = Rect::from_center_size(rect.center(), vec2(rect.width().min(440.0) - 24.0, 96.0));
    empty_state_card(p, card, icon, title, detail);
}

/// Demo loading state: the progress bar belongs *inside* the card, below the
/// detail text. Account for wrapped text so a narrow viewer also fits it.
pub(super) fn loading_state(ui: &mut Ui, viewport: Rect, icon: Icon, title: &str, detail: &str,
    fraction: f32, progress_text: &str, animate: bool) {
    let painter = ui.painter().with_clip_rect(viewport);
    let card_width = (viewport.width().min(440.0) - 24.0).max(80.0);
    let detail_height = painter.layout(detail.to_owned(), FontId::proportional(11.5),
        theme::TEXT_DIM, (card_width - 28.0).max(1.0)).size().y;
    let bar_top = 58.0 + detail_height + 12.0;
    let card_height = (bar_top + 22.0 + 14.0).max(132.0);
    let card = Rect::from_center_size(viewport.center(), vec2(card_width, card_height));
    empty_state_card(&painter, card, icon, title, detail);
    let bar = Rect::from_min_size(pos2(card.left() + 16.0, card.top() + bar_top),
        vec2((card.width() - 32.0).max(1.0), 22.0));
    ui.put(bar, egui::ProgressBar::new(fraction).animate(animate).text(progress_text));
}

// ---------------------------------------------------------------- format --

/// SMPTE-style HH:MM:SS:FF at the project frame rate.
pub(super) fn timecode(seconds: f64, fps: u32) -> String {
    let fps = fps.max(1) as u64;
    let frames = (seconds.max(0.0) * fps as f64 + 1e-6).floor() as u64;
    let (f, s) = (frames % fps, frames / fps);
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60, f)
}

#[cfg(test)]
mod tests {
    #[test]
    fn background_picker_keeps_panel_open_for_choices_and_map() {
        use egui::{Context,Event,Modifiers,PointerButton,RawInput,Rect,pos2,vec2};
        use crate::app::render_studio::StudioBackground;
        struct State {open:bool,background:StudioBackground,map:u8,button:Rect,combo:Rect,
            choices:[Rect;5],items:[Rect;2],frame:u64,jitter:bool}
        fn frame(ctx:&Context,state:&mut State,events:Vec<Event>) {
            state.frame+=1;
            let _=ctx.run_ui(RawInput{screen_rect:Some(Rect::from_min_size(pos2(0.0,0.0),vec2(640.0,480.0))),events,..Default::default()},|ui|{
                egui::CentralPanel::default().show(ui,|ui|{
                    // Exercise the custom toolbar and changing widget counts,
                    // which the original plain-button regression did not cover.
                    if state.jitter {for _ in 0..state.frame%2 {ui.allocate_space(vec2(1.0,1.0));}}
                    super::region(ui,Rect::from_min_max(pos2(300.0,30.0),pos2(630.0,60.0)),
                        "studio_viewer_profile",egui::Layout::left_to_right(egui::Align::Center),|ui|{
                        let button=super::settings_button(ui,"studio_background_button","Background",true);
                        state.button=button.rect;
                        let mut open=state.open;
                        super::settings_popup(&button,&mut open,|ui|{
                            super::editor_style(ui);ui.set_width(280.0);
                            state.choices=super::super::render_studio::studio_background_choices(ui,&mut state.background).map(|r|r.rect);
                            if state.background==StudioBackground::Levelshot || state.background==StudioBackground::Bsp {
                                super::prop_row(ui,"Map",|ui|{
                                    let combo=super::combo_box("studio_background_map_inspector").width(ui.available_width())
                                        .selected_text(if state.map==0 {"mp/ffa3"}else{"mp/ffa4"})
                                        .show_ui(ui,|ui|{
                                            state.items[0]=ui.selectable_value(&mut state.map,0,"mp/ffa3").rect;
                                            state.items[1]=ui.selectable_value(&mut state.map,1,"mp/ffa4").rect;
                                        });
                                    state.combo=combo.response.rect;
                                });
                            }
                            super::hint(ui,"Demo clips show their recorded world. This background applies to character footage.");
                        });
                        state.open=open;
                    });
                    let viewport=Rect::from_min_max(pos2(20.0,64.0),pos2(630.0,400.0));
                    let response=ui.interact(viewport,ui.make_persistent_id("studio_stage_navigation"),egui::Sense::click_and_drag());
                    if response.contains_pointer() && ui.ctx().input(|i|i.pointer.any_pressed()){response.request_focus();}
                });
            });
        }
        fn click(ctx:&Context,state:&mut State,pos:egui::Pos2) {
            frame(ctx,state,vec![Event::PointerMoved(pos)]);
            for pressed in [true,false] {frame(ctx,state,vec![Event::PointerButton{pos,button:PointerButton::Primary,pressed,modifiers:Modifiers::NONE}]);}
            frame(ctx,state,vec![]);
        }
        for jitter in [false,true] {
            let ctx=Context::default();
            let mut state=State{open:false,background:StudioBackground::Black,map:0,button:Rect::NOTHING,
                combo:Rect::NOTHING,choices:[Rect::NOTHING;5],items:[Rect::NOTHING;2],frame:0,jitter};
            frame(&ctx,&mut state,vec![]);
            let center=state.button.center();click(&ctx,&mut state,center);
            assert!(state.open,"jitter={jitter}: open Background");
            let center=state.choices[2].center();click(&ctx,&mut state,center);
            assert!(state.background==StudioBackground::Blue,"jitter={jitter}: choose Blue screen");
            assert!(state.open,"jitter={jitter}: choosing Blue must keep Background open");
            let center=state.choices[1].center();
            frame(&ctx,&mut state,vec![Event::PointerMoved(center),
                Event::PointerButton{pos:center,button:PointerButton::Primary,pressed:true,modifiers:Modifiers::NONE},
                Event::PointerButton{pos:center,button:PointerButton::Primary,pressed:false,modifiers:Modifiers::NONE}]);
            frame(&ctx,&mut state,vec![]);
            assert!(state.background==StudioBackground::Green,"jitter={jitter}: fast click must choose Green");
            assert!(state.open);
            let center=state.choices[3].center();click(&ctx,&mut state,center);
            let center=state.combo.center();click(&ctx,&mut state,center);
            assert!(state.open && egui::Popup::is_any_open(&ctx),"jitter={jitter}: Map dropdown must stay open");
            let center=state.items[1].center();click(&ctx,&mut state,center);
            assert_eq!(state.map,1);assert!(state.open);
            click(&ctx,&mut state,pos2(100.0,440.0));
            assert!(!state.open,"jitter={jitter}: outside click must dismiss settings");
        }
    }
    #[test]
    fn timecode_counts_frames_at_project_rate() {
        assert_eq!(super::timecode(0.0, 30), "00:00:00:00");
        assert_eq!(super::timecode(1.5, 30), "00:00:01:15");
        assert_eq!(super::timecode(3725.0 + 23.0 / 24.0, 24), "01:02:05:23");
    }
}
