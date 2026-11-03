use std::{collections::HashMap, fs, path::Path};
use winit::keyboard::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BindKey {
    Keyboard(KeyCode),
    Mouse(u8),
    WheelUp,
    WheelDown,
}

#[derive(Debug, Clone, Copy)]
pub struct ControlAction {
    pub label: &'static str,
    pub command: &'static str,
    /// Heading the Controls page files this action under.
    pub group: &'static str,
}

pub const CONTROL_ACTIONS: &[ControlAction] = &[
    // Keep this ordered exactly like the stock JKA Controls menu categories.
    ControlAction { label: "FORWARD", command: "+forward", group: "Movement" },
    ControlAction { label: "BACKPEDAL", command: "+back", group: "Movement" },
    ControlAction { label: "STEP LEFT", command: "+moveleft", group: "Movement" },
    ControlAction { label: "STEP RIGHT", command: "+moveright", group: "Movement" },
    ControlAction { label: "JUMP", command: "+moveup", group: "Movement" },
    ControlAction { label: "CROUCH", command: "+movedown", group: "Movement" },
    ControlAction { label: "NO-ROLL DUCK", command: "+duck", group: "Movement" },
    ControlAction { label: "LOW JUMP", command: "lowjump", group: "Movement" },
    ControlAction { label: "WALK", command: "+speed", group: "Movement" },

    ControlAction { label: "USE / INTERACT", command: "+use", group: "Interaction" },
    ControlAction { label: "USE SELECTED ITEM", command: "+button2", group: "Interaction" },
    ControlAction { label: "NEXT INVENTORY ITEM", command: "invnext", group: "Interaction" },
    ControlAction { label: "PREVIOUS INVENTORY ITEM", command: "invprev", group: "Interaction" },
    ControlAction { label: "USE AVAILABLE INVENTORY ITEM", command: "invfree", group: "Interaction" },
    ControlAction { label: "USE BACTA", command: "use_bacta", group: "Interaction" },
    ControlAction { label: "USE SEEKER", command: "use_seeker", group: "Interaction" },
    ControlAction { label: "USE FORCE FIELD", command: "use_field", group: "Interaction" },
    ControlAction { label: "USE ELECTROBINOCULARS", command: "use_electrobinoculars", group: "Interaction" },
    ControlAction { label: "USE SENTRY", command: "use_sentry", group: "Interaction" },

    ControlAction { label: "PRIMARY ATTACK", command: "+attack", group: "Weapons" },
    ControlAction { label: "ALT ATTACK", command: "+altattack", group: "Weapons" },
    ControlAction { label: "PREVIOUS WEAPON", command: "weapprev", group: "Weapons" },
    ControlAction { label: "NEXT WEAPON", command: "weapnext", group: "Weapons" },
    ControlAction { label: "LAST WEAPON", command: "weaplast", group: "Weapons" },
    ControlAction { label: "SINGLEFIRE", command: "+singlefire", group: "Weapons" },
    ControlAction { label: "SELECT FIRE MODE", command: "selectfiremode", group: "Weapons" },
    ControlAction { label: "GRAPPLE", command: "+grapple", group: "Weapons" },
    ControlAction { label: "SABER STYLE", command: "saberAttackCycle", group: "Weapons" },
    ControlAction { label: "SABER / MELEE", command: "weapon 1", group: "Weapons" },
    ControlAction { label: "BRYAR PISTOL", command: "weapon 2", group: "Weapons" },
    ControlAction { label: "BLASTER", command: "weapon 3", group: "Weapons" },
    ControlAction { label: "DISRUPTOR", command: "weapon 4", group: "Weapons" },
    ControlAction { label: "BOWCASTER", command: "weapon 5", group: "Weapons" },
    ControlAction { label: "REPEATER", command: "weapon 6", group: "Weapons" },
    ControlAction { label: "DEMP 2", command: "weapon 7", group: "Weapons" },
    ControlAction { label: "FLECHETTE", command: "weapon 8", group: "Weapons" },
    ControlAction { label: "ROCKET LAUNCHER", command: "weapon 9", group: "Weapons" },
    ControlAction { label: "EXPLOSIVES", command: "weapon 10", group: "Weapons" },
    ControlAction { label: "CONCUSSION RIFLE", command: "weapon 13", group: "Weapons" },

    ControlAction { label: "USE SELECTED FORCE", command: "+useforce", group: "Force Powers" },
    ControlAction { label: "PREVIOUS FORCE POWER", command: "forceprev", group: "Force Powers" },
    ControlAction { label: "NEXT FORCE POWER", command: "forcenext", group: "Force Powers" },
    ControlAction { label: "FORCE PUSH", command: "force_throw", group: "Force Powers" },
    ControlAction { label: "FORCE PULL", command: "force_pull", group: "Force Powers" },
    ControlAction { label: "FORCE SPEED", command: "force_speed", group: "Force Powers" },
    ControlAction { label: "FORCE SEEING", command: "force_seeing", group: "Force Powers" },
    ControlAction { label: "MIND TRICK", command: "force_distract", group: "Force Powers" },
    ControlAction { label: "FORCE HEAL", command: "force_heal", group: "Force Powers" },
    ControlAction { label: "FORCE PROTECT", command: "force_protect", group: "Force Powers" },
    ControlAction { label: "FORCE ABSORB", command: "force_absorb", group: "Force Powers" },
    ControlAction { label: "FORCE GRIP", command: "+force_grip", group: "Force Powers" },
    ControlAction { label: "FORCE LIGHTNING", command: "+force_lightning", group: "Force Powers" },
    ControlAction { label: "FORCE RAGE", command: "force_rage", group: "Force Powers" },
    ControlAction { label: "FORCE DRAIN", command: "+force_drain", group: "Force Powers" },
    ControlAction { label: "TEAM HEAL", command: "force_healother", group: "Force Powers" },
    ControlAction { label: "TEAM ENERGIZE", command: "force_forcepowerother", group: "Force Powers" },

    ControlAction { label: "CHAT", command: "messagemode", group: "Other" },
    ControlAction { label: "TEAM CHAT", command: "messagemode2", group: "Other" },
    ControlAction { label: "TARGET CHAT", command: "messagemode3", group: "Other" },
    ControlAction { label: "ATTACKER CHAT", command: "messagemode4", group: "Other" },
    ControlAction { label: "SCORES", command: "+scores", group: "Other" },
    ControlAction { label: "BINOCULAR ZOOM", command: "zoom", group: "Other" },
    ControlAction { label: "ZOOM", command: "+zoom", group: "Other" },
    ControlAction { label: "TAUNT", command: "taunt", group: "Other" },
    ControlAction { label: "DEV WHEEL", command: "dev", group: "Other" },
    ControlAction { label: "EMOTE WHEEL", command: "emote", group: "Other" },
    ControlAction { label: "CHALLENGE DUEL", command: "engage_duel", group: "Other" },
    ControlAction { label: "TOGGLE THIRD PERSON", command: "toggle cg_thirdPerson 0 1", group: "Other" },
    // DinurdoJK-local actions stay available, but do not create extra top-level groups.
    ControlAction { label: "SURFACE TRACE", command: "trace", group: "Other" },
    ControlAction { label: "PUDDLE DEBUG", command: "puddle_debug", group: "Other" },
];

