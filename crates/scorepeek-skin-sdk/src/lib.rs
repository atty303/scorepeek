//! Language-neutral scorepeek skin ABI v1 types and Rust guest helpers.
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::BTreeMap};

pub const MAX_AFTER_MS: u64 = 2_147_483_647;

#[derive(Deserialize)]
pub struct Input {
    pub schema: String,
    pub backend: String,
    pub canvas: Canvas,
    #[serde(default)]
    pub widgets: Vec<Widget>,
    pub state: serde_json::Value,
}

#[derive(Deserialize)]
pub struct Canvas {
    pub id: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct Widget {
    pub id: String,
    pub kind: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub settings: serde_json::Value,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Schedule {
    Idle,
    NextFrame,
    AfterMs { milliseconds: u64 },
}

#[derive(Serialize)]
pub struct Output {
    pub schedule: Schedule,
    pub tree: Node,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Node {
    Element {
        key: String,
        tag: String,
        attributes: BTreeMap<String, String>,
        children: Vec<Node>,
    },
    Text {
        key: String,
        text: String,
    },
}

impl Node {
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
}

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Reserves the sole host input buffer for one synchronous call.
#[must_use]
pub fn allocate(length: i32) -> i32 {
    if length <= 0 {
        return 0;
    }
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    INPUT.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        buffer.resize(length, 0);
        buffer.as_mut_ptr() as i32
    })
}

/// Decodes the buffer most recently returned by [`allocate`].
///
/// # Errors
/// Returns an error if the pointer/length pair is not the current allocation or its JSON is invalid.
pub fn decode(pointer: i32, length: i32) -> Result<Input, String> {
    let length = usize::try_from(length).map_err(|_| "input length is negative")?;
    INPUT.with(|buffer| {
        let buffer = buffer.borrow();
        if buffer.as_ptr() as i32 != pointer || buffer.len() != length {
            return Err("input does not match the allocated ABI buffer".into());
        }
        serde_json::from_slice(&buffer).map_err(|error| error.to_string())
    })
}

/// Releases the synchronous input buffer after a call.
pub fn deallocate(pointer: i32, length: i32) {
    let Ok(length) = usize::try_from(length) else {
        return;
    };
    INPUT.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        if buffer.as_ptr() as i32 == pointer && buffer.len() == length {
            buffer.clear();
        }
    });
}

/// Encodes output into guest-owned memory and returns high32 pointer, low32 length.
#[must_use]
pub fn encode(output: &Output) -> i64 {
    OUTPUT.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        *buffer = serde_json::to_vec(output).unwrap_or_else(|_| br#"{"schedule":{"kind":"idle"},"tree":{"kind":"element","key":"error","tag":"main","attributes":{},"children":[]}}"#.to_vec());
        let pointer = u64::try_from(buffer.as_ptr() as usize).unwrap_or_default() & 0xffff_ffff;
        let length = u64::try_from(buffer.len()).unwrap_or_default() & 0xffff_ffff;
        ((pointer << 32) | length).cast_signed()
    })
}
