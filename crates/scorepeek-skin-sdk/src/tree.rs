use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Schedule {
    Idle,
    NextFrame,
    AfterMs { milliseconds: u64 },
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Output {
    pub schedule: Schedule,
    pub tree: Node,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Node {
    Element {
        key: String,
        tag: String,
        #[serde(default)]
        attributes: BTreeMap<String, String>,
        #[serde(default)]
        children: Vec<Node>,
    },
    Text {
        key: String,
        text: String,
    },
}

impl Node {
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Element { key, .. } | Self::Text { key, .. } => key,
        }
    }

    #[must_use]
    pub fn element(
        key: impl Into<String>,
        tag: impl Into<String>,
        attributes: BTreeMap<String, String>,
        children: Vec<Self>,
    ) -> Self {
        Self::Element {
            key: key.into(),
            tag: tag.into(),
            attributes,
            children,
        }
    }

    #[must_use]
    pub fn text(key: impl Into<String>, text: impl Into<String>) -> Self {
        Self::Text {
            key: key.into(),
            text: text.into(),
        }
    }

    fn validate(&self, keys: &mut BTreeSet<String>) -> Result<(), String> {
        let (key, children) = match self {
            Self::Element {
                key, tag, children, ..
            } => {
                if tag.is_empty() {
                    return Err("skin tree element tag must be non-empty".into());
                }
                (key, children.as_slice())
            }
            Self::Text { key, .. } => (key, &[][..]),
        };
        if key.is_empty() || !keys.insert(key.clone()) {
            return Err("skin tree keys must be non-empty and unique".into());
        }
        for child in children {
            child.validate(keys)?;
        }
        Ok(())
    }
}

impl Output {
    /// Validates scheduling bounds and stable tree keys.
    /// # Errors
    /// Returns a schedule, tag, or key contract violation.
    pub fn validate(&self) -> Result<(), String> {
        if matches!(self.schedule, Schedule::AfterMs { milliseconds } if milliseconds > crate::MAX_AFTER_MS)
        {
            return Err(format!(
                "skin schedule milliseconds must be at most {}",
                crate::MAX_AFTER_MS
            ));
        }
        self.tree.validate(&mut BTreeSet::new())
    }
}
