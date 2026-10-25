//! Main-menu content and layout shared by the app (input, hit testing) and the
//! renderer (drawing). The renderer owns every pixel of the main page; the app
//! only lays it out with Taffy, routes input and publishes a small `MenuFrame`
//! whenever something changes. Nothing here depends on egui or wgpu.
pub(crate) mod font;
pub(crate) mod layout;

pub(crate) const ITEM_COUNT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Play,
    Profile,
    Controls,
    Settings,
    ChatLogs,
    Screenshots,
    DeveloperTools,
    Quit,
}

/// Visual weight of an entry. Groups also decide the neon gas colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    Hero,
    Primary,
    Utility,
    Exit,
}

pub(crate) struct ItemInfo {
    pub item: Item,
    pub label: &'static str,
    pub blurb: &'static str,
    pub group: Group,
}

pub(crate) const ITEMS: [ItemInfo; ITEM_COUNT] = [
    ItemInfo { item: Item::Play, label: "PLAY", group: Group::Hero,
        blurb: "Join a server, start a solo map, or play a demo." },
    ItemInfo { item: Item::Profile, label: "PROFILE", group: Group::Primary,
        blurb: "Customize your character, Force, sabers, and appearance." },
    ItemInfo { item: Item::Controls, label: "CONTROLS", group: Group::Primary,
        blurb: "Keyboard and mouse bindings." },
    ItemInfo { item: Item::Settings, label: "SETTINGS", group: Group::Primary,
        blurb: "Use the same Setup pages available in game." },
    ItemInfo { item: Item::ChatLogs, label: "CHAT LOGS", group: Group::Utility,
        blurb: "Browse saved server chat by mod, server, date, message type, or text." },
    ItemInfo { item: Item::Screenshots, label: "SCREENSHOTS", group: Group::Utility,
        blurb: "Browse screenshots from base and every mod, inspect capture metadata, or return to a saved spot." },
    ItemInfo { item: Item::DeveloperTools, label: "DEVELOPER TOOLS", group: Group::Utility,
        blurb: "Browse and inspect game assets and source maps." },
    ItemInfo { item: Item::Quit, label: "QUIT", group: Group::Exit,
        blurb: "Exit DinurdoJK and return to the desktop." },
];

pub(crate) const TITLE: &str = "DINURDOJK";

/// The wall screen in the ctf4 menu shot that the CRT replaces: corners
/// TL, TR, BR, BL as fractions of the view, measured at 16:9 (2560x1440, no
/// camera sway) by line-fitting the visible yellow screen edges.
const TV_CORNERS_16_9: [[f32; 2]; 4] = [[0.679, 0.345], [0.939, 0.171], [0.963, 0.901], [0.679, 0.722]];

/// Those corners for a view of `aspect` (width / height). The menu camera keeps
/// its vertical field of view (horizontal FOV grows with width), so only x
/// scales about the centre. The camera does not sway on this stage.
pub(crate) fn tv_corners(aspect: f32) -> [[f32; 2]; 4] {
    let scale = (16.0 / 9.0) / aspect.max(0.1);
    TV_CORNERS_16_9.map(|[x, y]| [0.5 + (x - 0.5) * scale, y])
}

/// What the renderer needs to draw the page. Sent only when it changes; the
/// renderer animates everything (ignition, wipes, drips) from these edges.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MenuFrame {
    pub layout: layout::MenuLayout,
    /// Last entry the player pointed at or moved to; the lightbox and the
    /// CRT keep showing it.
    pub selected: u8,
    /// Entry under the pointer (or keyboard focus) right now. Only this one is
    /// fully lit and wiped clear; everything else fogs over again.
    pub active: Option<u8>,
    /// Last activated entry, for the press flash. Increments `press_serial`.
    pub pressed: u8,
    pub press_serial: u32,
    /// Power-on is independent of scene loading (always true on Main).
    pub revealed: bool,
    /// 0 = cached still, 1 = live world. Only affects the background pixels.
    pub live_scene_blend: f32,
}
