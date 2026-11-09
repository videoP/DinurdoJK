//! Declarative bridges between console text and typed subsystem state.
use super::{Entry, EntryKind, LatchScope};
use std::{collections::HashMap, fmt};

#[derive(Debug, Clone, PartialEq)]
pub enum CvarValue {
    Bool(bool),
    Int(i32),
    Float(f32),
    String(String),
}

impl fmt::Display for CvarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(value) => write!(f, "{}", u8::from(*value)),
            Self::Int(value) => value.fmt(f),
            Self::Float(value) => value.fmt(f),
            Self::String(value) => value.fmt(f),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CvarType {
    Bool,
    Int { min: i32, max: i32 },
    Float { min: f32, max: f32 },
    String,
    Enum(&'static [&'static str]),
}

impl CvarType {
    pub fn parse(self, value: &str) -> Result<CvarValue, String> {
        let trimmed = value.trim();
        match self {
            Self::Bool => match trimmed.to_ascii_lowercase().as_str() {
                "1" | "on" | "true" | "yes" => Ok(CvarValue::Bool(true)),
                "0" | "off" | "false" | "no" => Ok(CvarValue::Bool(false)),
                _ => Err("expected 0/1, off/on, false/true".into()),
            },
            Self::Int { min, max } => trimmed
                .parse::<i32>()
                .ok()
                .filter(|value| {
                    (min..=max).contains(value) && !(min >= 0 && trimmed.starts_with('-'))
                })
                .map(CvarValue::Int)
                .ok_or_else(|| {
                    if min == 0 && (2..=3).contains(&max) {
                        let choices: Vec<_> = (min..max).map(|value| value.to_string()).collect();
                        format!("expected {}, or {max}", choices.join(", "))
                    } else {
                        format!("expected an integer from {min} to {max}")
                    }
                }),
            Self::Float { min, max } => trimmed
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite() && (min..=max).contains(value))
                .map(CvarValue::Float)
                .ok_or_else(|| format!("expected a finite number from {min} to {max}")),
            Self::String => Ok(CvarValue::String(value.to_owned())),
            Self::Enum(choices) => choices
                .iter()
                .find(|choice| choice.eq_ignore_ascii_case(trimmed))
                .map(|choice| CvarValue::String((*choice).to_owned()))
                .ok_or_else(|| format!("expected {}", choices.join(" or "))),
        }
    }

    fn valid_domain(self) -> bool {
        match self {
            Self::Int { min, max } => min <= max,
            Self::Float { min, max } => min.is_finite() && max.is_finite() && min <= max,
            Self::Enum(choices) => {
                !choices.is_empty()
                    && choices.iter().enumerate().all(|(i, value)| {
                        !value.is_empty()
                            && !choices[..i]
                                .iter()
                                .any(|other| other.eq_ignore_ascii_case(value))
                    })
            }
            _ => true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CvarFlags(u8);
impl CvarFlags {
    pub const NONE: Self = Self(0);
    pub const ARCHIVE: Self = Self(1);
    pub const USERINFO: Self = Self(2);
    /// A fixed interface value has no mutable settings field to load.
    pub const FIXED: Self = Self(4);
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }
}

/// Adapter for scalar settings whose console, config and archive forms are
/// identical. Only implement types with a lossless, checked conversion.
/// Complex/legacy cvars keep explicit adapters in their declaration.
pub(crate) trait CvarScalar: Copy {
    fn cvar_value(self) -> CvarValue;
    fn from_cvar_value(value: CvarValue) -> Result<Self, String>;
}

impl CvarScalar for bool {
    fn cvar_value(self) -> CvarValue { CvarValue::Bool(self) }
    fn from_cvar_value(value: CvarValue) -> Result<Self, String> {
        match value { CvarValue::Bool(v) => Ok(v), _ => Err("expected boolean cvar".into()) }
    }
}
impl CvarScalar for f32 {
    fn cvar_value(self) -> CvarValue { CvarValue::Float(self) }
    fn from_cvar_value(value: CvarValue) -> Result<Self, String> {
        match value { CvarValue::Float(v) if v.is_finite() => Ok(v), _ => Err("expected finite float cvar".into()) }
    }
}
impl CvarScalar for i32 {
    fn cvar_value(self) -> CvarValue { CvarValue::Int(self) }
    fn from_cvar_value(value: CvarValue) -> Result<Self, String> {
        match value { CvarValue::Int(v) => Ok(v), _ => Err("expected integer cvar".into()) }
    }
}
impl CvarScalar for u8 {
    fn cvar_value(self) -> CvarValue { CvarValue::Int(self.into()) }
    fn from_cvar_value(value: CvarValue) -> Result<Self, String> {
        match value { CvarValue::Int(v) => Self::try_from(v).map_err(|_| "integer exceeds u8 range".into()),
            _ => Err("expected integer cvar".into()) }
    }
}

impl CvarScalar for u32 {
    fn cvar_value(self) -> CvarValue { CvarValue::Int(i32::try_from(self).expect("u32 cvar exceeds i32 domain")) }
    fn from_cvar_value(value: CvarValue) -> Result<Self, String> {
        match value {
            CvarValue::Int(v) => Self::try_from(v).map_err(|_| "negative integer for u32 cvar".into()),
            _ => Err("expected integer cvar".into()),
        }
    }
}

pub struct CvarDef<State: 'static> {
    pub entry: Entry,
    pub value_type: CvarType,
    pub flags: CvarFlags,
    pub get: fn(&State) -> CvarValue,
    pub set: fn(&mut State, CvarValue) -> Result<(), String>,
    pub set_text: Option<fn(&mut State, &str, &str) -> Result<(), String>>,
    pub normalize: Option<fn(&str, &str) -> Result<String, String>>,
    /// Predicate for resource-dependent changes that cannot take effect live.
    /// Unlike LatchScope, this can depend on the resources currently prepared.
    pub restart_required: Option<fn(&State) -> bool>,
}

