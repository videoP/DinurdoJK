//! Local emote favorites, personal categories and saved variants. Starred
//! emotes populate the LIBRARY tier of the built-in `/emote` wheel.
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
    fn default_categories() -> Vec<Category> {
        [
            ("taunt", "Taunt"),
            ("flex", "Flex"),
            ("dance", "Dance"),
            ("misc", "Miscellaneous"),
        ]
        .into_iter()
        .map(|(id, name)| Category {
            id: id.into(),
            name: name.into(),
        })
        .collect()
    }
    pub fn category_for(&self, emote: &crate::emotes::Emote) -> String {
        let assigned = std::iter::once(&emote.id)
            .chain(emote.catalog_aliases.iter())
            .chain(emote.variants.iter().map(|v| &v.id))
            .find_map(|id| self.assignments.get(id))
            .filter(|id| self.categories.iter().any(|c| &c.id == *id));
        if let Some(id) = assigned {
            return id.clone();
        }
        let title = crate::emotes::display_title(emote).to_lowercase();
        let category = if ["taunt", "laugh", "take the l", "facepalm", "boo!"]
            .iter()
            .any(|s| title.contains(s))
        {
            "taunt"
        } else if ["flex", "muscle", "weightlift", "bodybuild"]
            .iter()
            .any(|s| title.contains(s))
        {
            "flex"
        } else if [
            "dance", "floss", "boogie", "bounce", "shuffle", "twist", "groove", "the worm", "bop",
        ]
        .iter()
        .any(|s| title.contains(s))
            || std::iter::once(emote)
                .chain(emote.variants.iter())
                .any(|e| {
                    e.audio
                        .iter()
                        .any(|a| a.lane == crate::emotes::AudioLane::Music)
                })
        {
            "dance"
        } else {
            "misc"
        };
        self.categories
            .iter()
            .find(|c| c.id == category)
            .or_else(|| self.categories.iter().find(|c| c.id == "misc"))
            .or_else(|| self.categories.first())
            .map(|c| c.id.clone())
            .unwrap_or_default()
    }
    fn settings(&self) -> serde_json::Value {
        serde_json::json!({"favorites":self.favorites,"variants":self.variants,
            "categories":self.categories.iter().map(|c|serde_json::json!({"id":c.id,"name":c.name})).collect::<Vec<_>>(),"assignments":self.assignments,"profile_background":self.profile_background})
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

    pub fn load(base: &std::path::Path) -> Self {
        let data = std::fs::read(base.join("emotes/favorites.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .unwrap_or_default();
        Self::from_settings(&data)
    }
}

impl App {
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

    /// The `/emote` wheel's LIBRARY tier. `Err` explains an empty tier.
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

    pub(super) fn play_favorite_emote(&mut self, id: &str) {
        if self.front_end || self.game_session.is_none() {
            self.console_status = "Emotes play in game".to_owned();
            return;
        }
        let Ok(emotes) = self.wheel_favorite_emotes() else { return };
        if let Some(emote) = emotes.into_iter().find(|e| e.id == id) {
            crate::emotes::play_world(emote);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_favorites_and_category_renames_preserve_assignments() {
        let legacy = serde_json::json!({"favorites":["EID_Floss","EID_Floss"],"variants":{"EID_Floss":2},"opacity":0.3});
        let mut wheel = EmoteWheel::from_settings(&legacy);
        assert_eq!(wheel.categories.len(), 4);
        assert_eq!(wheel.favorites, vec!["EID_Floss"]);
        let emote=jka_emotes::manifest::parse_manifest(&serde_json::json!({"emotes":[{"id":"EID_Floss","name":"Floss","gla":"models/test.gla","first_frame":0,"end_frame":30,"fps":30}]})).unwrap().remove(0);
        assert_eq!(wheel.category_for(&emote), "dance");
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
        assert_eq!(wheel.category_for(&emote), "dance");
    }
    #[test]
    fn malformed_categories_and_backgrounds_are_bounded() {
        let data = serde_json::json!({"categories":[{"id":"a","name":"One"},{"id":"a","name":"Two"},{"id":"b","name":" "}],"assignments":{"bad":"missing"},"profile_background":"../../secret"});
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
        assert_eq!(ids, ["EID_Laugh", "EID_Floss"]);
    }
}
