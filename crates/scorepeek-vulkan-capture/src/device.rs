//! Logical-device, queue, and extension selection.

use ash::{Instance, vk};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GlobalPriorityExtension {
    Ext,
    Khr,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct GlobalPriorityCapabilities {
    pub(crate) khr_low: bool,
    pub(crate) ext_low: bool,
}

pub(crate) fn has_extension(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    expected: &std::ffi::CStr,
) -> Result<bool, String> {
    // SAFETY: physical belongs to instance.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
        .map_err(|error| format!("vkEnumerateDeviceExtensionProperties: {error}"))?;
    Ok(extensions.iter().any(|extension| {
        // SAFETY: Vulkan guarantees NUL-terminated extension_name.
        unsafe { std::ffi::CStr::from_ptr(extension.extension_name.as_ptr()) == expected }
    }))
}

pub(crate) fn queue_family_supports_low_global_priority(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    queue_family: u32,
) -> bool {
    // SAFETY: physical belongs to instance, and every p_next points to an element retained until
    // the query returns.
    unsafe {
        let count = instance.get_physical_device_queue_family_properties2_len(physical);
        let Ok(queue_family) = usize::try_from(queue_family) else {
            return false;
        };
        if queue_family >= count {
            return false;
        }
        let mut priorities = vec![vk::QueueFamilyGlobalPriorityPropertiesKHR::default(); count];
        let mut properties = vec![vk::QueueFamilyProperties2::default(); count];
        for (property, priority) in properties.iter_mut().zip(&mut priorities) {
            property.p_next = std::ptr::from_mut(priority).cast();
        }
        instance.get_physical_device_queue_family_properties2(physical, &mut properties);
        priorities[queue_family]
            .priorities_as_slice()
            .contains(&vk::QueueGlobalPriorityKHR::LOW)
    }
}

pub(crate) fn global_priority_query_feature_supported(
    instance: &Instance,
    physical: vk::PhysicalDevice,
) -> bool {
    let mut query = vk::PhysicalDeviceGlobalPriorityQueryFeaturesKHR::default();
    let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut query);
    // SAFETY: physical belongs to instance and query is retained through the call.
    unsafe { instance.get_physical_device_features2(physical, &mut features) };
    query.global_priority_query == vk::TRUE
}

pub(crate) const fn select_global_priority_extension(
    capabilities: GlobalPriorityCapabilities,
) -> Option<GlobalPriorityExtension> {
    if capabilities.khr_low {
        Some(GlobalPriorityExtension::Khr)
    } else if capabilities.ext_low {
        Some(GlobalPriorityExtension::Ext)
    } else {
        None
    }
}

pub(crate) fn ensure_extensions(
    instance: &Instance,
    physical: vk::PhysicalDevice,
    required: &[*const i8],
) -> Result<(), String> {
    // SAFETY: physical belongs to instance.
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
        .map_err(|error| format!("vkEnumerateDeviceExtensionProperties: {error}"))?;
    for name in required {
        // SAFETY: extension constants and Vulkan property names are NUL terminated.
        let expected = unsafe { std::ffi::CStr::from_ptr(*name) };
        let found = extensions.iter().any(|extension| {
            // SAFETY: Vulkan guarantees NUL-terminated extension_name.
            unsafe { std::ffi::CStr::from_ptr(extension.extension_name.as_ptr()) == expected }
        });
        if !found {
            return Err(format!(
                "required Vulkan extension is unavailable: {}",
                expected.to_string_lossy()
            ));
        }
    }
    Ok(())
}

pub(crate) fn choose_memory_type(
    properties: &vk::PhysicalDeviceMemoryProperties,
    allowed: u32,
) -> Result<(u32, bool), String> {
    let required = vk::MemoryPropertyFlags::HOST_VISIBLE;
    let preferred = vk::MemoryPropertyFlags::HOST_COHERENT | vk::MemoryPropertyFlags::HOST_CACHED;
    for prefer in [true, false] {
        for index in 0..properties.memory_type_count {
            if allowed & (1 << index) == 0 {
                continue;
            }
            let flags = properties.memory_types[index as usize].property_flags;
            if flags.contains(required) && (!prefer || flags.contains(preferred)) {
                return Ok((
                    index,
                    flags.contains(vk::MemoryPropertyFlags::HOST_COHERENT),
                ));
            }
        }
    }
    Err("no host-visible staging memory type".to_owned())
}