/// Bind overrides that only apply while spectating a live game. Normal `bind`
/// entries apply when no spectator override is assigned.
pub const SPECTATOR_CONTROL_ACTIONS: &[ControlAction] = &[
    ControlAction { label: "NEXT PLAYER", command: "follownext", group: "Spectate" },
    ControlAction { label: "PREVIOUS PLAYER", command: "followprev", group: "Spectate" },
    ControlAction { label: "CHANGE CAMERA TYPE", command: "toggle cg_thirdPerson 0 1 2", group: "Spectate" },
];

/// Demo-only override actions. Without an override, each action inherits an
/// existing normal/spectator binding; demo overrides never modify gameplay binds.
/// Play/Pause inherits a *tap* of Walk so holding Walk still enables the demo UI.
pub const DEMO_CONTROL_ACTIONS: &[ControlAction] = &[
    ControlAction { label: "PLAY / PAUSE (TAP)", command: "demo_pause", group: "Demo" },
    ControlAction { label: "NEXT PLAYER", command: "demo_next", group: "Demo" },
    ControlAction { label: "PREVIOUS PLAYER / FREECAM BOOST", command: "demo_previous", group: "Demo" },
    ControlAction { label: "CHANGE CAMERA TYPE", command: "demo_camera", group: "Demo" },
    ControlAction { label: "ENTER FREECAM", command: "demo_freecam", group: "Demo" },
];

pub fn demo_inherit_command(command: &str) -> &'static str {
    match command {
        "demo_pause" => "+speed",
        "demo_next" => "+attack",
        "demo_previous" => "+altattack",
        "demo_camera" => "toggle cg_thirdPerson 0 1 2",
        "demo_freecam" => "+moveup",
        _ => "",
    }
}

pub fn spectator_inherit_command(command: &str) -> &str {
    match command {
        "follownext" => "+attack",
        "followprev" => "+altattack",
        "toggle cg_thirdPerson 0 1 2" => "saberAttackCycle",
        _ => command,
    }
}

/// Commands whose current DinurdoJK implementation is specifically tied to
/// TaystJK/jaPRO integration. They are intentionally not shown on the Base JKA
/// Controls page; the dynamic JAPRO section exposes them only while a jaPRO server is active.
pub const JAPRO_CONTROL_ACTIONS: &[ControlAction] = &[
    ControlAction { label: "JETPACK", command: "+button12", group: "jaPRO" },
    ControlAction { label: "DASH", command: "+button13", group: "jaPRO" },
    ControlAction { label: "THROW FLAG", command: "throwflag", group: "jaPRO" },
    ControlAction { label: "FULL FORCE CHALLENGE", command: "engage_fullforceduel", group: "jaPRO" },
    ControlAction { label: "GUN CHALLENGE", command: "engage_gunduel", group: "jaPRO" },
    ControlAction { label: "TELEMARK GUI", command: "amtelegui", group: "jaPRO" },
    ControlAction { label: "TELEPORT MARK", command: "amTeleMark", group: "jaPRO" },
    ControlAction { label: "TELEPORT", command: "amTele", group: "jaPRO" },
    ControlAction { label: "FLIPKICK", command: "flipkick", group: "jaPRO" },
    ControlAction { label: "VOICE CHAT / VGS", command: "voicechat", group: "jaPRO" },
];

pub fn japro_selection(index: usize) -> usize {
    CONTROL_ACTIONS.len() + index
}

pub fn spectator_selection(index: usize) -> usize {
    CONTROL_ACTIONS.len() + JAPRO_CONTROL_ACTIONS.len() + index
}

pub fn demo_selection(index: usize) -> usize {
    CONTROL_ACTIONS.len() + JAPRO_CONTROL_ACTIONS.len() + SPECTATOR_CONTROL_ACTIONS.len() + index
}

pub fn is_demo_selection(selection: usize) -> bool {
    let start = demo_selection(0);
    selection >= start && selection < start + DEMO_CONTROL_ACTIONS.len()
}

pub fn control_action(selection: usize) -> Option<&'static ControlAction> {
    if let Some(action) = CONTROL_ACTIONS.get(selection) {
        return Some(action);
    }
    let japro_index = selection.saturating_sub(CONTROL_ACTIONS.len());
    if let Some(action) = JAPRO_CONTROL_ACTIONS.get(japro_index) {
        return Some(action);
    }
    if let Some(action) = SPECTATOR_CONTROL_ACTIONS.get(
        selection.saturating_sub(CONTROL_ACTIONS.len() + JAPRO_CONTROL_ACTIONS.len()),
    ) {
        return Some(action);
    }
    DEMO_CONTROL_ACTIONS.get(selection.saturating_sub(demo_selection(0)))
}

#[cfg(test)]
pub fn is_japro_selection(selection: usize) -> bool {
    selection >= CONTROL_ACTIONS.len()
        && selection < CONTROL_ACTIONS.len() + JAPRO_CONTROL_ACTIONS.len()
}

pub fn is_spectator_selection(selection: usize) -> bool {
    selection >= CONTROL_ACTIONS.len() + JAPRO_CONTROL_ACTIONS.len()
        && selection
            < CONTROL_ACTIONS.len() + JAPRO_CONTROL_ACTIONS.len() + SPECTATOR_CONTROL_ACTIONS.len()
}


#[derive(Debug, Clone)]
pub struct Bindings {
    map: HashMap<BindKey, String>,
    spectator_map: HashMap<BindKey, String>,
    demo_map: HashMap<BindKey, String>,
}

