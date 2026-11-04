//! Commands cvars.
use crate::app::{App, ApplyLatchedScope, ConsoleCvarSetResult, VideoSettings};

impl App {
    pub(in crate::app) fn console_cvar_value(&self, name: &str) -> Option<String> {
        super::registry::cvar_registry()
            .get(self, name)
            .map(|value| value.to_string())
            .or_else(|| self.network.user_cvar(name).map(|cvar| cvar.value.clone()))
    }

    pub(in crate::app) fn parse_console_bool(value: &str) -> Option<bool> {
        match value.trim().to_ascii_lowercase().as_str() {
            "1" | "on" | "true" | "yes" => Some(true),
            "0" | "off" | "false" | "no" => Some(false),
            _ => None,
        }
    }

    pub(in crate::app) fn parse_cvar_bool(name: &str, value: &str) -> Result<bool, String> {
        Self::parse_console_bool(value)
            .ok_or_else(|| format!("{name}: expected 0/1, off/on, false/true"))
    }
    pub(in crate::app) fn parse_cvar_finite(name: &str, value: &str) -> Result<f32, String> {
        value
            .trim()
            .parse::<f32>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or_else(|| format!("{name}: expected a finite number"))
    }
    pub(in crate::app) fn normalize_latched_console_value(
        &self,
        name: &str,
        value: &str,
    ) -> Result<String, String> {
        let def = super::registry::cvar_registry()
            .find(name)
            .ok_or_else(|| format!("Cvar {name} does not exist."))?;
        match def.normalize {
            Some(normalize) => normalize(name, value),
            None => Ok(value.trim().to_owned()),
        }
    }

    pub(in crate::app) fn set_console_cvar_from_console(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<ConsoleCvarSetResult, String> {
        let Some(entry) = crate::console::find(name) else {
            self.set_console_cvar(name, value)?;
            return Ok(ConsoleCvarSetResult::Applied);
        };
        if entry.latch == crate::console::LatchScope::None {
            self.set_console_cvar(name, value)?;
            return Ok(ConsoleCvarSetResult::Applied);
        }

        let normalized = self.normalize_latched_console_value(entry.name, value)?;
        let key = entry.name.to_ascii_lowercase();
        let current = self
            .console_cvar_value(entry.name)
            .ok_or_else(|| format!("{}: cvar is not writable here", entry.name))?;

        // OpenJK Cvar_Set2 cancels a pending latch when the requested value is
        // set back to the current live value.
        if normalized.eq_ignore_ascii_case(&current) {
            let removed = self.latched_console_cvars.remove(&key).is_some();
            if removed {
                self.mark_config_dirty();
            }
            return Ok(ConsoleCvarSetResult::Unchanged);
        }
        if self
            .latched_console_cvars
            .get(&key)
            .is_some_and(|pending| pending.eq_ignore_ascii_case(&normalized))
        {
            return Ok(ConsoleCvarSetResult::Unchanged);
        }

        self.latched_console_cvars.insert(key, normalized);
        self.mark_config_dirty();
        Ok(ConsoleCvarSetResult::Latched)
    }

    pub(in crate::app) fn apply_latched_console_cvars(&mut self, scope: ApplyLatchedScope) {
        let ready: Vec<_> = self
            .latched_console_cvars
            .iter()
            .filter_map(|(name, value)| {
                let entry = crate::console::find(name)?;
                let apply = match (scope, entry.latch) {
                    (
                        ApplyLatchedScope::MapLoad,
                        crate::console::LatchScope::MapLoadOrVidRestart,
                    ) => true,
                    (
                        ApplyLatchedScope::VidRestart,
                        crate::console::LatchScope::VidRestart
                        | crate::console::LatchScope::MapLoadOrVidRestart,
                    ) => true,
                    _ => false,
                };
                apply.then(|| (entry.name.to_owned(), value.clone()))
            })
            .collect();

        for (name, value) in ready {
            self.latched_console_cvars
                .remove(&name.to_ascii_lowercase());
            if let Err(error) = self.set_console_cvar(&name, &value) {
                self.push_console_line(format!("^1{error}"));
            }
        }
    }

    pub(in crate::app) fn apply_latched_values_to_settings(&self, settings: &mut VideoSettings) {
        for (name, value) in &self.latched_console_cvars {
            if let Some(set) =
                super::registry::cvar_definition(name).and_then(|def| def.config.apply_latched)
            {
                set(settings, value);
            }
        }
    }
    /// Set a user-created cvar with Quake set/seta/setu/sets flag semantics.
    pub(in crate::app) fn set_user_created_cvar(
        &mut self, name: &str, value: &str, flags: crate::net::UserCvarFlags,
    ) -> Result<(), String> {
        if crate::console::find(name).is_some() {
            return Err(format!("{name} is already a registered command or cvar"));
        }
        let previous = self.network.user_cvar(name);
        let was_archived = previous.is_some_and(|cv| cv.flags.contains(crate::net::UserCvarFlags::ARCHIVE));
        let had_userinfo = previous.is_some_and(|cv| cv.flags.contains(crate::net::UserCvarFlags::USERINFO));
        let changed = self.network.set_user_cvar(name, value, flags)?;
        let Some(updated) = self.network.user_cvar(name) else { unreachable!() };
        let archived = updated.flags.contains(crate::net::UserCvarFlags::ARCHIVE);
        let userinfo = updated.flags.contains(crate::net::UserCvarFlags::USERINFO);
        if changed && (was_archived || archived) { self.mark_config_dirty(); }
        if changed && (had_userinfo || userinfo) { self.request_userinfo_send(); }
        Ok(())
    }

    pub(in crate::app) fn set_console_cvar(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), String> {
        if super::registry::cvar_registry().find(name).is_some() {
            return super::registry::cvar_registry().set(self, name, value);
        }
        if self.network.user_cvar(name).is_some() {
            return self.set_user_created_cvar(name, value, crate::net::UserCvarFlags::NONE);
        }
        Err(format!("Cvar {name} does not exist."))
    }
}
