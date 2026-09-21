/// Stable output metadata exposed without leaking a Wayland proxy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputDescription {
    pub name: String,
    pub model: String,
    pub logical_size: Option<[u32; 2]>,
}