impl Default for Bindings {
    fn default() -> Self {
        let mut bindings = Self { map: HashMap::new(), spectator_map: HashMap::new(), demo_map: HashMap::new() };
        // Mirrors the stock MP mpdefault.cfg, which jaPRO also runs on (it ships no
        // default binds of its own). Commands this client does not implement yet
        // (+strafe, +lookup/+lookdown, +left/+right, centerview, +mlook,
        // scoresUp/Down, automap_toggle) are left
        // out so a keypress never lands on "unknown command". N is DinurdoJK's own
        // noclip key; the stock keys it would otherwise collide with stay stock.
        for (key, command) in [
            // Weapons.
            (BindKey::Keyboard(KeyCode::Digit1), "weapon 1"),
            (BindKey::Keyboard(KeyCode::Digit2), "weapon 2"),
            (BindKey::Keyboard(KeyCode::Digit3), "weapon 3"),
            (BindKey::Keyboard(KeyCode::Digit4), "weapon 4"),
            (BindKey::Keyboard(KeyCode::Digit5), "weapon 5"),
            (BindKey::Keyboard(KeyCode::Digit6), "weapon 6"),
            (BindKey::Keyboard(KeyCode::Digit7), "weapon 7"),
            (BindKey::Keyboard(KeyCode::Digit8), "weapon 8"),
            (BindKey::Keyboard(KeyCode::Digit9), "weapon 13"),
            (BindKey::Keyboard(KeyCode::Digit0), "weapon 9"),
            (BindKey::Keyboard(KeyCode::Minus), "weapon 10"),
            (BindKey::WheelUp, "weapprev"),
            (BindKey::WheelDown, "weapnext"),
            // Movement.
            (BindKey::Keyboard(KeyCode::KeyW), "+forward"),
            (BindKey::Keyboard(KeyCode::KeyS), "+back"),
            (BindKey::Keyboard(KeyCode::KeyA), "+moveleft"),
            (BindKey::Keyboard(KeyCode::KeyD), "+moveright"),
            (BindKey::Keyboard(KeyCode::ArrowUp), "+forward"),
            (BindKey::Keyboard(KeyCode::ArrowDown), "+back"),
            (BindKey::Keyboard(KeyCode::Comma), "+moveleft"),
            (BindKey::Keyboard(KeyCode::Period), "+moveright"),
            (BindKey::Keyboard(KeyCode::Space), "+moveup"),
            (BindKey::Keyboard(KeyCode::KeyC), "+movedown"),
            (BindKey::Keyboard(KeyCode::ShiftLeft), "+speed"),
            (BindKey::Keyboard(KeyCode::Enter), "+use"),
            (BindKey::Keyboard(KeyCode::KeyR), "+use"),
            // Attack.
            (BindKey::Keyboard(KeyCode::ControlLeft), "+attack"),
            (BindKey::Keyboard(KeyCode::AltLeft), "+altattack"),
            (BindKey::Mouse(1), "+attack"),
            (BindKey::Mouse(2), "+altattack"),
            (BindKey::Mouse(3), "saberAttackCycle"),
            (BindKey::Keyboard(KeyCode::KeyL), "saberAttackCycle"),
            // Force powers.
            (BindKey::Keyboard(KeyCode::F1), "force_throw"),
            (BindKey::Keyboard(KeyCode::F2), "force_pull"),
            (BindKey::Keyboard(KeyCode::F3), "force_speed"),
            (BindKey::Keyboard(KeyCode::F4), "force_seeing"),
            (BindKey::Keyboard(KeyCode::F5), "force_heal"),
            (BindKey::Keyboard(KeyCode::F6), "force_protect"),
            (BindKey::Keyboard(KeyCode::F7), "force_absorb"),
            (BindKey::Keyboard(KeyCode::F8), "force_distract"),
            (BindKey::Keyboard(KeyCode::F9), "+force_grip"),
            (BindKey::Keyboard(KeyCode::F10), "+force_lightning"),
            (BindKey::Keyboard(KeyCode::F11), "force_rage"),
            (BindKey::Keyboard(KeyCode::F12), "+force_drain"),
            (BindKey::Keyboard(KeyCode::Backslash), "force_forcepowerother"),
            (BindKey::Keyboard(KeyCode::BracketRight), "force_healother"),
            (BindKey::Keyboard(KeyCode::KeyF), "+useforce"),
            (BindKey::Keyboard(KeyCode::KeyE), "forcenext"),
            (BindKey::Keyboard(KeyCode::KeyQ), "forceprev"),
            // Communication, scoreboard and misc.
            (BindKey::Keyboard(KeyCode::KeyY), "messagemode"),
            (BindKey::Keyboard(KeyCode::KeyT), "messagemode2"),
            (BindKey::Keyboard(KeyCode::KeyU), "messagemode3"),
            (BindKey::Keyboard(KeyCode::KeyV), "voicechat"),
            (BindKey::Keyboard(KeyCode::KeyO), "ui_opensiegemenu ingame_objectives"),
            (BindKey::Keyboard(KeyCode::Tab), "+scores"),
            (BindKey::Keyboard(KeyCode::KeyP), "toggle cg_thirdPerson 0 1"),
            (BindKey::Keyboard(KeyCode::KeyK), "engage_duel"),
            (BindKey::Keyboard(KeyCode::KeyN), "noclip"),
        ] {
            bindings.set(key, command);
        }
        // Sparse override: normal Mouse3 remains saberAttackCycle in gameplay,
        // while spectators use it to cycle their presentation camera.
        bindings.set_spectator(BindKey::Mouse(3), "toggle cg_thirdPerson 0 1 2");
        bindings
    }
}

impl Bindings {
    pub fn load(primary: &Path) -> Self {
        if let Ok(text) = fs::read_to_string(primary) {
            let mut bindings = Self::default();
            if apply_cfg(&mut bindings, &text) {
                return bindings;
            }
        }



        Self::default()
    }

    pub fn get(&self, key: BindKey) -> Option<&str> {
        self.map.get(&key).map(String::as_str)
    }

    pub fn get_spectator(&self, key: BindKey) -> Option<&str> {
        self.spectator_map.get(&key).map(String::as_str)
    }

    /// Spectator binds override normal binds one key at a time. Missing
    /// spectator entries inherit the normal bind instead of becoming unbound.
    /// Right Alt/Ctrl/Shift inherit their corresponding left-side bind until
    /// explicitly bound. This is a lookup fallback, not a copied config bind.
    pub fn get_resolved(&self, key: BindKey, spectating: bool) -> Option<&str> {
        let lookup = |key| {
            if spectating {
                self.get_spectator(key).or_else(|| self.get(key))
            } else {
                self.get(key)
            }
        };
        // An explicitly assigned right-side bind wins in every context.
        // If absent, resolve the left-side bind in the same context.
        lookup(key).or_else(|| left_modifier_for_right(key).and_then(lookup))
    }

