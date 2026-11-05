//! Studio route-trail tick label style.

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudioTickLabels {
    #[default] Off,
    Seconds,
    MinutesSeconds,
}
impl StudioTickLabels {
    pub fn text(self, seconds: u32) -> Option<String> {
        match self {
            Self::Off => None,
            Self::Seconds => Some(seconds.to_string()),
            Self::MinutesSeconds => Some(format!("{}:{:02}", seconds / 60, seconds % 60)),
        }
    }
}

