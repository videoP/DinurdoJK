//! Reflections.
use crate::app::egui_settings::{index_of, quality_table_row, theme, ui, App, ReflectionQuality};

impl App {
    pub(in crate::app::egui_settings) fn egui_reflections(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "REFLECTIONS",
            "Environment probes, screen-space reflections and planar views.",
        );
        const REFLECTION_QUALITY: [(ReflectionQuality, &str); 6] = [
            (ReflectionQuality::Off, "Off"),
            (ReflectionQuality::Legacy, "Legacy"),
            (ReflectionQuality::Low, "Low"),
            (ReflectionQuality::Medium, "Medium"),
            (ReflectionQuality::High, "High"),
            (ReflectionQuality::Ultra, "Ultra"),
        ];
        if let Some(target) = quality_table_row(
            ui,
            "Reflection quality",
            "Master reflection policy. Off removes optional tcGen environment stages, but keeps \
             genuinely translucent authored effects (such as custom glass), while stripping \
             opaque environment bases that have a diffuse replacement. Legacy preserves \
             authored environment maps with no enhanced reflections; Low enables probes, \
             Medium adds screen-space reflections, and High adds one dynamically selected planar reflector; \
             Ultra raises SSR quality and allows up to four planar reflectors. Changes apply \
             live, except that raising to High/Ultra or crossing Off needs the map prepared \
             again (Off specializes the material set and planar modes preserve \
             reflection-plane BSP topology); until Apply Video Settings the renderer runs the \
             nearest quality the loaded map supports. Expensive techniques fall back to \
             cheaper ones automatically.",
            theme::Reset::Video(ui::VIDEO_ROW_SSR),
            self.video.reflection_quality,
            &REFLECTION_QUALITY,
            4,
        ) {
            let current = index_of(&REFLECTION_QUALITY, self.video.reflection_quality, 4);
            self.video_selected = ui::VIDEO_ROW_SSR;
            self.change_video_setting(target as i32 - current as i32);
        }
    }
}