    pub fn get_demo(&self, key: BindKey) -> Option<&str> {
        self.demo_map.get(&key).map(String::as_str)
    }

    pub fn demo_has_override(&self, command: &str) -> bool {
        self.demo_map.values().any(|value| binding_contains_command(value, command))
    }

    pub fn set_demo(&mut self, key: BindKey, command: impl Into<String>) {
        let command = command.into();
        if !DEMO_CONTROL_ACTIONS.iter().any(|action| action.command == command) {
            return; // Only the explicit demo verbs are legal demo overrides.
        }
        // A single key triggers at most one demo action.
        self.demo_map.insert(key, command);
    }

    pub fn unbind_demo(&mut self, key: BindKey) -> bool {
        self.demo_map.remove(&key).is_some()
    }

    pub fn clear_demo(&mut self) {
        self.demo_map.clear();
    }

    pub fn unbind_demo_command(&mut self, command: &str) -> usize {
        let before = self.demo_map.len();
        self.demo_map.retain(|_, value| value != command);
        before - self.demo_map.len()
    }

    pub fn demo_keys_for_command(&self, command: &str) -> Vec<BindKey> {
        let mut keys: Vec<_> = self.demo_map.iter()
            .filter_map(|(key, value)| (value == command).then_some(*key))
            .collect();
        keys.sort_by_key(|key| key_name(*key));
        keys
    }

    pub fn display_for_demo_command(&self, command: &str) -> String {
        let keys = self.demo_keys_for_command(command);
        if !keys.is_empty() {
            return keys.into_iter().map(key_name).collect::<Vec<_>>().join(" OR ");
        }
        let inherit = demo_inherit_command(command);
        if command == "demo_camera" && self.spectator_keys_for_command(inherit).is_empty() {
            let normal = spectator_inherit_command(inherit);
            return format!("Inherit Normal ({normal}) · {}", self.display_for_command(normal));
        }
        let keys = if command == "demo_camera" {
            self.spectator_keys_for_command(inherit)
        } else {
            self.keys_for_command(inherit)
        };
        let keys_text = if keys.is_empty() { "UNBOUND".to_owned() }
            else { keys.into_iter().map(key_name).collect::<Vec<_>>().join(" OR ") };
        let source = if command == "demo_camera" { "Spectate" } else { "Normal" };
        format!("Inherit {source} ({inherit}) · {keys_text}")
    }

    pub fn display_for_spectator_inheritance(&self, command: &str) -> String {
        let override_keys = self.spectator_keys_for_command(command);
        if !override_keys.is_empty() {
            return override_keys.into_iter().map(key_name).collect::<Vec<_>>().join(" OR ");
        }
        let normal = spectator_inherit_command(command);
        format!("Inherit Normal ({normal}) · {}", self.display_for_command(normal))
    }

    pub fn set(&mut self, key: BindKey, command: impl Into<String>) {
        let command = command.into();
        if command.is_empty() {
            self.map.remove(&key);
        } else {
            self.map.insert(key, command);
        }
    }