impl<State> CvarDef<State> {
    pub const fn new(
        name: &'static str,
        default: &'static str,
        range: &'static str,
        description: &'static str,
        value_type: CvarType,
        flags: CvarFlags,
        latch: LatchScope,
        get: fn(&State) -> CvarValue,
        set: fn(&mut State, CvarValue) -> Result<(), String>,
    ) -> Self {
        Self {
            entry: Entry {
                name,
                kind: EntryKind::Cvar,
                default,
                range,
                description,
                latch,
                usage: "",
            },
            value_type,
            flags,
            get,
            set,
            set_text: None,
            normalize: None,
            restart_required: None,
        }
    }

    /// Custom callbacks retain established JKA numeric/clamping and formatting rules.
    /// Values live in typed state; this adapter carries console text only at the boundary.
    pub const fn with_text_setter(
        mut self,
        setter: fn(&mut State, &str, &str) -> Result<(), String>,
        normalize: Option<fn(&str, &str) -> Result<String, String>>,
    ) -> Self {
        self.set_text = Some(setter);
        self.normalize = normalize;
        self
    }

    /// Conditional restart contract lives on the cvar declaration rather
    /// than being independently reinvented in the renderer and menus.
    pub const fn with_restart_requirement(mut self, required: fn(&State) -> bool) -> Self {
        self.restart_required = Some(required);
        self
    }

    pub const fn copy(&self) -> Self {
        Self {
            entry: self.entry,
            value_type: self.value_type,
            flags: self.flags,
            get: self.get,
            set: self.set,
            set_text: self.set_text,
            normalize: self.normalize,
            restart_required: self.restart_required,
        }
    }
    pub fn with_default(&self, default: String) -> &'static Self {
        let mut definition = self.copy();
        definition.entry.default = Box::leak(default.into_boxed_str());
        Box::leak(Box::new(definition))
    }

    pub fn parse(&self, value: &str) -> Result<CvarValue, String> {
        self.value_type
            .parse(value)
            .map_err(|error| format!("{}: {error}", self.entry.name))
    }

    /// One validator/normalizer for staged console values. The same
    /// declaration owns the type, optional legacy text normalization, and
    /// active setter; staging cannot accept a different syntax by accident.
    pub fn normalize_for_latch(&self, value: &str) -> Result<String, String> {
        if let Some(normalize) = self.normalize {
            normalize(self.entry.name, value)
        } else {
            self.parse(value).map(|value| value.to_string())
        }
    }

    pub fn default_value(&self) -> CvarValue {
        self.parse(self.entry.default)
            .expect("validated cvar default")
    }

    /// Bind the same declaration to a larger state without duplicating metadata.
    pub const fn bind<Other>(
        &self,
        get: fn(&Other) -> CvarValue,
        set: fn(&mut Other, CvarValue) -> Result<(), String>,
    ) -> CvarDef<Other> {
        CvarDef {
            entry: self.entry,
            value_type: self.value_type,
            flags: self.flags,
            get,
            set,
            set_text: None,
            normalize: None,
            restart_required: None,
        }
    }
}

pub struct CvarRegistry<State: 'static> {
    definitions: Vec<&'static CvarDef<State>>,
    by_name: HashMap<String, &'static CvarDef<State>>,
}

impl<State> CvarRegistry<State> {
    pub fn new(definitions: impl IntoIterator<Item = &'static CvarDef<State>>) -> Self {
        let definitions: Vec<_> = definitions.into_iter().collect();
        let mut by_name = HashMap::new();
        for def in &definitions {
            assert!(!def.entry.name.is_empty(), "empty cvar name");
            assert!(
                def.value_type.valid_domain(),
                "invalid domain: {}",
                def.entry.name
            );
            assert!(
                def.parse(def.entry.default).is_ok(),
                "invalid default: {}",
                def.entry.name
            );
            assert!(
                by_name
                    .insert(def.entry.name.to_ascii_lowercase(), *def)
                    .is_none(),
                "duplicate cvar: {}",
                def.entry.name
            );
        }
        Self {
            definitions,
            by_name,
        }
    }

