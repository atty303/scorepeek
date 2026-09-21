use ash::vk;

pub const MAX_PLANES: usize = 4;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PlaneLayout {
    pub offset: u64,
    pub size: u64,
    pub row_pitch: u64,
    pub array_pitch: u64,
    pub depth_pitch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageContract {
    pub width: u32,
    pub height: u32,
    pub vk_format: u32,
    pub drm_fourcc: u32,
    pub modifier: u64,
    pub allocation_size: u64,
    pub plane_count: u32,
    pub device_uuid: [u8; vk::UUID_SIZE],
    pub planes: [PlaneLayout; MAX_PLANES],
}

pub(crate) fn validate_contract(contract: ImageContract) -> Result<(), String> {
    let format = vk::Format::from_raw(contract.vk_format.cast_signed());
    if contract.width == 0
        || contract.height == 0
        || contract.plane_count == 0
        || contract.plane_count as usize > MAX_PLANES
        || !matches!(
            format,
            vk::Format::B8G8R8A8_UNORM
                | vk::Format::B8G8R8A8_SRGB
                | vk::Format::R8G8B8A8_UNORM
                | vk::Format::R8G8B8A8_SRGB
        )
    {
        return Err("unsupported Vulkan image contract".to_owned());
    }
    byte_count(contract).map(|_| ())
}

pub(crate) fn byte_count(contract: ImageContract) -> Result<usize, String> {
    usize::try_from(contract.width)
        .ok()
        .and_then(|width| width.checked_mul(contract.height as usize))
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes <= 128 * 1024 * 1024)
        .ok_or_else(|| "Vulkan image byte count exceeds the capture bound".to_owned())
}
