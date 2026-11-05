//! TaystJK/jaPRO canned Voice Game System (VGS) command table.
//!
//! The menu hierarchy, hotkeys and leaf command tokens below are a direct port
//! of `assets/japro/ui/jamp/ingame_vgs.menu`.  Rendering is intentionally left
//! to DinurdoJK's egui layer; this module is only the source-authored behavior.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Menu {
    #[default]
    Main,
    Global,
    Compliment,
    Respond,
    Taunt,
    Meme,
    Attack,
    Defend,
    Repair,
    Base,
    Command,
    Enemy,
    Flag,
    Need,
    SelfMenu,
    SelfAttack,
    SelfDefend,
    SelfRepair,
    SelfTask,
    SelfUpgrade,
    Target,
    Upgrade,
    Warning,
    VeryQuick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Menu(Menu),
    Command(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub key: char,
    pub label: &'static str,
    pub action: Action,
}

macro_rules! submenu {
    ($key:literal, $label:literal, $menu:ident) => {
        Item { key: $key, label: $label, action: Action::Menu(Menu::$menu) }
    };
}
macro_rules! command {
    ($key:literal, $label:literal, $token:literal) => {
        Item { key: $key, label: $label, action: Action::Command($token) }
    };
}

const MAIN: &[Item] = &[
    submenu!('g', "G: Global", Global),
    submenu!('a', "A: Attack", Attack),
    submenu!('d', "D: Defend", Defend),
    submenu!('r', "R: Repair", Repair),
    submenu!('b', "B: Base", Base),
    submenu!('c', "C: Command", Command),
    submenu!('e', "E: Enemy", Enemy),
    submenu!('f', "F: Flag", Flag),
    submenu!('n', "N: Need", Need),
    submenu!('s', "S: Self", SelfMenu),
    submenu!('t', "T: Target", Target),
    submenu!('u', "U: Upgrade", Upgrade),
    submenu!('w', "W: Warning", Warning),
    submenu!('v', "V: Very Quick", VeryQuick),
];

const GLOBAL: &[Item] = &[
    submenu!('c', "C: Compliment", Compliment),
    submenu!('r', "R: Respond", Respond),
    submenu!('t', "T: Taunt", Taunt),
    submenu!('m', "M: Memes", Meme),
    command!('y', "Y: Yes", "global_yes"),
    command!('n', "N: No", "global_no"),
    command!('h', "H: Hi", "global_hi"),
    command!('b', "B: Bye", "global_bye"),
    command!('o', "O: Oops", "global_ooops"),
    command!('q', "Q: Quiet", "global_quiet"),
    command!('s', "S: Shazbot", "global_shazbot"),
    command!('w', "W: Woohoo", "global_woohoo"),
];

const COMPLIMENT: &[Item] = &[
    command!('a', "A: Awesome", "compliment_awesome"),
    command!('g', "G: Good game", "compliment_goodgame"),
    command!('n', "N: Nice move", "compliment_nicemove"),
    command!('s', "S: Great shot", "compliment_greatshot"),
    command!('y', "Y: You rock", "compliment_yourock"),
];

const RESPOND: &[Item] = &[
    command!('a', "A: Any time", "respond_anytime"),
    command!('d', "D: Don't know", "respond_dontknow"),
    command!('t', "T: Thanks", "respond_thanks"),
    command!('w', "W: Wait", "respond_respondwait"),
];

const TAUNT: &[Item] = &[
    command!('a', "A: Aww...", "taunt_aww"),
    command!('b', "B: Best you can do?", "taunt_obnoxious"),
    command!('g', "G: I am the greatest!", "taunt_brag"),
    command!('t', "T: THAT was graceful!", "taunt_sarcasm"),
    command!('w', "W: When will you learn?", "taunt_learn"),
];

const MEME: &[Item] = &[
    command!('f', "F: Fix it, NOW!", "meme_fixitnow"),
    command!('t', "T: The fuck is that?", "meme_tfisthat"),
    command!('b', "B: Ban him.", "meme_banhim"),
    command!('o', "O: Oh, sorry.", "meme_ohsorry"),
    command!('h', "H: Huh??", "meme_huh"),
    command!('s', "S: Surprise!", "meme_surprise"),
    command!('y', "Y: Ya blew it..", "meme_yablewit"),
];