    pub fn with_default_state(
        definitions: impl IntoIterator<Item = &'static CvarDef<State>>,
        state: &State,
    ) -> Self {
        Self::new(
            definitions
                .into_iter()
                .map(|def| def.with_default((def.get)(state).to_string())),
        )
    }

    pub fn find(&self, name: &str) -> Option<&'static CvarDef<State>> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }
    pub fn iter(&self) -> impl Iterator<Item = &'static CvarDef<State>> + '_ {
        self.definitions.iter().copied()
    }
    /// Single registry-driven check for resource-dependent restarts.
    pub fn requires_restart(&self, state: &State) -> bool {
        self.iter().any(|def| def.restart_required.is_some_and(|required| required(state)))
    }
    #[cfg(test)]
    pub fn archived(&self) -> impl Iterator<Item = &'static CvarDef<State>> + '_ {
        self.iter()
            .filter(|def| def.flags.contains(CvarFlags::ARCHIVE))
    }
    pub fn get(&self, state: &State, name: &str) -> Option<CvarValue> {
        self.find(name).map(|def| (def.get)(state))
    }
    /// Apply a value immediately. Console callers use the existing latch layer
    /// before reaching this API; config loading changes inactive typed settings.
    pub fn set(&self, state: &mut State, name: &str, value: &str) -> Result<(), String> {
        let def = self
            .find(name)
            .ok_or_else(|| format!("Cvar {name} does not exist."))?;
        if let Some(set) = def.set_text {
            set(state, name, value)
        } else {
            (def.set)(state, def.parse(value)?)
        }
    }
    pub fn apply_defaults(&self, state: &mut State) -> Result<(), String> {
        for def in self.iter() {
            (def.set)(state, def.default_value())?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn write_archive(&self, state: &State) -> String {
        use std::fmt::Write;
        let mut text = String::new();
        for def in self.archived() {
            let _ = writeln!(text, "seta {} \"{}\"", def.entry.name, (def.get)(state));
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct State {
        mode: i32,
        changes: usize,
    }
    static MODE: CvarDef<State> = CvarDef::new(
        "mode",
        "1",
        "0..2",
        "Test mode",
        CvarType::Int { min: 0, max: 2 },
        CvarFlags::ARCHIVE,
        LatchScope::None,
        |state| CvarValue::Int(state.mode),
        |state, value| {
            let CvarValue::Int(value) = value else {
                unreachable!()
            };
            state.mode = value;
            state.changes += 1;
            Ok(())
        },
    );
    #[test]
    fn validated_setting_and_archive_round_trip() {
        let registry = CvarRegistry::new([&MODE]);
        let mut state = State::default();
        registry.set(&mut state, "MODE", "2").unwrap();
        assert_eq!(registry.get(&state, "mode"), Some(CvarValue::Int(2)));
        assert_eq!(state.changes, 1);
        assert!(registry.set(&mut state, "mode", "3").is_err());
        assert!(registry.set(&mut state, "mode", "-0").is_err());
        assert_eq!(state.changes, 1);
        assert_eq!(registry.write_archive(&state), "seta mode \"2\"\n");
        assert_eq!(MODE.default_value(), CvarValue::Int(1));
        assert!(registry.set(&mut state, "missing", "0").is_err());
    }
    #[test]
    fn authoritative_defaults_apply_to_typed_state() {
        let registry = CvarRegistry::new([&MODE]);
        let mut state = State::default();
        registry.apply_defaults(&mut state).unwrap();
        assert_eq!(state.mode, 1);
        assert_eq!(state.changes, 1);
    }

    #[test]
    fn typed_domains_reject_nonfinite_and_invalid_enums() {
        let float = CvarType::Float { min: 0.0, max: 1.0 };
        for value in ["NaN", "inf", "-1", "2"] {
            assert!(float.parse(value).is_err());
        }
        assert_eq!(float.parse("0.5"), Ok(CvarValue::Float(0.5)));
        let enumeration = CvarType::Enum(&["vulkan", "dx12"]);
        assert_eq!(
            enumeration.parse("DX12"),
            Ok(CvarValue::String("dx12".into()))
        );
        assert!(enumeration.parse("opengl").is_err());
        assert_eq!(
            CvarType::String.parse("  a b  "),
            Ok(CvarValue::String("  a b  ".into()))
        );
        assert_eq!(CvarType::Bool.parse("YES"), Ok(CvarValue::Bool(true)));
        assert!(CvarType::Int { min: 2, max: 1 }.valid_domain() == false);
    }
    #[test]
    #[should_panic(expected = "duplicate cvar")]
    fn duplicate_names_are_rejected() {
        CvarRegistry::new([&MODE, &MODE]);
    }
}
