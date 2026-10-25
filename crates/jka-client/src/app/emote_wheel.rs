//! Local emote favorites, personal categories and saved variants. Starred
//! emotes populate the FAVORITES tier of the built-in `/emote` wheel.
use super::*;

#[derive(Clone, PartialEq, Eq, Debug)]
pub(super) struct Category {
    pub id: String,
    pub name: String,
}

#[derive(Default)]
pub(super) struct EmoteWheel {
    pub favorites: Vec<String>,
    pub variants: std::collections::HashMap<String, usize>,
    pub categories: Vec<Category>,
    pub assignments: std::collections::HashMap<String, String>,
    pub profile_background: Option<String>,
}

impl EmoteWheel {
    // The IDs of the original four categories are retained: a saved manual
    // assignment must never silently move to a different category.
    fn default_categories() -> Vec<Category> {
        [
            ("dance-street", "Street & Hip-Hop"),
            ("dance-pop", "Pop & Choreo"),
            ("dance-party", "Party & Club"),
            ("dance-goofy", "Goofy & Meme"),
            ("dance", "Other Dances"),
            ("music", "Music & Instruments"),
            ("taunt", "Taunts & Trolling"),
            ("celebrate", "Celebrations & Victory"),
            ("social", "Greetings & Social"),
            ("reaction", "Reactions & Moods"),
            ("flex", "Poses & Flex"),
            ("action", "Action & Acrobatics"),
            ("props", "Props & Activities"),
            ("misc", "Other Emotes"),
        ]
        .into_iter()
        .map(|(id, name)| Category { id: id.into(), name: name.into() })
        .collect()
    }