const ATTACK: &[Item] = &[
    command!('a', "A: Attack", "attack_attack"),
    command!('b', "B: Base", "attack_base"),
    command!('c', "C: Chase", "attack_chase"),
    command!('d', "D: Disrupt", "attack_disrupt"),
    command!('f', "F: Flag", "attack_flag"),
    command!('g', "G: Generator", "attack_generator"),
    command!('r', "R: Reinforce", "attack_reinforce"),
    command!('s', "S: Sensors", "attack_sensors"),
    command!('t', "T: Turrets", "attack_turrets"),
    command!('v', "V: Vehicle", "attack_vehicle"),
    command!('w', "W: Wait", "attack_attackwait"),
];

const DEFEND: &[Item] = &[
    command!('b', "B: Base", "defend_base"),
    command!('c', "C: Flag carrier", "defend_flagcarrier"),
    command!('e', "E: Entrances", "defend_entrances"),
    command!('f', "F: Flag", "defend_flag"),
    command!('g', "G: Generator", "defend_generator"),
    command!('m', "M: Me", "defend_me"),
    command!('r', "R: Reinforce", "defend_reinforce"),
    command!('s', "S: Sensors", "defend_sensors"),
    command!('t', "T: Turrets", "defend_turrets"),
    command!('v', "V: Vehicle", "defend_vehicle"),
];

const REPAIR: &[Item] = &[
    command!('g', "G: Generator", "repair_generator"),
    command!('s', "S: Sensors", "repair_sensors"),
    command!('t', "T: Turrets", "repair_turrets"),
    command!('v', "V: Vehicle", "repair_vehicle"),
];

const BASE: &[Item] = &[
    command!('c', "C: Clear", "base_clear"),
    command!('e', "E: Enemy in base", "base_enemyinbase"),
    command!('r', "R: Retake", "base_retake"),
    command!('s', "S: Secure", "base_secure"),
];

const COMMAND: &[Item] = &[
    command!('a', "A: Acknowledged", "command_acknowledged"),
    command!('c', "C: Completed", "command_completed"),
    command!('d', "D: Declined", "command_declined"),
    command!('w', "W: Assignment?", "command_assignment"),
];

const ENEMY: &[Item] = &[
    command!('d', "D: Disarray", "enemy_disarray"),
    command!('g', "G: Generator", "enemy_generator"),
    command!('s', "S: Sensors", "enemy_sensors"),
    command!('t', "T: Turrets", "enemy_turrets"),
    command!('v', "V: Vehicle", "enemy_vehicle"),
];

const FLAG: &[Item] = &[
    command!('d', "D: Defend", "flag_defend"),
    command!('f', "F: I have the flag", "flag_ihave"),
    command!('g', "G: Give me", "flag_giveme"),
    command!('q', "Q: Self retrieve", "flag_iretrieve"),
    command!('r', "R: Retrieve", "flag_retrieve"),
    command!('s', "S: Flag is secure", "flag_secure"),
    command!('t', "T: Take", "flag_take"),
];

const NEED: &[Item] = &[
    command!('c', "C: Covering fire", "need_cover"),
    command!('d', "D: Driver", "need_driver"),
    command!('e', "E: Escort", "need_escort"),
    command!('h', "H: Hold vehicle", "need_holdvehicle"),
    command!('r', "R: I need a ride", "need_ride"),
    command!('s', "S: Support", "need_support"),
    command!('v', "V: Vehicle ready", "need_vehicleready"),
    command!('w', "W: Where to?", "need_whereto"),
];

const SELF_MENU: &[Item] = &[
    submenu!('a', "A: Attack", SelfAttack),
    submenu!('d', "D: Defend", SelfDefend),
    submenu!('r', "R: Repair", SelfRepair),
    submenu!('t', "T: Task", SelfTask),
    submenu!('u', "U: Upgrade", SelfUpgrade),
];

const SELF_ATTACK: &[Item] = &[
    command!('a', "A: Attack", "selfattack_attack"),
    command!('b', "B: Base", "selfattack_base"),
    command!('f', "F: Flag", "selfattack_flag"),
    command!('g', "G: Generator", "selfattack_generator"),
    command!('s', "S: Sensors", "selfattack_sensors"),
    command!('t', "T: Turrets", "selfattack_turrets"),
    command!('v', "V: Vehicle", "selfattack_vehicle"),
];

