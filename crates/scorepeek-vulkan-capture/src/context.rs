//! Vulkan loader instance and physical-device selection.

use ash::{Instance, vk};

pub(crate) fn select_physical_device(
    instance: &Instance,
    uuid: [u8; vk::UUID_SIZE],
) -> Result<vk::PhysicalDevice, String> {
    // SAFETY: returned handles belong to instance; property chains are valid stack values.
    unsafe {
        instance
            .enumerate_physical_devices()
            .map_err(|error| format!("vkEnumeratePhysicalDevices: {error}"))?
            .into_iter()
            .find(|physical| {
                let mut ids = vk::PhysicalDeviceIDProperties::default();
                let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut ids);
                instance.get_physical_device_properties2(*physical, &mut properties);
                ids.device_uuid == uuid
            })
            .ok_or_else(|| "producer Vulkan device UUID is unavailable".to_owned())
    }
}

pub(crate) fn select_queue_family(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> Result<u32, String> {
    // SAFETY: physical belongs to instance.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    select_queue_family_index(&families)
}

pub(crate) fn select_queue_family_index(
    families: &[vk::QueueFamilyProperties],
) -> Result<u32, String> {
    families
        .iter()
        .enumerate()
        .filter(|(_, family)| family.queue_flags.contains(vk::QueueFlags::TRANSFER))
        .min_by_key(|(_, family)| {
            (
                family.queue_flags.contains(vk::QueueFlags::GRAPHICS),
                family.queue_flags.contains(vk::QueueFlags::COMPUTE),
            )
        })
        .and_then(|(index, _)| u32::try_from(index).ok())
        .ok_or_else(|| "no transfer-capable Vulkan queue family".to_owned())
}
