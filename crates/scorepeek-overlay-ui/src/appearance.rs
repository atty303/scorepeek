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
    pub const fn graph_colors(self) -> GraphColors {
        match self {
            Self::CyanSystem => GraphColors {
                score: "#10dff4",
                miss: "#ffc719",
            },
            Self::ResultAurora => GraphColors {
                score: "#d368f2",
                miss: "#ffd334",
            },
            Self::DjBlackbox => GraphColors {
                score: "#b9e92b",
                miss: "#ffad00",
            },
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::CyanSystem => "cyan-system",
            Self::ResultAurora => "result-aurora",
            Self::DjBlackbox => "dj-blackbox",
        }
    }
}

#[derive(Clone, Copy)]
pub struct GraphColors {
    pub score: &'static str,
    pub miss: &'static str,
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