const SELF_DEFEND: &[Item] = &[
    command!('b', "B: Base", "selfdefend_base"),
    command!('d', "D: Defend", "selfdefend_defend"),
    command!('f', "F: Flag", "selfdefend_flag"),
    command!('g', "G: Generator", "selfdefend_generator"),
    command!('s', "S: Sensors", "selfdefend_sensors"),
    command!('t', "T: Turrets", "selfdefend_turrets"),
    command!('v', "V: Vehicle", "selfdefend_vehicle"),
];

const SELF_REPAIR: &[Item] = &[
    command!('b', "B: Base", "selfrepair_base"),
    command!('g', "G: Generator", "selfrepair_generator"),
    command!('s', "S: Sensors", "selfrepair_sensors"),
    command!('t', "T: Turrets", "selfrepair_turrets"),
    command!('v', "V: Vehicle", "selfrepair_vehicle"),
];

const SELF_TASK: &[Item] = &[
    command!('c', "C: Cover", "selftask_cover"),
    command!('d', "D: Set up defenses", "selftask_defenses"),
    command!('f', "F: Deploy forcefields", "selftask_forcefields"),
    command!('s', "S: Deploy sensors", "selftask_deploysensors"),
    command!('t', "T: Turrets", "selftask_deployturrets"),
    command!('o', "O: On it", "selftask_onit"),
    command!('v', "V: Vehicle", "selftask_vehicle"),
];

const SELF_UPGRADE: &[Item] = &[
    command!('g', "G: Generator", "upgradeself_generator"),
    command!('s', "S: Sensors", "upgradeself_sensor"),
    command!('t', "T: Turrets", "upgradeself_turret"),
];

const TARGET: &[Item] = &[
    command!('a', "A: Acquired", "target_acquired"),
    command!('b', "B: Base", "target_base"),
    command!('d', "D: Destroyed", "target_destroyed"),
    command!('f', "F: Flag", "target_flag"),
    command!('m', "M: Fire on my target", "target_fireonmy"),
    command!('n', "N: Need", "target_need"),
    command!('s', "S: Sensors", "target_sensors"),
    command!('t', "T: Turrets", "target_turret"),
    command!('v', "V: Vehicle", "target_vehicle"),
    command!('w', "W: Wait", "target_wait"),
];

const UPGRADE: &[Item] = &[
    command!('g', "G: Generator", "upgrade_generator"),
    command!('s', "S: Sensors", "upgrade_sensor"),
    command!('t', "T: Turrets", "upgrade_turret"),
];

const WARNING: &[Item] = &[
    command!('e', "E: Enemies", "warn_enemies"),
    command!('v', "V: Vehicle", "warn_vehicle"),
];

const VERY_QUICK: &[Item] = &[
    command!('y', "Y: Yes", "team_yes"),
    command!('n', "N: No", "team_no"),
    command!('a', "A: Anytime", "team_anytime"),
    command!('b', "B: Base secure?", "team_basesecure"),
    command!('c', "C: Cease fire", "team_ceasefire"),
    command!('d', "D: Don't know", "team_dontknow"),
    command!('h', "H: Help!", "team_help"),
    command!('m', "M: Move", "team_move"),
    command!('s', "S: Sorry", "team_sorry"),
    command!('t', "T: Thanks", "team_thanks"),
    command!('w', "W: Wait", "team_wait"),
];