    pub fn unbind(&mut self, key: BindKey) -> bool {
        self.map.remove(&key).is_some()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn set_spectator(&mut self, key: BindKey, command: impl Into<String>) {
        let command = command.into();
        if command.is_empty() {
            self.spectator_map.remove(&key);
        } else {
            self.spectator_map.insert(key, command);
        }
    }

    pub fn unbind_spectator(&mut self, key: BindKey) -> bool {
        self.spectator_map.remove(&key).is_some()
    }

    pub fn clear_spectator(&mut self) {
        self.spectator_map.clear();
    }

    pub fn unbind_command(&mut self, command: &str) -> usize {
        let mut changed = 0;
        for value in self.map.values_mut() {
            if !binding_contains_command(value, command) {
                continue;
            }
            let kept: Vec<_> = split_binding_commands(value)
                .filter(|part| !part.trim().eq_ignore_ascii_case(command.trim()))
                .map(str::to_owned)
                .collect();
            *value = kept.join("; ");
            changed += 1;
        }
        self.map.retain(|_, value| !value.is_empty());
        changed
    }

    pub fn unbind_spectator_command(&mut self, command: &str) -> usize {
        let mut changed = 0;
        for value in self.spectator_map.values_mut() {
            if !binding_contains_command(value, command) {
                continue;
            }
            let kept: Vec<_> = split_binding_commands(value)
                .filter(|part| !part.trim().eq_ignore_ascii_case(command.trim()))
                .map(str::to_owned)
                .collect();
            *value = kept.join("; ");
            changed += 1;
        }
        self.spectator_map.retain(|_, value| !value.is_empty());
        changed
    }

    pub fn keys_for_command(&self, command: &str) -> Vec<BindKey> {
        let mut keys: Vec<_> = self.map.iter()
            .filter_map(|(key, value)| binding_contains_command(value, command).then_some(*key))
            .collect();
        keys.sort_by_key(|key| key_name(*key));
        keys
    }

    pub fn display_for_command(&self, command: &str) -> String {
        let keys = self.keys_for_command(command);
        if keys.is_empty() {
            "UNBOUND".into()
        } else {
            keys.into_iter().map(key_name).collect::<Vec<_>>().join(" OR ")
        }
    }

    pub fn spectator_keys_for_command(&self, command: &str) -> Vec<BindKey> {
        let mut keys: Vec<_> = self.spectator_map.iter()
            .filter_map(|(key, value)| binding_contains_command(value, command).then_some(*key))
            .collect();
        keys.sort_by_key(|key| key_name(*key));
        keys
    }

    pub fn display_for_spectator_command(&self, command: &str) -> String {
        let keys = self.spectator_keys_for_command(command);
        if keys.is_empty() {
            "INHERIT NORMAL".into()
        } else {
            keys.into_iter().map(key_name).collect::<Vec<_>>().join(" OR ")
        }
    }

    pub fn demo_sorted(&self) -> Vec<(BindKey, String)> {
        let mut entries: Vec<_> = self.demo_map.iter()
            .map(|(key, value)| (*key, value.clone())).collect();
        entries.sort_by_key(|(key, _)| key_name(*key));
        entries
    }

    pub fn sorted(&self) -> Vec<(BindKey, String)> {
        let mut entries: Vec<_> = self.map.iter().map(|(key, value)| (*key, value.clone())).collect();
        entries.sort_by_key(|(key, _)| key_name(*key));
        entries
    }

    pub fn spectator_sorted(&self) -> Vec<(BindKey, String)> {
        let mut entries: Vec<_> = self.spectator_map.iter()
            .map(|(key, value)| (*key, value.clone()))
            .collect();
        entries.sort_by_key(|(key, _)| key_name(*key));
        entries
    }

    pub fn write_cfg(&self, out: &mut String) {
        out.push_str("\n// Key bindings. OpenJK-compatible bind syntax.\nunbindall\n");
        for (key, command) in self.sorted() {
            out.push_str("bind ");
            out.push_str(&key_name(key));
            out.push_str(" \"");
            out.push_str(&escape_cfg(&command));
            out.push_str("\"\n");
        }
        out.push_str("\n// Spectator-only bind overrides. Missing keys inherit normal binds.\nunbindspecall\n");
        for (key, command) in self.spectator_sorted() {
            out.push_str("bindspec ");
            out.push_str(&key_name(key));
            out.push_str(" \"");
            out.push_str(&escape_cfg(&command));
            out.push_str("\"\n");
        }
        out.push_str("\n// Demo-only action overrides; missing actions inherit gameplay/spectator binds.\nunbinddemoall\n");
        for (key, command) in self.demo_sorted() {
            out.push_str("binddemo ");
            out.push_str(&key_name(key));
            out.push_str(" \"");
            out.push_str(&escape_cfg(&command));
            out.push_str("\"\n");
        }
    }
}


fn apply_cfg(bindings: &mut Bindings, text: &str) -> bool {
    let mut saw_bindings = false;
    let text = crate::config::strip_cfg_comments(text);
    for raw in text.lines() {
        let words = split_command_words(raw.trim());
        if words.is_empty() {
            continue;
        }
        if words[0].eq_ignore_ascii_case("unbindall") {
            bindings.clear();
            saw_bindings = true;
            continue;
        }
        if words[0].eq_ignore_ascii_case("unbinddemoall") {
            bindings.clear_demo();
            saw_bindings = true;
            continue;
        }
        if words[0].eq_ignore_ascii_case("binddemo") && words.len() >= 3 {
            if let Some(key) = parse_key(&words[1]) {
                bindings.set_demo(key, words[2..].join(" "));
                saw_bindings = true;
            }
            continue;
        }
        if words[0].eq_ignore_ascii_case("unbindspecall") {
            bindings.clear_spectator();
            saw_bindings = true;
            continue;
        }
        if words[0].eq_ignore_ascii_case("bind") && words.len() >= 3 {
            if let Some(key) = parse_key(&words[1]) {
                bindings.set(key, words[2..].join(" "));
                saw_bindings = true;
            }
            continue;
        }
        if words[0].eq_ignore_ascii_case("bindspec") && words.len() >= 3 {
            if let Some(key) = parse_key(&words[1]) {
                bindings.set_spectator(key, words[2..].join(" "));
                saw_bindings = true;
            }
        }
    }
    saw_bindings
}

pub fn binding_contains_command(binding: &str, command: &str) -> bool {
    binding
        .split(';')
        .any(|part| part.trim().eq_ignore_ascii_case(command.trim()))
}

pub fn split_binding_commands(binding: &str) -> impl Iterator<Item = &str> {
    binding.split(';').map(str::trim).filter(|part| !part.is_empty())
}

/// Button commands currently consumed by DinurdoJK's CL_CreateCmd/CG input
/// path. These have real `+foo` / `-foo` state, rather than being fire-and-forget
/// console commands.
pub fn is_supported_button_stem(stem: &str) -> bool {
    matches!(
        stem.to_ascii_lowercase().as_str(),
        "moveup"
            | "movedown"
            | "forward"
            | "back"
            | "moveleft"
            | "moveright"
            | "speed"
            | "attack"
            | "use"
            | "force_grip"
            | "altattack"
            | "useforce"
            | "force_lightning"
            | "force_drain"
            | "button0"
            | "button1"
            | "button2"
            | "button3"
            | "button4"
            | "button5"
            | "button6"
            | "button7"
            | "button8"
            | "button9"
            | "button10"
            | "button11"
            | "button12"
            | "button13"
            | "button14"
            | "button15"
            | "singlefire"
            | "duck"
            | "grapple"
            | "scores"
            | "zoom"
    )
}

/// `movement_keys` is a semantic set represented with the stock default key
/// codes so the local Pmove/demo/HUD paths can share it regardless of rebinding.
pub fn canonical_movement_key(command: &str) -> Option<KeyCode> {
    Some(match command.to_ascii_lowercase().as_str() {
        "+forward" => KeyCode::KeyW,
        "+back" => KeyCode::KeyS,
        "+moveleft" => KeyCode::KeyA,
        "+moveright" => KeyCode::KeyD,
        "+moveup" => KeyCode::Space,
        "+movedown" => KeyCode::ControlLeft,
        "+speed" => KeyCode::ShiftLeft,
        _ => return None,
    })
}

/// EternalJK-style opt-in right-modifier binds: an unbound right-side key
/// behaves like its left counterpart without modifying the saved bindings.
fn left_modifier_for_right(key: BindKey) -> Option<BindKey> {
    Some(BindKey::Keyboard(match key {
        BindKey::Keyboard(KeyCode::AltRight) => KeyCode::AltLeft,
        BindKey::Keyboard(KeyCode::ControlRight) => KeyCode::ControlLeft,
        BindKey::Keyboard(KeyCode::ShiftRight) => KeyCode::ShiftLeft,
        _ => return None,
    }))
}

pub fn bind_key_for_code(code: KeyCode) -> BindKey {
    // Winit provides distinct physical codes for left and right modifiers.
    // Preserve them so each side can have its own gameplay binding.
    BindKey::Keyboard(code)
}

pub fn parse_key(name: &str) -> Option<BindKey> {
    let upper = name.trim().to_ascii_uppercase();
    let keyboard = match upper.as_str() {
        "SPACE" => Some(KeyCode::Space),
        "TAB" => Some(KeyCode::Tab),
        "ENTER" | "RETURN" => Some(KeyCode::Enter),
        "ESCAPE" | "ESC" => Some(KeyCode::Escape),
        "BACKSPACE" => Some(KeyCode::Backspace),
        "UPARROW" | "UP" => Some(KeyCode::ArrowUp),
        "DOWNARROW" | "DOWN" => Some(KeyCode::ArrowDown),
        "LEFTARROW" | "LEFT" => Some(KeyCode::ArrowLeft),
        "RIGHTARROW" | "RIGHT" => Some(KeyCode::ArrowRight),
        "ALT" | "LALT" | "LEFTALT" => Some(KeyCode::AltLeft),
        "RALT" | "RIGHTALT" => Some(KeyCode::AltRight),
        "CTRL" | "CONTROL" | "LCTRL" | "LCONTROL" | "LEFTCTRL" | "LEFTCONTROL" => Some(KeyCode::ControlLeft),
        "RCTRL" | "RCONTROL" | "RIGHTCTRL" | "RIGHTCONTROL" => Some(KeyCode::ControlRight),
        "SHIFT" | "LSHIFT" | "LEFTSHIFT" => Some(KeyCode::ShiftLeft),
        "RSHIFT" | "RIGHTSHIFT" => Some(KeyCode::ShiftRight),
        "INS" | "INSERT" => Some(KeyCode::Insert),
        "DEL" | "DELETE" => Some(KeyCode::Delete),
        "PGUP" | "PAGEUP" => Some(KeyCode::PageUp),
        "PGDN" | "PAGEDOWN" => Some(KeyCode::PageDown),
        "HOME" => Some(KeyCode::Home),
        "END" => Some(KeyCode::End),
        "PAUSE" => Some(KeyCode::Pause),
        "SEMICOLON" | ";" => Some(KeyCode::Semicolon),
        "APOSTROPHE" | "'" => Some(KeyCode::Quote),
        "COMMA" | "," => Some(KeyCode::Comma),
        "PERIOD" | "." => Some(KeyCode::Period),
        "SLASH" | "/" => Some(KeyCode::Slash),
        "BACKSLASH" | "\\" => Some(KeyCode::Backslash),
        "MINUS" | "-" => Some(KeyCode::Minus),
        "EQUALS" | "=" => Some(KeyCode::Equal),
        "LBRACKET" | "[" => Some(KeyCode::BracketLeft),
        "RBRACKET" | "]" => Some(KeyCode::BracketRight),
        "F1" => Some(KeyCode::F1), "F2" => Some(KeyCode::F2), "F3" => Some(KeyCode::F3),
        "F4" => Some(KeyCode::F4), "F5" => Some(KeyCode::F5), "F6" => Some(KeyCode::F6),
        "F7" => Some(KeyCode::F7), "F8" => Some(KeyCode::F8), "F9" => Some(KeyCode::F9),
        "F10" => Some(KeyCode::F10), "F11" => Some(KeyCode::F11), "F12" => Some(KeyCode::F12),
        _ => None,
    };
    if let Some(key) = keyboard { return Some(BindKey::Keyboard(key)); }
    if let Some(number) = upper.strip_prefix("MOUSE").and_then(|v| v.parse::<u8>().ok()) {
        if (1..=5).contains(&number) { return Some(BindKey::Mouse(number)); }
    }
    match upper.as_str() {
        "MWHEELUP" => return Some(BindKey::WheelUp),
        "MWHEELDOWN" => return Some(BindKey::WheelDown),
        _ => {}
    }
    if upper.len() == 1 {
        let byte = upper.as_bytes()[0];
        if byte.is_ascii_alphabetic() {
            let code = match byte {
                b'A' => KeyCode::KeyA, b'B' => KeyCode::KeyB, b'C' => KeyCode::KeyC, b'D' => KeyCode::KeyD,
                b'E' => KeyCode::KeyE, b'F' => KeyCode::KeyF, b'G' => KeyCode::KeyG, b'H' => KeyCode::KeyH,
                b'I' => KeyCode::KeyI, b'J' => KeyCode::KeyJ, b'K' => KeyCode::KeyK, b'L' => KeyCode::KeyL,
                b'M' => KeyCode::KeyM, b'N' => KeyCode::KeyN, b'O' => KeyCode::KeyO, b'P' => KeyCode::KeyP,
                b'Q' => KeyCode::KeyQ, b'R' => KeyCode::KeyR, b'S' => KeyCode::KeyS, b'T' => KeyCode::KeyT,
                b'U' => KeyCode::KeyU, b'V' => KeyCode::KeyV, b'W' => KeyCode::KeyW, b'X' => KeyCode::KeyX,
                b'Y' => KeyCode::KeyY, b'Z' => KeyCode::KeyZ, _ => unreachable!(),
            };
            return Some(BindKey::Keyboard(code));
        }
        if byte.is_ascii_digit() {
            let code = match byte {
                b'0' => KeyCode::Digit0, b'1' => KeyCode::Digit1, b'2' => KeyCode::Digit2, b'3' => KeyCode::Digit3,
                b'4' => KeyCode::Digit4, b'5' => KeyCode::Digit5, b'6' => KeyCode::Digit6, b'7' => KeyCode::Digit7,
                b'8' => KeyCode::Digit8, b'9' => KeyCode::Digit9, _ => unreachable!(),
            };
            return Some(BindKey::Keyboard(code));
        }
    }
    None
}

pub fn key_name(key: BindKey) -> String {
    match key {
        BindKey::Mouse(n) => format!("MOUSE{n}"),
        BindKey::WheelUp => "MWHEELUP".into(),
        BindKey::WheelDown => "MWHEELDOWN".into(),
        BindKey::Keyboard(code) => match code {
            KeyCode::Space => "SPACE".into(), KeyCode::Tab => "TAB".into(), KeyCode::Enter => "ENTER".into(),
            KeyCode::Escape => "ESCAPE".into(), KeyCode::Backspace => "BACKSPACE".into(),
            KeyCode::ArrowUp => "UPARROW".into(), KeyCode::ArrowDown => "DOWNARROW".into(),
            KeyCode::ArrowLeft => "LEFTARROW".into(), KeyCode::ArrowRight => "RIGHTARROW".into(),
            KeyCode::AltLeft => "ALT".into(), KeyCode::AltRight => "RALT".into(),
            KeyCode::ControlLeft => "CTRL".into(), KeyCode::ControlRight => "RCTRL".into(),
            KeyCode::ShiftLeft => "SHIFT".into(), KeyCode::ShiftRight => "RSHIFT".into(),
            KeyCode::Insert => "INS".into(), KeyCode::Delete => "DEL".into(), KeyCode::PageUp => "PGUP".into(),
            KeyCode::PageDown => "PGDN".into(), KeyCode::Home => "HOME".into(), KeyCode::End => "END".into(),
            KeyCode::Pause => "PAUSE".into(), KeyCode::Semicolon => "SEMICOLON".into(), KeyCode::Quote => "APOSTROPHE".into(),
            KeyCode::Comma => "COMMA".into(), KeyCode::Period => "PERIOD".into(), KeyCode::Slash => "SLASH".into(),
            KeyCode::Backslash => "BACKSLASH".into(), KeyCode::Minus => "MINUS".into(), KeyCode::Equal => "EQUALS".into(),
            KeyCode::BracketLeft => "LBRACKET".into(), KeyCode::BracketRight => "RBRACKET".into(),
            KeyCode::KeyA => "A".into(), KeyCode::KeyB => "B".into(), KeyCode::KeyC => "C".into(), KeyCode::KeyD => "D".into(),
            KeyCode::KeyE => "E".into(), KeyCode::KeyF => "F".into(), KeyCode::KeyG => "G".into(), KeyCode::KeyH => "H".into(),
            KeyCode::KeyI => "I".into(), KeyCode::KeyJ => "J".into(), KeyCode::KeyK => "K".into(), KeyCode::KeyL => "L".into(),
            KeyCode::KeyM => "M".into(), KeyCode::KeyN => "N".into(), KeyCode::KeyO => "O".into(), KeyCode::KeyP => "P".into(),
            KeyCode::KeyQ => "Q".into(), KeyCode::KeyR => "R".into(), KeyCode::KeyS => "S".into(), KeyCode::KeyT => "T".into(),
            KeyCode::KeyU => "U".into(), KeyCode::KeyV => "V".into(), KeyCode::KeyW => "W".into(), KeyCode::KeyX => "X".into(),
            KeyCode::KeyY => "Y".into(), KeyCode::KeyZ => "Z".into(),
            KeyCode::Digit0 => "0".into(), KeyCode::Digit1 => "1".into(), KeyCode::Digit2 => "2".into(), KeyCode::Digit3 => "3".into(),
            KeyCode::Digit4 => "4".into(), KeyCode::Digit5 => "5".into(), KeyCode::Digit6 => "6".into(), KeyCode::Digit7 => "7".into(),
            KeyCode::Digit8 => "8".into(), KeyCode::Digit9 => "9".into(),
            KeyCode::F1 => "F1".into(), KeyCode::F2 => "F2".into(), KeyCode::F3 => "F3".into(), KeyCode::F4 => "F4".into(),
            KeyCode::F5 => "F5".into(), KeyCode::F6 => "F6".into(), KeyCode::F7 => "F7".into(), KeyCode::F8 => "F8".into(),
            KeyCode::F9 => "F9".into(), KeyCode::F10 => "F10".into(), KeyCode::F11 => "F11".into(), KeyCode::F12 => "F12".into(),
            _ => format!("{code:?}").to_ascii_uppercase(),
        },
    }
}

pub fn split_command_words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    let mut token_started = false;
    for ch in line.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            token_started = true;
            continue;
        }
        if ch == '\\' && quoted {
            escaped = true;
            token_started = true;
            continue;
        }
        match ch {
            '"' => { quoted = !quoted; token_started = true; }
            c if c.is_whitespace() && !quoted => {
                if token_started { out.push(std::mem::take(&mut current)); token_started = false; }
            }
            _ => { current.push(ch); token_started = true; }
        }
    }
    if escaped { current.push('\\'); }
    if token_started { out.push(current); }
    out
}

