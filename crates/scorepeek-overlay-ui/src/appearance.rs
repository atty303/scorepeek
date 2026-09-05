use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Skin {
    #[default]
    CyanSystem,
    ResultAurora,
    DjBlackbox,
}

impl Skin {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CyanSystem => "cyan-system",
            Self::ResultAurora => "result-aurora",
            Self::DjBlackbox => "dj-blackbox",
        }
    }
}

impl std::str::FromStr for Skin {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "cyan-system" => Ok(Self::CyanSystem),
            "result-aurora" => Ok(Self::ResultAurora),
            "dj-blackbox" => Ok(Self::DjBlackbox),
            _ => Err("skin must be cyan-system, result-aurora or dj-blackbox".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Appearance {
    pub skin: Skin,
}