    pub fn category_for(&self, emote: &crate::emotes::Emote) -> String {
        // Explicit profile assignments (including an alias/variant assignment)
        // always take precedence over automatic classification.
        if let Some(assigned) = std::iter::once(&emote.id)
            .chain(emote.catalog_aliases.iter())
            .chain(emote.variants.iter().map(|v| &v.id))
            .find_map(|id| self.assignments.get(id))
            .filter(|id| self.categories.iter().any(|c| &c.id == *id))
        {
            return assigned.clone();
        }
        let title = crate::emotes::display_title(emote).to_ascii_lowercase();
        let id = emote.id.to_ascii_lowercase();
        let matches = |words: &[&str]| words.iter().any(|word| title.contains(*word) || id.contains(*word));
        let music = std::iter::once(emote).chain(emote.variants.iter())
            .any(|e| e.audio.iter().any(|a| a.lane == crate::emotes::AudioLane::Music));
        let prop = std::iter::once(emote).chain(emote.variants.iter())
            .any(|e| !e.props.is_empty());

        // These are intentionally high-confidence display-name/ID heuristics,
        // not invented Fortnite metadata. Unrecognized musical emotes go to
        // Other Dances, which the radial breaks into bounded browse sections.
        let target = if matches(&[
            "take the l", "taunt", "troll", "mock", "boo!", "laugh", "laughter",
            "raspberry", "jeer", "sneer", "loser", "clown", "slow clap",
        ]) {
            "taunt"
        } else if matches(&[
            "victory", "winner", "champion", "celebrat", "applause", "cheer",
            "triumph", "trophy", "goal", "confetti", "congrat", "fireworks",
        ]) {
            "celebrate"
        } else if matches(&[
            "hello", "goodbye", "good bye", "greet", "handshake", "high five", "highfive",
            "hug", "kiss", "salute", "wave hello", "heart sign", "heartsign", "valentine",
        ]) {
            "social"
        } else if matches(&[
            "facepalm", "shrug", "cry", "sad", "angry", "rage", "scared",
            "confused", "surprise", "shock", "thinking", "disappoint", "nervous",
            "yawn", "sigh", "nope", "hmm", "sulk",
        ]) {
            "reaction"
        } else if matches(&[
            "flex", "muscle", "bodybuild", "weightlift", "posing", "pose", "selfie",
            "model stance", "power stance", "hero stance",
        ]) {
            "flex"
        } else if matches(&[
            "flip", "acrobat", "parkour", "cartwheel", "somersault", "backflip",
            "frontflip", "karate", "martial", "kickbox", "boxing", "handstand",
            "pushup", "push-up", "situp", "sit-up", "sprint", "workout",
        ]) {
            "action"
        } else if matches(&[
            "guitar", "piano", "drum", "sax", "trumpet", "trombone", "violin",
            "ukulele", "banjo", "flute", "harp", "instrument", "lute", "keyboard",
            "beatbox", "sing along", "singalong", "concert",
        ]) {
            "music"
        } else if matches(&[
            "breakdance", "break dance", "hip hop", "hiphop", "freestyle", "street dance",
            "shuffle", "poplock", "pop lock", "b-boy", "bboy", "crisscross", "criss cross",
        ]) {
            "dance-street"
        } else if matches(&[
            "k-pop", "kpop", "choreo", "idol", "pop dance", "ballet", "waltz",
            "tango", "ribbon dance", "ribbondance", "jazz hands", "jazzhands",
        ]) {
            "dance-pop"
        } else if matches(&[
            "disco", "rave", "club", "party", "boogie", "groove", "salsa",
            "conga", "line dance", "linedance", "dancefloor", "dance floor",
        ]) {
            "dance-party"
        } else if matches(&[
            "floss", "the worm", "chicken dance", "robot dance", "meme",
            "goofy", "silly", "rickroll", "rick roll", "dabbing", "dab dance",
            "flailing", "banana dance",
        ]) {
            "dance-goofy"
        } else if music || matches(&["dance", "twist", "bop", "bounce"]) {
            "dance"
        } else if prop || matches(&[
            "sit", "sleep", "read", "cook", "fishing", "eat", "drink", "paint",
            "skate", "toy", "phone", "swing", "chair", "campfire",
        ]) {
            "props"
        } else {
            "misc"
        };
        self.categories.iter().find(|c| c.id == target)
            .or_else(|| self.categories.iter().find(|c| c.id == if music { "dance" } else { "misc" }))
            .or_else(|| self.categories.first())
            .map(|c| c.id.clone()).unwrap_or_default()
    }
    fn settings(&self) -> serde_json::Value {
        serde_json::json!({"favorites":self.favorites,"variants":self.variants,
            "categories":self.categories.iter().map(|c|serde_json::json!({"id":c.id,"name":c.name})).collect::<Vec<_>>(),"category_schema":2,"assignments":self.assignments,"profile_background":self.profile_background})
    }
    fn from_settings(data: &serde_json::Value) -> Self {
        let mut categories: Vec<Category> = data["categories"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                Some(Category {
                    id: v["id"].as_str()?.to_owned(),
                    name: v["name"].as_str()?.to_owned(),
                })
            })
            .filter(|c| {
                !c.id.is_empty()
                    && c.id.len() <= 64
                    && c.id.is_ascii()
                    && !c.name.trim().is_empty()
                    && c.name.chars().count() <= 64
            })
            .take(32)
            .collect();
        let mut seen = std::collections::HashSet::new();
        categories.retain(|c| seen.insert(c.id.clone()));
        if categories.is_empty() {
            categories = Self::default_categories();
        } else if data["category_schema"].as_u64().unwrap_or(1) < 2 {
            // One-time migration of existing favorites.json files. Retain
            // stable IDs, renamed legacy categories, and all user additions.
            let mut legacy = categories;
            let mut merged = Vec::new();
            for mut category in Self::default_categories() {
                if let Some(index) = legacy.iter().position(|c| c.id == category.id) {
                    let previous = legacy.remove(index);
                    let default_old_name = match category.id.as_str() {
                        "taunt" => "Taunt", "flex" => "Flex", "dance" => "Dance",
                        "misc" => "Miscellaneous", _ => "",
                    };
                    if previous.name != default_old_name {
                        category.name = previous.name;
                    }
                }
                merged.push(category);
            }
            merged.extend(legacy);
            merged.truncate(32);
            categories = merged;
        }
        let assignments = data["assignments"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, v)| {
                v.as_str()
                    .filter(|c| categories.iter().any(|category| category.id == *c))
                    .map(|c| (id.clone(), c.to_owned()))
            })
            .collect();
        let profile_background = data["profile_background"]
            .as_str()
            .filter(|s| {
                s.len() <= 256 && !s.contains("..") && !s.contains('\\') && !s.starts_with('/')
            })
            .map(str::to_owned);
        let mut favorites = Vec::new();
        for id in data["favorites"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str())
        {
            if !favorites.iter().any(|old| old == id) {
                favorites.push(id.to_owned());
            }
        }
        let variants = data["variants"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, value)| {
                value
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok())
                    .map(|n| (id.clone(), n))
            })
            .collect();
        Self {
            favorites,
            variants,
            categories,
            assignments,
            profile_background,
        }
    }
    /// Starred emotes in wheel order: grouped by personal category (in the
    /// Profile's category order), then in starring order, each resolved to
    /// its saved variant.
    fn wheel_emotes(&self, catalog: &[crate::emotes::Emote]) -> Vec<crate::emotes::Emote> {
        let mut picked: Vec<(usize, crate::emotes::Emote)> = Vec::new();
        for id in &self.favorites {
            let Some(root) = crate::emotes::resolve_catalog_id(catalog, id) else { continue };
            if picked.iter().any(|(_, e)| e.id == root.id) {
                continue;
            }
            let category = self.category_for(&root);
            let rank = self.categories.iter().position(|c| c.id == category).unwrap_or(usize::MAX);
            let variant = self.variants.get(id).copied().unwrap_or_else(|| crate::emotes::default_variant_index(&root));
            let emote = root.variants.get(variant).cloned().unwrap_or(root);
            picked.push((rank, emote));
        }
        picked.sort_by_key(|(rank, _)| *rank);
        picked.into_iter().map(|(_, emote)| emote).collect()
    }

    /// All installed emotes in a personal category, including unstarred ones.
    /// Categorize the catalogue root before choosing its saved variation.
    pub(super) fn catalog_emotes(&self, catalog: &[crate::emotes::Emote], category: Option<&str>) -> Vec<crate::emotes::Emote> {
        catalog.iter().filter(|emote| category.is_none_or(|id| self.category_for(emote) == id))
            .map(|emote| {
                let variant = self.variants.get(&emote.id).copied()
                    .unwrap_or_else(|| crate::emotes::default_variant_index(emote));
                emote.variants.get(variant).unwrap_or(emote).clone()
            }).collect()
    }

    pub fn load(base: &std::path::Path) -> Self {
        let data = std::fs::read(base.join("emotes/favorites.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_default();
        Self::from_settings(&data)
    }
}

impl App {
    pub(in crate::app) fn stop_game_emote(&mut self) {
        if self.live_connected() { self.live_input.queue_emote_stop(); }
        crate::emotes::stop();
    }

    /// Join a referenced player at their timeline phase and broadcast the
    /// reference through protocol 26's existing angle-based taunt transport.
    pub(in crate::app) fn match_crosshair_emote(&mut self) {
        if !self.live_connected() {
            self.push_console_line("^3MATCH:^7 Connect to a game and aim at a dancing player.");
            return;
        }
        // Ordinary crosshair scans are suspended while the wheel is open.
        // Force the same fresh trace that message-mode 3 uses, rather than
        // reusing a stale HUD last-seen client or the last aimed-at entity.
        self.update_crosshair_target_for_chat(Instant::now());
        let target = self.crosshair_seen
            .filter(|seen| seen.on_target && !seen.is_pilot)
            .and_then(|seen| u16::try_from(seen.entity).ok())
            .filter(|&client| client < 32);
        let Some(preview) = target.and_then(crate::emotes::network::match_target_preview) else {
            self.push_console_line("^3MATCH:^7 Aim directly at someone playing a Fortnite emote.");
            return;
        };
        // Send a MATCH reference, not the copied emote ID. Other clients can
        // inherit the source player's server-time origin as well as its variant.
        let Some(reference) = target.and_then(crate::emote_net_codec::match_reference_id) else {
            return;
        };
        if !self.live_input.queue_emote_taunt(reference) {
            self.push_console_line("^3MATCH:^7 Could not send the emote taunt.");
            return;
        }
        crate::emotes::play_world_matched(preview.emote, preview.elapsed_ms);
        self.close_emote_radial();
    }

    /// In-game selections share the same taunt transport; Profile previews stay local.
    pub(in crate::app) fn play_game_emote(&mut self, emote: crate::emotes::Emote) {
        if self.live_connected() {
            if let Some(id) = crate::emotes::network::wire_id(&emote) {
                self.live_input.queue_emote_taunt(id);
            } else {
                self.live_input.queue_generic_command(27);
                self.live_input.queue_emote_stop();
                self.push_console_line(format!("^3Emote {} / {} has no permanent network ID; playing locally with a normal taunt.", emote.id, emote.variant_name));
            }
        } else {
            self.execute_game_command("taunt");
        }
        crate::emotes::play_world(emote);
    }

    pub(super) fn save_emote_favorites(&mut self) {
        if let Ok(catalog) = crate::emotes::catalog(&self.base) {
            crate::emotes::update_favorite_assets(&catalog, &self.emote_wheel.favorites);
        }
        let path = self.base.join("emotes/favorites.json");
        let result = (|| -> Result<(), String> {
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            let data = self.emote_wheel.settings();
            let bytes = serde_json::to_vec_pretty(&data).map_err(|e| e.to_string())?;
            let temporary = path.with_extension("json.tmp");
            std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(temporary, path).map_err(|e| e.to_string())
        })();
        if let Err(error) = result {
            self.console_status = format!("Saving emote favorites: {error}");
        }
    }

    /// The `/emote` wheel's FAVORITES tier. `Err` explains an empty tier.
    pub(super) fn wheel_favorite_emotes(&self) -> Result<Vec<crate::emotes::Emote>, &'static str> {
        let catalog = crate::emotes::catalog(&self.base).map_err(|_| "NO EMOTE LIBRARY")?;
        if catalog.is_empty() {
            return Err("NO EMOTE LIBRARY");
        }
        let emotes = self.emote_wheel.wheel_emotes(&catalog);
        if emotes.is_empty() { Err("STAR EMOTES IN PROFILE") } else { Ok(emotes) }
    }

    /// Opening the tier decodes the starred emotes' audio ahead of a pick.
    pub(super) fn warm_favorite_emote_audio(&mut self) {
        let Ok(emotes) = self.wheel_favorite_emotes() else { return };
        let paths: Vec<_> = emotes
            .iter()
            .flat_map(|e| e.audio.iter())
            .filter(|a| a.audience != crate::emotes::AudioAudience::Preview)
            .filter_map(|a| a.wav.clone())
            .take(8)
            .collect();
        if let Some(sound) = self.game_session.as_mut().and_then(|s| s.sound_presenter.as_mut()) {
            sound.warm_emote_audio(&paths);
        }
    }

    pub(super) fn play_wheel_emote(&mut self, id: &str) {
        if self.front_end || self.game_session.is_none() {
            self.console_status = "Emotes play in game".to_owned();
            return;
        }
        let Ok(catalog) = crate::emotes::catalog(&self.base) else { return };
        let emote = self.emote_wheel.wheel_emotes(&catalog).into_iter()
            .chain(self.emote_wheel.catalog_emotes(&catalog, None))
            .find(|emote| emote.id == id);
        if let Some(emote) = emote {
            self.play_game_emote(emote);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_profile_categories_upgrade_without_losing_manual_classification() {
        let old = serde_json::json!({
            "categories": [
                {"id": "taunt", "name": "My Taunts"},
                {"id": "dance", "name": "Dance"},
                {"id": "category-1", "name": "Custom"}
            ],
            "favorites": ["EID_Floss"],
            "assignments": {"EID_Floss": "category-1"}
        });
        let migrated = EmoteWheel::from_settings(&old);
        assert_eq!(migrated.categories.len(), 15);
        assert_eq!(migrated.categories.iter().find(|c| c.id == "taunt").unwrap().name, "My Taunts");
        assert_eq!(migrated.categories.iter().find(|c| c.id == "dance").unwrap().name, "Other Dances");
        assert_eq!(migrated.assignments["EID_Floss"], "category-1");
        assert_eq!(migrated.favorites, ["EID_Floss"]);
        let roundtrip = EmoteWheel::from_settings(&migrated.settings());
        assert_eq!(roundtrip.categories, migrated.categories);
        assert_eq!(roundtrip.assignments, migrated.assignments);
    }

    #[test]
    fn library_includes_unstarred_emotes_and_honors_categories_and_variants() {
        let mut catalog = jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[
            {"id":"laugh","name":"Laugh","gla":"models/a.gla","first_frame":0,"end_frame":30,"fps":30},
            {"id":"floss","name":"Floss","gla":"models/b.gla","first_frame":0,"end_frame":30,"fps":30}
        ]})).unwrap();
        let mut alternate = catalog[1].clone();
        alternate.id = "floss-alt".into();
        catalog[1].variants = vec![catalog[1].clone(), alternate];
        let wheel = EmoteWheel::from_settings(&serde_json::json!({
            "favorites":["laugh"], "variants":{"floss":1}, "assignments":{"laugh":"flex"}
        }));
        let ids = |emotes: Vec<crate::emotes::Emote>| emotes.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(wheel.wheel_emotes(&catalog)), ["laugh"]);
        assert_eq!(ids(wheel.catalog_emotes(&catalog, Some("dance-goofy"))), ["floss-alt"]);
        assert_eq!(ids(wheel.catalog_emotes(&catalog, Some("flex"))), ["laugh"]);
        assert!(wheel.catalog_emotes(&catalog, Some("dance")).is_empty());
        assert_eq!(ids(wheel.catalog_emotes(&catalog, None)), ["laugh", "floss-alt"]);
    }

    #[test]
    fn legacy_favorites_and_category_renames_preserve_assignments() {
        let legacy = serde_json::json!({"favorites":["EID_Floss","EID_Floss"],"variants":{"EID_Floss":2},"opacity":0.3});
        let mut wheel = EmoteWheel::from_settings(&legacy);
        assert_eq!(wheel.categories.len(), 14);
        assert_eq!(wheel.favorites, vec!["EID_Floss"]);
        let emote=jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[{"id":"EID_Floss","name":"Floss","gla":"models/test.gla","first_frame":0,"end_frame":30,"fps":30}]})).unwrap().remove(0);
        assert_eq!(wheel.category_for(&emote), "dance-goofy");
        wheel.categories.push(Category {
            id: "custom".into(),
            name: "Party".into(),
        });
        wheel.assignments.insert(emote.id.clone(), "custom".into());
        wheel.categories.last_mut().unwrap().name = "My dances".into();
        wheel.profile_background = Some("mp/ffa3".into());
        let loaded = EmoteWheel::from_settings(&wheel.settings());
        assert_eq!(loaded.category_for(&emote), "custom");
        assert_eq!(loaded.categories.last().unwrap().name, "My dances");
        assert_eq!(loaded.variants[&emote.id], 2);
        assert_eq!(loaded.profile_background.as_deref(), Some("mp/ffa3"));
        wheel.categories.pop();
        assert_eq!(wheel.category_for(&emote), "dance-goofy");
    }
    #[test]
    fn malformed_categories_and_backgrounds_are_bounded() {
        let data = serde_json::json!({"categories":[{"id":"a","name":"One"},{"id":"a","name":"Two"},{"id":"b","name":" "}],"assignments":{"bad":"missing"},"profile_background":"../../secret","category_schema":2});
        let wheel = EmoteWheel::from_settings(&data);
        assert_eq!(wheel.categories.len(), 1);
        assert!(wheel.assignments.is_empty());
        assert!(wheel.profile_background.is_none());
    }
    #[test]
    fn wheel_groups_favorites_by_category_and_skips_unknown_ids() {
        let catalog = jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[
            {"id":"EID_Laugh","name":"Laugh It Up","gla":"models/a.gla","first_frame":0,"end_frame":30,"fps":30},
            {"id":"EID_Floss","name":"Floss","gla":"models/b.gla","first_frame":0,"end_frame":30,"fps":30}]})).unwrap();
        let wheel = EmoteWheel::from_settings(&serde_json::json!({"favorites":["EID_Floss","EID_Missing","EID_Laugh"]}));
        let ids: Vec<_> = wheel.wheel_emotes(&catalog).into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["EID_Floss", "EID_Laugh"]);
    }
}