fn escape_cfg(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bind_loader_ignores_inline_and_block_comments() {
        let mut bindings = Bindings::default();
        let text = concat!(
            "// Bindings\n",
            "unbindall // reset\n",
            "bind MOUSE1 \"+attack\" // attack\n",
            "/* ignored\n",
            "bind MOUSE1 \"+altattack\" */\n",
            "bind MOUSE2 \"say http://example.org/\"// keep quoted URL\n",
            "unbindspecall // spectator only\n",
            "bindspec MOUSE3 \"toggle cg_thirdPerson 0 1 2\" // camera\n",
        );
        assert!(apply_cfg(&mut bindings, text));
        assert_eq!(bindings.get(BindKey::Mouse(1)), Some("+attack"));
        assert_eq!(bindings.get(BindKey::Mouse(2)), Some("say http://example.org/"));
        assert_eq!(bindings.get_spectator(BindKey::Mouse(3)),
            Some("toggle cg_thirdPerson 0 1 2"));
    }

    #[test]
    fn quoted_bind_keeps_multiword_command() {
        assert_eq!(split_command_words("bind x \"say hello there\""), ["bind", "x", "say hello there"]);
        assert_eq!(split_command_words("bind x \"\""), ["bind", "x", ""]);
    }

    #[test]
    fn default_movement_bindings_exist() {
        let bindings = Bindings::default();
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::KeyW)), Some("+forward"));
        assert_eq!(bindings.get(BindKey::Mouse(1)), Some("+attack"));
    }

    #[test]
    fn default_bindings_follow_mpdefault() {
        let bindings = Bindings::default();
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::KeyC)), Some("+movedown"));
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::Digit1)), Some("weapon 1"));
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::Digit9)), Some("weapon 13"));
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::Digit0)), Some("weapon 9"));
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::F4)), Some("force_seeing"));
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::KeyQ)), Some("forceprev"));
        assert_eq!(bindings.get(BindKey::Keyboard(KeyCode::KeyO)), Some("ui_opensiegemenu ingame_objectives"));
        assert_eq!(bindings.get(BindKey::WheelUp), Some("weapprev"));
    }

    #[test]
    fn left_and_right_modifiers_bind_independently_and_cfg_round_trip() {
        let cases = [
            (KeyCode::AltLeft, "ALT", "LALT", "+altattack"),
            (KeyCode::AltRight, "RALT", "RIGHTALT", "+use"),
            (KeyCode::ControlLeft, "CTRL", "LCTRL", "+attack"),
            (KeyCode::ControlRight, "RCTRL", "RIGHTCTRL", "+force_grip"),
            (KeyCode::ShiftLeft, "SHIFT", "LSHIFT", "+speed"),
            (KeyCode::ShiftRight, "RSHIFT", "RIGHTSHIFT", "+force_drain"),
        ];
        let mut bindings = Bindings::default();
        for (code, name, alias, command) in cases {
            let key = BindKey::Keyboard(code);
            assert_eq!(bind_key_for_code(code), key);
            assert_eq!(parse_key(name), Some(key));
            assert_eq!(parse_key(alias), Some(key));
            assert_eq!(key_name(key), name);
            bindings.set(key, command);
        }
        assert_ne!(bind_key_for_code(KeyCode::AltLeft), bind_key_for_code(KeyCode::AltRight));
        assert_ne!(bind_key_for_code(KeyCode::ControlLeft), bind_key_for_code(KeyCode::ControlRight));
        assert_ne!(bind_key_for_code(KeyCode::ShiftLeft), bind_key_for_code(KeyCode::ShiftRight));

        // A normal config, including old ALT/CTRL/SHIFT binds, remains compatible.
        let mut cfg = String::new();
        bindings.write_cfg(&mut cfg);
        let mut restored = Bindings::default();
        assert!(apply_cfg(&mut restored, &cfg));
        for (code, name, _, command) in cases {
            assert!(cfg.contains(&format!("bind {name} \"{command}\"")));
            assert_eq!(restored.get(BindKey::Keyboard(code)), Some(command));
        }
        // Rebinding one side does not alter the other side.
        restored.unbind(BindKey::Keyboard(KeyCode::ControlRight));
        assert_eq!(restored.get(BindKey::Keyboard(KeyCode::ControlLeft)), Some("+attack"));
    }

    #[test]
    fn right_modifier_fallback_respects_spectator_and_cfg_rules() {
        let left = BindKey::Keyboard(KeyCode::AltLeft);
        let right = BindKey::Keyboard(KeyCode::AltRight);
        let mut bindings = Bindings::default();
        bindings.clear();
        bindings.clear_spectator();
        bindings.set(left, "+attack");
        bindings.set_spectator(left, "followprev");
        assert_eq!(bindings.get_resolved(right, false), Some("+attack"));
        assert_eq!(bindings.get_resolved(right, true), Some("followprev"));

        // Explicit right binds override inherited left binds in both contexts.
        bindings.set(right, "+use");
        assert_eq!(bindings.get_resolved(right, true), Some("+use"));
        bindings.set_spectator(right, "follownext");
        assert_eq!(bindings.get_resolved(right, true), Some("follownext"));
        assert!(bindings.unbind_spectator(right));
        assert_eq!(bindings.get_resolved(right, true), Some("+use"));
        assert!(bindings.unbind(right));
        assert_eq!(bindings.get_resolved(right, true), Some("followprev"));

        // Saving must not synthesize a right bind from the inherited left one.
        let mut cfg = String::new();
        bindings.write_cfg(&mut cfg);
        assert!(cfg.contains("bind ALT \"+attack\""));
        assert!(!cfg.lines().any(|line| line.starts_with("bind RALT ")));
        let mut restored = Bindings::default();
        assert!(apply_cfg(&mut restored, &cfg));
        assert_eq!(restored.get(right), None);
        assert_eq!(restored.get_resolved(right, false), Some("+attack"));
        assert_eq!(restored.get_resolved(right, true), Some("followprev"));
    }

    #[test]
    fn spectator_bindings_override_and_fall_back() {
        let bindings = Bindings::default();
        assert_eq!(bindings.get(BindKey::Mouse(3)), Some("saberAttackCycle"));
        assert_eq!(bindings.get_spectator(BindKey::Mouse(3)), Some("toggle cg_thirdPerson 0 1 2"));
        assert_eq!(bindings.get_resolved(BindKey::Mouse(3), false), Some("saberAttackCycle"));
        assert_eq!(bindings.get_resolved(BindKey::Mouse(3), true), Some("toggle cg_thirdPerson 0 1 2"));
        assert_eq!(bindings.get_resolved(BindKey::Keyboard(KeyCode::KeyY), true), Some("messagemode"));
    }

    #[test]
    fn spectator_cfg_round_trip_syntax_is_distinct() {
        let bindings = Bindings::default();
        let mut text = String::new();
        bindings.write_cfg(&mut text);
        assert!(text.contains("unbindspecall"));
        assert!(text.contains("bindspec MOUSE3 \"toggle cg_thirdPerson 0 1 2\""));
    }

    #[test]
    fn demo_controls_inherit_live_normal_and_spectator_binds() {
        let mut bindings = Bindings::default();
        assert!(bindings.display_for_demo_command("demo_pause").contains("Inherit Normal (+speed)"));
        assert!(bindings.display_for_demo_command("demo_camera").contains("Inherit Spectate (toggle cg_thirdPerson 0 1 2)"));
        assert!(bindings.display_for_spectator_inheritance("follownext").contains("Inherit Normal (+attack)"));
        bindings.set(BindKey::Keyboard(KeyCode::KeyG), "+speed");
        assert!(bindings.display_for_demo_command("demo_pause").contains("G"));
        bindings.set_demo(BindKey::Keyboard(KeyCode::KeyF), "demo_pause");
        assert!(bindings.demo_has_override("demo_pause"));
        assert_eq!(bindings.display_for_demo_command("demo_pause"), "F");
        bindings.unbind_demo_command("demo_pause");
        assert!(!bindings.demo_has_override("demo_pause"));
        assert!(bindings.display_for_demo_command("demo_pause").starts_with("Inherit Normal (+speed)"));
    }

    #[test]
    fn demo_binding_cfg_round_trip_does_not_change_gameplay_binds() {
        let mut bindings = Bindings::default();
        bindings.set_demo(BindKey::Keyboard(KeyCode::KeyF), "demo_pause");
        bindings.set_demo(BindKey::Mouse(4), "demo_previous");
        let mut text = String::new();
        bindings.write_cfg(&mut text);
        assert!(text.contains("unbinddemoall"));
        assert!(text.contains("binddemo F \"demo_pause\""));
        let mut loaded = Bindings::default();
        assert!(apply_cfg(&mut loaded, &text));
        assert_eq!(loaded.get_demo(BindKey::Keyboard(KeyCode::KeyF)), Some("demo_pause"));
        assert_eq!(loaded.get_demo(BindKey::Mouse(4)), Some("demo_previous"));
        assert_eq!(loaded.get(BindKey::Keyboard(KeyCode::KeyF)), Some("+useforce"));
        assert_eq!(loaded.get(BindKey::Mouse(2)), Some("+altattack"));
    }

    #[test]
    fn parameterized_controls_match_exact_binding_command() {
        assert!(binding_contains_command("weapon 1", "weapon 1"));
        assert!(!binding_contains_command("weapon 13", "weapon 1"));
        assert!(binding_contains_command("say hi; toggle cg_thirdPerson", "toggle cg_thirdPerson"));
    }

    #[test]
    fn contextual_controls_have_disjoint_selection_ranges() {
        let japro = japro_selection(0);
        assert!(is_japro_selection(japro));
        assert!(!is_spectator_selection(japro));
        assert_eq!(control_action(japro).map(|action| action.command), Some("+button12"));

        let spectator = spectator_selection(0);
        assert!(is_spectator_selection(spectator));
        assert!(!is_japro_selection(spectator));
        assert_eq!(control_action(spectator).map(|action| action.command), Some("follownext"));

        let demo = demo_selection(0);
        assert!(is_demo_selection(demo));
        assert!(!is_spectator_selection(demo));
        assert_eq!(control_action(demo).map(|action| action.command), Some("demo_pause"));
    }
}
