//! Setup -> Interface: the local model rows of TRANSLATION, and CHAT FILTER
//! (`cg_chatCensor*`, `cg_chatLlm*`, the prompts).
use crate::app::egui_settings::{theme, App};
use crate::chat_llm::{self as cl, engine, CensorAction, ModelState};
use crate::app::chat_llm::ChatModelUse;

/// Which prompt a text box edits.
#[derive(Clone, Copy)]
enum Prompt {
    Translate,
    Censor,
    Summary,
}

impl Prompt {
    fn cvar(self) -> &'static str {
        match self {
            Self::Translate => "cg_chatTranslatePrompt",
            Self::Censor => "cg_chatCensorPrompt",
            Self::Summary => "cg_quickRundownPrompt",
        }
    }

    fn default(self) -> &'static str {
        match self {
            Self::Translate => cl::DEFAULT_TRANSLATE_PROMPT,
            Self::Censor => cl::DEFAULT_CENSOR_PROMPT,
            Self::Summary => cl::DEFAULT_SUMMARY_PROMPT,
        }
    }
}

impl App {
    /// Address field, state and buttons for one model. `translate` picks
    /// `cg_chatTranslateModel`, otherwise `cg_chatLlmModel`.
    fn egui_chat_model_rows(&mut self, ui: &mut egui::Ui, translate: bool) {
        let settings = &self.chat_llm.settings;
        let (cvar, saved, active, default, default_name) = if translate {
            (
                "cg_chatTranslateModel",
                settings.translate_url.clone(),
                settings.translate_model().to_owned(),
                cl::DEFAULT_TRANSLATE_URL,
                "Hy-MT2 1.8B, 1.1 GB",
            )
        } else {
            (
                "cg_chatLlmModel",
                settings.assist_url.clone(),
                settings.assist_model().to_owned(),
                cl::DEFAULT_ASSIST_URL,
                "Qwen3.5 4B, 2.7 GB",
            )
        };
        ui.push_id(if translate { "translation_model" } else { "assist_model" }, |ui| {
        theme::row(
            ui,
            "Model address",
            &format!("A link to a .gguf file. Empty uses {default_name}. Apply only changes the selection and stops features using the previous model; it does not download anything."),
            theme::Reset::None,
            |ui| {
                let edits = &mut self.chat_llm.edits;
                let input = if translate { &mut edits.translate_url } else { &mut edits.assist_url };
                let input = input.get_or_insert_with(|| saved.clone());
                let width = (ui.available_width() - 90.0).clamp(80.0, 330.0);
                let response = ui.add(egui::TextEdit::singleline(input).hint_text(default).desired_width(width));
                let edited = input.trim() != saved;
                let enter = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if (ui.add_enabled(edited, egui::Button::new("APPLY")).clicked() || enter) && edited {
                    let value = input.trim().to_owned();
                    self.chat_llm.edits.note = self.set_console_cvar(cvar, &value).err();
                }
            },
        );
        let host = engine();
        let state = host.state(&active);
        let cached = host.cached(&active);
        // Use a growing panel: status text and actions must not compete for
        // the fixed-height settings row's narrow control column.
        egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                let label = match &state {
                    ModelState::Idle if cached => "Downloaded · unloaded".to_owned(),
                    ModelState::Idle => "Not downloaded · stopped".to_owned(),
                    ModelState::Ready => "Loaded in system RAM · CPU".to_owned(),
                    other => other.describe(),
                };
                let color = match &state {
                    ModelState::Ready => egui::Color32::from_rgb(94, 192, 128),
                    ModelState::Loading | ModelState::Downloading { .. } => theme::WARNING(),
                    ModelState::Failed(_) => theme::DANGER(),
                    ModelState::Idle => theme::TEXT_DIM(),
                };
                let (icon, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(icon.center(), 3.5, color);
                ui.label(egui::RichText::new(label).color(color));
            });
            if let ModelState::Downloading { done, total } = state {
                let fraction = if total > 0 { (done as f64 / total as f64).clamp(0.0, 1.0) as f32 } else { 0.0 };
                ui.add(egui::ProgressBar::new(fraction).show_percentage());
            }
            ui.horizontal_wrapped(|ui| {
                if self.chat_model_in_use(&active)
                    || matches!(state, ModelState::Ready | ModelState::Loading | ModelState::Downloading { .. })
                {
                    let stop = match state {
                        ModelState::Downloading { .. } => "CANCEL DOWNLOAD & TURN OFF",
                        ModelState::Loading => "CANCEL LOAD & TURN OFF",
                        _ => "STOP & UNLOAD",
                    };
                    if ui.button(stop).clicked() {
                        self.unload_chat_model(Some(&active));
                    }
                }
                if cached && ui.button("DELETE DOWNLOADED FILE...").clicked() {
                    self.chat_llm.edits.delete_model = Some(active.clone());
                }
            });
            ui.label(egui::RichText::new(if translate {
                "Enable translation to use this model. Stop turns local translation off and keeps the downloaded file."
            } else {
                "Enable Filter chat or use quickRundown to use this model. Stop turns filtering off, cancels summaries and keeps the downloaded file."
            }).small().color(theme::TEXT_DIM()));
            if matches!(state, ModelState::Downloading { .. }) {
                ui.label(egui::RichText::new("Cancel keeps the partial download. Enable the feature again to confirm and resume.").small());
            }
            if let Some(pending) = self.chat_llm.edits.download.as_ref().filter(|pending| {
                pending.url == active && matches!(pending.usage, ChatModelUse::Translation(_)) == translate
            }) {
                let action = match pending.usage {
                    ChatModelUse::Filter => "DOWNLOAD & ENABLE FILTER",
                    ChatModelUse::Translation(_) => "DOWNLOAD & ENABLE TRANSLATION",
                    ChatModelUse::Summary { .. } => "DOWNLOAD & SUMMARIZE",
                };
                let size = if active == cl::DEFAULT_TRANSLATE_URL { "about 1.1 GB" }
                    else if active == cl::DEFAULT_ASSIST_URL { "about 2.7 GB" }
                    else if active == cl::LIGHT_ASSIST_URL { "about 1.3 GB" }
                    else { "size depends on the selected GGUF file" };
                ui.separator();
                ui.label(egui::RichText::new(format!("Download this model? {size}.")).color(theme::WARNING()));
                ui.add(egui::Label::new(egui::RichText::new(&active).small()).wrap());
                ui.label("The file is saved in DinurdoJK/cache/models. A partial file is resumed when the server supports it.");
                ui.horizontal_wrapped(|ui| {
                    if ui.button(action).clicked() {
                        self.confirm_chat_model_download();
                    }
                    if ui.button("CANCEL").clicked() {
                        self.chat_llm.edits.download = None;
                    }
                });
            } else if self.chat_model_in_use(&active) && matches!(state, ModelState::Idle) && !cached {
                ui.label("Download required. Nothing will download until you confirm.");
                if ui.button("REVIEW DOWNLOAD...").clicked() {
                    let usage = if translate { ChatModelUse::Translation(self.chat_translation.settings.mode) } else { ChatModelUse::Filter };
                    self.request_chat_model_download(usage);
                }
            }
            if self.chat_llm.edits.delete_model.as_deref() == Some(active.as_str()) {
                ui.separator();
                ui.label("Delete this model from disk? This also stops features using it. Using it later will need a new download.");
                let mut delete = false;
                ui.horizontal_wrapped(|ui| {
                    delete = ui.button("DELETE FILE & TURN OFF").clicked();
                    if ui.button("KEEP FILE").clicked() {
                        self.chat_llm.edits.delete_model = None;
                    }
                });
                if delete {
                    self.unload_chat_model(Some(&active));
                    self.chat_llm.edits.note = host.delete_cached(&active).err();
                }
            }
            if let Some(note) = &self.chat_llm.edits.note {
                ui.label(egui::RichText::new(note).small().color(theme::DANGER()));
            }
        });
        });
    }

    fn egui_chat_prompt(&mut self, ui: &mut egui::Ui, prompt: Prompt, label: &str, detail: &str) {
        let settings = &self.chat_llm.settings;
        let saved = match prompt {
            Prompt::Translate => &settings.translate_prompt,
            Prompt::Censor => &settings.censor_prompt,
            Prompt::Summary => &settings.summary_prompt,
        };
        let saved_text = if saved.is_empty() { prompt.default().to_owned() } else { cl::unescape_prompt(saved) };
        let custom = !saved.is_empty();
        theme::hint(ui, &format!("{label} ({}). {detail}", prompt.cvar()), theme::TEXT_DIM());
        let edits = &mut self.chat_llm.edits;
        let input = match prompt {
            Prompt::Translate => &mut edits.translate_prompt,
            Prompt::Censor => &mut edits.censor_prompt,
            Prompt::Summary => &mut edits.summary_prompt,
        };
        let input = input.get_or_insert_with(|| saved_text.clone());
        ui.add(
            egui::TextEdit::multiline(input)
                .desired_rows(if matches!(prompt, Prompt::Censor) { 7 } else { 3 })
                .desired_width(f32::INFINITY)
                .font(egui::TextStyle::Small),
        );
        let edited = input.trim() != saved_text.trim();
        let value = input.trim().to_owned();
        let mut apply = None;
        ui.horizontal(|ui| {
            if ui.add_enabled(edited, egui::Button::new("Apply")).clicked() {
                // Storing the default text itself would pin it; keep "".
                apply = Some(if value == prompt.default() { String::new() } else { cl::escape_prompt(&value) });
            }
            if ui.add_enabled(custom || edited, egui::Button::new("Use default")).clicked() {
                apply = Some(String::new());
            }
        });
        if let Some(value) = apply {
            self.chat_llm.edits.note = self.set_console_cvar(prompt.cvar(), &value).err();
            match prompt {
                Prompt::Translate => self.chat_llm.edits.translate_prompt = None,
                Prompt::Censor => self.chat_llm.edits.censor_prompt = None,
                Prompt::Summary => self.chat_llm.edits.summary_prompt = None,
            }
        }
    }

    /// Rows shown under TRANSLATION while the local provider is selected.
    pub(in crate::app) fn egui_chat_local_translation_settings(&mut self, ui: &mut egui::Ui) {
        if !engine().available() {
            theme::banner(ui, "This build has no local model runtime (built without local-llm).", theme::TEXT_DIM());
        }
        self.egui_chat_model_rows(ui, true);
        self.egui_chat_prompt(
            ui,
            Prompt::Translate,
            "Translation prompt",
            "{target}, {source} and {text} are filled in.",
        );
    }

    pub(in crate::app) fn egui_chat_filter_settings(&mut self, ui: &mut egui::Ui) {
        theme::section(
            ui,
            "CHAT FILTER",
            "A model running on this computer masks profanity and hostility in incoming chat, including disguised spellings. It knows who you are, who wrote the message and who killed whom. Filtered lines get a red mark before the name; hold the cursor on one (chat open, or in the console) to read what was written. That text is drawn on this screen only, never on the Spout2 stream output.",
        );
        let settings = self.chat_llm.settings.clone();
        theme::row(
            ui,
            "Filter model",
            "cg_chatLlmModel. Accurate is Qwen3.5 4B: 2.7 GB of memory and about a second per message; it tells banter from abuse and follows the strictness setting. Light is Qwen3.5 2B: 1.3 GB and about half the time; it catches explicit words in any spelling but misjudges banter and barely reacts to strictness. Changing the selection cancels the old download and turns filtering off. It never starts a download. Enable Filter chat afterward to use the new selection.",
            theme::Reset::None,
            |ui| {
                let light = settings.assist_url.trim() == cl::LIGHT_ASSIST_URL;
                let accurate = settings.assist_url.trim().is_empty() || settings.assist_url.trim() == cl::DEFAULT_ASSIST_URL;
                for (label, selected, value) in [("Accurate (4B)", accurate, ""), ("Light (2B)", light, cl::LIGHT_ASSIST_URL)] {
                    if theme::chip(ui, label, selected).clicked() && !selected {
                        self.chat_llm.edits.note = self.set_console_cvar("cg_chatLlmModel", value).err();
                    }
                    ui.add_space(3.0);
                }
                if !light && !accurate {
                    ui.label(egui::RichText::new("custom").color(theme::TEXT_DIM()));
                }
            },
        );
        let settings = self.chat_llm.settings.clone();
        theme::row(
            ui,
            "Filter chat",
            "Enable to use the selected model on the CPU. If its file is missing, you will be asked to confirm the download first. Stop & Unload below also cancels summaries and releases the model.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, settings.censor) {
                    let _ = self.set_console_cvar("cg_chatCensor", if value { "1" } else { "0" });
                }
            },
        );
        self.egui_chat_model_rows(ui, false);
        if !engine().available() {
            theme::banner(ui, "This build has no local model runtime (built without local-llm).", theme::TEXT_DIM());
        }
        theme::row(
            ui,
            "Strictness",
            "cg_chatCensorSeverity. 1 only explicit words. 3 also clear harassment. 5 treats anything that could be a jab at you as hostile, connecting kills and earlier chat. Higher levels hide more harmless banter.",
            theme::Reset::None,
            |ui| {
                let mut value = f32::from(settings.censor_severity);
                let readout = match settings.censor_severity {
                    1 => "1 words only",
                    2 => "2 direct insults",
                    3 => "3 harassment",
                    4 => "4 reads context",
                    _ => "5 protective",
                };
                if theme::slider(ui, &mut value, 1.0..=5.0, readout) {
                    let level = value.round().clamp(1.0, 5.0) as u8;
                    if level != settings.censor_severity {
                        let _ = self.set_console_cvar("cg_chatCensorSeverity", &level.to_string());
                    }
                }
            },
        );
        theme::row(
            ui,
            "Catch ragebait",
            "cg_chatCensorRagebait. Also filter messages meant to provoke rather than to say something: sarcastic praise right after a kill, passive-aggressive politeness, goading someone who is upset. Judged from who killed whom and what was said just before, so the same words between friends stay visible. Works at any strictness; use the Accurate (4B) model.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, settings.censor_ragebait) {
                    let _ = self.set_console_cvar("cg_chatCensorRagebait", if value { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "Filtered messages",
            "cg_chatCensorAction. Mask words shows the message with *** in place of what was objected to. Hide message shows a marked placeholder line instead, so you still see that someone wrote.",
            theme::Reset::None,
            |ui| {
                for (action, label) in [(CensorAction::Cleanse, "Mask words"), (CensorAction::Hide, "Hide message")] {
                    if theme::chip(ui, label, settings.censor_action == action).clicked() && settings.censor_action != action {
                        let _ = self.set_console_cvar("cg_chatCensorAction", &action.index().to_string());
                    }
                    ui.add_space(3.0);
                }
            },
        );
        theme::row(
            ui,
            "Hold until checked",
            "cg_chatCensorHold. A new message shows as ... until the filter has looked at it (about a second), so nothing appears first and is replaced later. If the check fails the text stays masked.",
            theme::Reset::None,
            |ui| {
                if let Some(value) = theme::switch(ui, settings.censor_hold) {
                    let _ = self.set_console_cvar("cg_chatCensorHold", if value { "1" } else { "0" });
                }
            },
        );
        theme::row(
            ui,
            "CPU threads",
            "cg_chatLlmThreads. Threads the chat models may use. Auto takes about a third of the cores.",
            theme::Reset::None,
            |ui| {
                let mut value = f32::from(settings.threads);
                let readout = if settings.threads == 0 {
                    format!("auto ({})", jka_llm::default_threads())
                } else {
                    settings.threads.to_string()
                };
                if theme::slider(ui, &mut value, 0.0..=16.0, &readout) {
                    let threads = value.round().clamp(0.0, 16.0) as u8;
                    if threads != settings.threads {
                        let _ = self.set_console_cvar("cg_chatLlmThreads", &threads.to_string());
                    }
                }
            },
        );
        self.egui_chat_prompt(
            ui,
            Prompt::Censor,
            "Filter prompt",
            "Keep the answer format: SAFE, or TOXIC: followed by the offending words copied from the message. The client masks those words itself. The strictness rule and the match context are added after it.",
        );
        self.egui_chat_prompt(
            ui,
            Prompt::Summary,
            "Summary prompt",
            "Used by the chatQRD [minutes] [truth|cleanse] console command. {player} and {language} are filled in.",
        );
        if let Some(note) = &self.chat_llm.edits.note {
            theme::hint(ui, note, theme::TEXT_DIM());
        }
        theme::hint(ui, &self.chat_llm.status_line(), theme::TEXT_DIM());
    }
}
