#[derive(Debug)]
pub struct Readback {
    pub bytes: Box<[u8]>,
    pub submit_ns: u64,
    pub fence_ns: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadbackProfile {
    pub queue_family: u32,
    pub queue_flags: u32,
    pub global_priority_low: bool,
    pub commands_prerecorded: bool,
    pub staging_persistently_mapped: bool,
}