impl Menu {
    pub fn title(self) -> &'static str {
        match self {
            Self::Main => "VGS",
            Self::Global => "Global",
            Self::Compliment => "Compliment",
            Self::Respond => "Respond",
            Self::Taunt => "Taunt",
            Self::Meme => "Memes",
            Self::Attack => "Attack",
            Self::Defend => "Defend",
            Self::Repair => "Repair",
            Self::Base => "Base",
            Self::Command => "Command",
            Self::Enemy => "Enemy",
            Self::Flag => "Flag",
            Self::Need => "Need",
            Self::SelfMenu => "Self",
            Self::SelfAttack => "Self Attack",
            Self::SelfDefend => "Self Defend",
            Self::SelfRepair => "Self Repair",
            Self::SelfTask => "Self Task",
            Self::SelfUpgrade => "Self Upgrade",
            Self::Target => "Target",
            Self::Upgrade => "Upgrade",
            Self::Warning => "Warning",
            Self::VeryQuick => "Very Quick",
        }
    }

    pub fn items(self) -> &'static [Item] {
        match self {
            Self::Main => MAIN,
            Self::Global => GLOBAL,
            Self::Compliment => COMPLIMENT,
            Self::Respond => RESPOND,
            Self::Taunt => TAUNT,
            Self::Meme => MEME,
            Self::Attack => ATTACK,
            Self::Defend => DEFEND,
            Self::Repair => REPAIR,
            Self::Base => BASE,
            Self::Command => COMMAND,
            Self::Enemy => ENEMY,
            Self::Flag => FLAG,
            Self::Need => NEED,
            Self::SelfMenu => SELF_MENU,
            Self::SelfAttack => SELF_ATTACK,
            Self::SelfDefend => SELF_DEFEND,
            Self::SelfRepair => SELF_REPAIR,
            Self::SelfTask => SELF_TASK,
            Self::SelfUpgrade => SELF_UPGRADE,
            Self::Target => TARGET,
            Self::Upgrade => UPGRADE,
            Self::Warning => WARNING,
            Self::VeryQuick => VERY_QUICK,
        }
    }
}

pub fn action_for_key(menu: Menu, key: char) -> Option<Action> {
    let key = key.to_ascii_lowercase();
    menu.items().iter().find(|item| item.key == key).map(|item| item.action)
}

const ALL_MENUS: &[Menu] = &[
    Menu::Main,
    Menu::Global,
    Menu::Compliment,
    Menu::Respond,
    Menu::Taunt,
    Menu::Meme,
    Menu::Attack,
    Menu::Defend,
    Menu::Repair,
    Menu::Base,
    Menu::Command,
    Menu::Enemy,
    Menu::Flag,
    Menu::Need,
    Menu::SelfMenu,
    Menu::SelfAttack,
    Menu::SelfDefend,
    Menu::SelfRepair,
    Menu::SelfTask,
    Menu::SelfUpgrade,
    Menu::Target,
    Menu::Upgrade,
    Menu::Warning,
    Menu::VeryQuick,
];

/// TaystJK's VGS chat line uses the human-readable description associated with
/// the received canned voice command.  Keep that text tied to the exact
/// source-authored `ingame_vgs.menu` leaf which generated the command instead
/// of maintaining a second, independently invented description table.
pub fn description_for_sound(name: &str) -> Option<&'static str> {
    let file = name
        .rsplit(|ch| ch == '/' || ch == '\\')
        .next()
        .unwrap_or(name);
    let without_marker = file.strip_prefix('*').unwrap_or(file);
    let token = without_marker
        .strip_suffix(".wav")
        .or_else(|| without_marker.strip_suffix(".mp3"))
        .unwrap_or(without_marker);

    for &menu in ALL_MENUS {
        for item in menu.items() {
            if let Action::Command(command) = item.action {
                if command.eq_ignore_ascii_case(token) {
                    return item
                        .label
                        .split_once(": ")
                        .map(|(_, description)| description)
                        .or(Some(item.label));
                }
            }
        }
    }
    None
}

