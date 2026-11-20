use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GatewaySettings {
    pub startup: Startup,
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
