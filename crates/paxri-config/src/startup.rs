use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatewaySettings {
    pub startup: Startup,
    pub session_keep_alive: SessionKeepAlive,
    pub limits: crate::limits::Limits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayChange {
    SessionKeepAlive(SessionKeepAlive),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionKeepAlive {
    #[default]
    #[serde(rename = "5m")]
    Minutes5,
    #[serde(rename = "30m")]
    Minutes30,
    #[serde(rename = "1h")]
    Hours1,
    #[serde(rename = "6h")]
    Hours6,
    #[serde(rename = "24h")]
    Hours24,
    #[serde(rename = "forever")]
    Forever,
}

impl SessionKeepAlive {
    pub const ALL: [Self; 6] = [
        Self::Minutes5,
        Self::Minutes30,
        Self::Hours1,
        Self::Hours6,
        Self::Hours24,
        Self::Forever,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minutes5 => "5m",
            Self::Minutes30 => "30m",
            Self::Hours1 => "1h",
            Self::Hours6 => "6h",
            Self::Hours24 => "24h",
            Self::Forever => "forever",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Minutes5 => "5 minutes",
            Self::Minutes30 => "30 minutes",
            Self::Hours1 => "1 hour",
            Self::Hours6 => "6 hours",
            Self::Hours24 => "24 hours",
            Self::Forever => "Forever",
        }
    }

    /// Maximum continuous offline duration before the gateway ends a hosted terminal session.
    /// `None` keeps hosted sessions alive until explicitly ended, exited, or unpaired.
    pub fn duration(self) -> Option<Duration> {
        match self {
            Self::Minutes5 => Some(Duration::from_secs(5 * 60)),
            Self::Minutes30 => Some(Duration::from_secs(30 * 60)),
            Self::Hours1 => Some(Duration::from_secs(60 * 60)),
            Self::Hours6 => Some(Duration::from_secs(6 * 60 * 60)),
            Self::Hours24 => Some(Duration::from_secs(24 * 60 * 60)),
            Self::Forever => None,
        }
    }
}

impl std::str::FromStr for SessionKeepAlive {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|option| option.as_str() == value)
            .ok_or("choose 5m, 30m, 1h, 6h, 24h or forever")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Startup {
    Manual,
    #[default]
    Gui,
    Login,
    System,
}

impl Startup {
    pub const ALL: [Self; 4] = [Self::Manual, Self::Gui, Self::Login, Self::System];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Gui => "gui",
            Self::Login => "login",
            Self::System => "system",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "Manually",
            Self::Gui => "When the app opens",
            Self::Login => "At sign-in",
            Self::System => "At system startup",
        }
    }
}

impl std::str::FromStr for Startup {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str() == value)
            .ok_or("choose manual, gui, login or system")
    }
}