/// Direct port of TaystJK/jaPRO `bg_customVGSSoundNames` (without its NULL
/// terminator). The leading '*' is part of the network custom-sound name.
pub const SOUND_NAMES: &[&str] = &[
    "*attack_attack", "*attack_attackwait", "*attack_base", "*attack_chase",
    "*attack_disrupt", "*attack_flag", "*attack_generator", "*attack_reinforce",
    "*attack_sensors", "*attack_turrets", "*attack_vehicle", "*base_clear",
    "*base_enemyinbase", "*base_retake", "*base_secure", "*command_acknowledged",
    "*command_assignment", "*command_completed", "*command_declined", "*defend_base",
    "*defend_entrances", "*defend_flag", "*defend_flagcarrier", "*defend_generator",
    "*defend_me", "*defend_reinforce", "*defend_sensors", "*defend_turrets",
    "*defend_vehicle", "*enemy_disarray", "*enemy_generator", "*enemy_sensors",
    "*enemy_turrets", "*enemy_vehicle", "*flag_defend", "*flag_giveme", "*flag_ihave",
    "*flag_iretrieve", "*flag_retrieve", "*flag_secure", "*flag_take",
    "*compliment_awesome", "*compliment_goodgame", "*compliment_greatshot",
    "*compliment_nicemove", "*compliment_yourock", "*respond_anytime",
    "*respond_dontknow", "*respond_respondwait", "*respond_thanks", "*taunt_aww",
    "*taunt_brag", "*taunt_learn", "*taunt_obnoxious", "*taunt_sarcasm", "*global_bye",
    "*global_hi", "*global_no", "*global_ooops", "*global_quiet", "*global_shazbot",
    "*global_woohoo", "*global_yes", "*need_cover", "*need_driver", "*need_escort",
    "*need_holdvehicle", "*need_ride", "*need_support", "*need_vehicleready",
    "*need_whereto", "*repair_generator", "*repair_sensors", "*repair_turrets",
    "*repair_vehicle", "*selfattack_attack", "*selfattack_base", "*selfattack_flag",
    "*selfattack_generator", "*selfattack_sensors", "*selfattack_turrets",
    "*selfattack_vehicle", "*selfdefend_base", "*selfdefend_defend", "*selfdefend_flag",
    "*selfdefend_generator", "*selfdefend_sensors", "*selfdefend_turrets",
    "*selfdefend_vehicle", "*selfrepair_base", "*selfrepair_generator",
    "*selfrepair_sensors", "*selfrepair_turrets", "*selfrepair_vehicle", "*selftask_cover",
    "*selftask_defenses", "*selftask_deploysensors", "*selftask_deployturrets",
    "*selftask_forcefields", "*selftask_onit", "*selftask_vehicle",
    "*upgradeself_generator", "*upgradeself_sensor", "*upgradeself_turret",
    "*target_acquired", "*target_base", "*target_destroyed", "*target_fireonmy",
    "*target_flag", "*target_need", "*target_sensors", "*target_turret", "*target_vehicle",
    "*target_wait", "*upgrade_generator", "*upgrade_sensor", "*upgrade_turret",
    "*warn_enemies", "*warn_vehicle", "*team_anytime", "*team_basesecure",
    "*team_ceasefire", "*team_dontknow", "*team_help", "*team_move", "*team_no",
    "*team_sorry", "*team_thanks", "*team_wait", "*team_yes", "*meme_fixitnow",
    "*meme_tfisthat", "*meme_banhim", "*meme_ohsorry", "*meme_huh", "*meme_surprise",
    "*meme_yablewit",
];

pub fn is_vgs_sound(name: &str) -> bool {
    SOUND_NAMES.iter().any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// jaPRO cg_event.c `isGlobalVGS`: these VGS families are shown/heard by every
/// team, not only the speaker's.
pub fn is_global_vgs(name: &str) -> bool {
    const GLOBAL_PREFIXES: [&str; 5] = ["*global_", "*compliment_", "*respond_", "*taunt_", "*meme_"];
    GLOBAL_PREFIXES.iter().any(|prefix| {
        name.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_leaf_uses_an_authoritative_vgs_sound_name() {
        for &menu in ALL_MENUS {
            for item in menu.items() {
                if let Action::Command(token) = item.action {
                    assert!(is_vgs_sound(&format!("*{token}")), "missing VGS sound: {token}");
                }
            }
        }
    }


    #[test]
    fn every_authoritative_vgs_sound_has_source_description() {
        for sound in SOUND_NAMES {
            assert!(
                description_for_sound(sound).is_some(),
                "missing VGS description for {sound}"
            );
        }
        assert_eq!(description_for_sound("*global_yes"), Some("Yes"));
        assert_eq!(description_for_sound("*team_help.wav"), Some("Help!"));
        assert_eq!(description_for_sound("sound/chars/mp_generic_male/misc/meme_huh.mp3"), Some("Huh??"));
    }

    #[test]
    fn source_hotkeys_are_case_insensitive() {
        assert_eq!(action_for_key(Menu::Main, 'G'), Some(Action::Menu(Menu::Global)));
        assert_eq!(action_for_key(Menu::Global, 'Y'), Some(Action::Command("global_yes")));
    }
}
