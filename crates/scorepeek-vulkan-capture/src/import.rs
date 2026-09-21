//! Minimal Vulkan DMA-BUF import and readback boundary for Scorepeek capture.
//!
//! Vulkan is inherently unsafe. This crate is the only runtime crate permitted to issue Vulkan
//! calls; callers receive an owned byte slice after a local fence has completed.

use std::os::fd::OwnedFd;
use std::time::Instant;

use ash::{Entry, Instance, vk};

#[cfg(test)]
use crate::context::select_queue_family_index;
use crate::context::{select_physical_device, select_queue_family};
use crate::contract::{ImageContract, byte_count, validate_contract};
use crate::device::{
    GlobalPriorityCapabilities, GlobalPriorityExtension, ensure_extensions,
    global_priority_query_feature_supported, has_extension,
    queue_family_supports_low_global_priority, select_global_priority_extension,
};
use crate::readback::{Readback, ReadbackProfile};
use crate::resources::create_resources;

pub struct ImportedImage {
    pub(crate) _entry: Entry,
    pub(crate) instance: Instance,
    pub(crate) device: ash::Device,
    pub(crate) queue: vk::Queue,
    pub(crate) image: vk::Image,
    pub(crate) image_memory: vk::DeviceMemory,
    pub(crate) staging: vk::Buffer,
    pub(crate) staging_memory: vk::DeviceMemory,
    pub(crate) staging_mapped: usize,
    pub(crate) staging_coherent: bool,
    pub(crate) command_pool: vk::CommandPool,
    pub(crate) command: vk::CommandBuffer,
    pub(crate) fence: vk::Fence,
    pub(crate) contract: ImageContract,
    pub(crate) profile: ReadbackProfile,
    pub(crate) clock: Instant,
}

impl ImportedImage {
    /// Imports the single producer-owned DMA-BUF into a device selected by Vulkan UUID.
    ///
    /// # Errors
    /// Returns a stable textual operation plus the Vulkan result when the contract cannot be
    /// admitted. No partial Vulkan object escapes this boundary.
    pub fn import(contract: ImageContract, dma_buf: OwnedFd) -> Result<Self, String> {
        validate_contract(contract)?;
        // SAFETY: Entry owns the dynamically loaded Vulkan loader for every dependent handle.
        let entry = unsafe { Entry::load() }.map_err(|error| format!("load Vulkan: {error}"))?;
        let app_name = c"scorepeek";
        let app = vk::ApplicationInfo::default()
            .application_name(app_name)
            .application_version(1)
            .engine_name(app_name)
            .api_version(vk::API_VERSION_1_1);
        // SAFETY: create info points to stack data for the duration of the call.
        let instance = unsafe {
            entry.create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
        }
        .map_err(|error| format!("vkCreateInstance: {error}"))?;

        let created = (|| {
            let physical = select_physical_device(&instance, contract.device_uuid)?;
            let queue_family = select_queue_family(&instance, physical)?;
            let queue_flags =
                unsafe { instance.get_physical_device_queue_family_properties(physical) }
                    [queue_family as usize]
                    .queue_flags;
            let priority = [0.0_f32];
            let has_khr_global_priority =
                has_extension(&instance, physical, ash::khr::global_priority::NAME)?;
            let has_ext_global_priority =
                has_extension(&instance, physical, ash::ext::global_priority::NAME)?;
            let has_ext_global_priority_query =
                has_extension(&instance, physical, ash::ext::global_priority_query::NAME)?;
            let supports_global_priority_query = (has_khr_global_priority
                || has_ext_global_priority_query)
                && global_priority_query_feature_supported(&instance, physical);
            let low_priority_supported = supports_global_priority_query
                && queue_family_supports_low_global_priority(&instance, physical, queue_family);
            let global_priority_extension =
                select_global_priority_extension(GlobalPriorityCapabilities {
                    khr_low: has_khr_global_priority && low_priority_supported,
                    ext_low: has_ext_global_priority
                        && has_ext_global_priority_query
                        && low_priority_supported,
                });
            let mut global_priority = vk::DeviceQueueGlobalPriorityCreateInfoKHR::default()
                .global_priority(vk::QueueGlobalPriorityKHR::LOW);
            let mut queue_info = vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priority);
            if global_priority_extension.is_some() {
                queue_info = queue_info.push_next(&mut global_priority);
            }
            let queue_infos = [queue_info];
            let mut extension_names = vec![
                ash::khr::external_memory_fd::NAME.as_ptr(),
                ash::ext::external_memory_dma_buf::NAME.as_ptr(),
                ash::ext::image_drm_format_modifier::NAME.as_ptr(),
            ];
            if let Some(extension) = global_priority_extension {
                extension_names.push(match extension {
                    GlobalPriorityExtension::Ext => ash::ext::global_priority::NAME.as_ptr(),
                    GlobalPriorityExtension::Khr => ash::khr::global_priority::NAME.as_ptr(),
                });
            }
            ensure_extensions(&instance, physical, &extension_names)?;
            let device_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_infos)
                .enabled_extension_names(&extension_names);
            // SAFETY: queue and extension arrays live through the call.
            let device = unsafe { instance.create_device(physical, &device_info, None) }
                .map_err(|error| format!("vkCreateDevice: {error}"))?;
            // SAFETY: the requested queue exists by device creation above.
            let queue = unsafe { device.get_device_queue(queue_family, 0) };
            create_resources(
                entry.clone(),
                instance.clone(),
                physical,
                device,
                queue,
                queue_family,
                queue_flags,
                global_priority_extension.is_some(),
                contract,
                dma_buf,
            )
        })();
        if created.is_err() {
            // SAFETY: no successful ImportedImage owns the instance on this path.
            unsafe { instance.destroy_instance(None) };
        }
        created
    }

    /// Copies the imported image to host-visible memory and waits on a consumer-local fence.
    ///
    /// # Errors
    /// Returns the Vulkan operation which failed. No bytes are returned before the local fence
    /// completes and, for non-coherent memory, the mapped range is invalidated.
    #[allow(clippy::too_many_lines)]
    pub fn readback(&mut self) -> Result<Readback, String> {
        let byte_count = byte_count(self.contract)?;
        // SAFETY: all objects are owned by self, calls are serialized by &mut self, and the
        // producer protocol transfers external queue ownership before READY.
        unsafe {
            self.device
                .reset_fences(&[self.fence])
                .map_err(|error| format!("vkResetFences: {error}"))?;
            let commands = [self.command];
            let submissions = [vk::SubmitInfo::default().command_buffers(&commands)];
            self.device
                .queue_submit(self.queue, &submissions, self.fence)
                .map_err(|error| format!("vkQueueSubmit(readback): {error}"))?;
            let submit_ns = nanos(self.clock);
            self.device
                .wait_for_fences(&[self.fence], true, u64::MAX)
                .map_err(|error| format!("vkWaitForFences(readback): {error}"))?;
            let fence_ns = nanos(self.clock);
            if !self.staging_coherent
                && let Err(error) =
                    self.device
                        .invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default()
                            .memory(self.staging_memory)
                            .size(vk::WHOLE_SIZE)])
            {
                return Err(format!("vkInvalidateMappedMemoryRanges: {error}"));
            }
            let bytes = std::slice::from_raw_parts(self.staging_mapped as *const u8, byte_count)
                .to_vec()
                .into_boxed_slice();
            Ok(Readback {
                bytes,
                submit_ns,
                fence_ns,
            })
        }
    }

    #[must_use]
    pub const fn profile(&self) -> ReadbackProfile {
        self.profile
    }
}

impl Drop for ImportedImage {
    fn drop(&mut self) {
        // SAFETY: destruction is reverse creation order and this type uniquely owns all handles.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_buffer(self.staging, None);
            self.device.unmap_memory(self.staging_memory);
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.image_memory, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

fn nanos(clock: Instant) -> u64 {
    u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{MAX_PLANES, PlaneLayout};

    #[test]
    fn rejects_empty_image() {
        let contract = ImageContract {
            width: 0,
            height: 1,
            vk_format: vk::Format::B8G8R8A8_UNORM.as_raw().cast_unsigned(),
            drm_fourcc: 0,
            modifier: 0,
            allocation_size: 0,
            plane_count: 1,
            device_uuid: [0; vk::UUID_SIZE],
            planes: [PlaneLayout::default(); MAX_PLANES],
        };
        assert!(validate_contract(contract).is_err());
    }

    #[test]
    fn consumer_prefers_a_non_graphics_transfer_queue() {
        let families = [
            vk::QueueFamilyProperties {
                queue_flags: vk::QueueFlags::GRAPHICS
                    | vk::QueueFlags::COMPUTE
                    | vk::QueueFlags::TRANSFER,
                queue_count: 1,
                ..Default::default()
            },
            vk::QueueFamilyProperties {
                queue_flags: vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
                queue_count: 4,
                ..Default::default()
            },
        ];
        assert_eq!(select_queue_family_index(&families).unwrap(), 1);
    }

    #[test]
    fn consumer_falls_back_to_a_graphics_transfer_queue() {
        let families = [vk::QueueFamilyProperties {
            queue_flags: vk::QueueFlags::GRAPHICS | vk::QueueFlags::TRANSFER,
            queue_count: 1,
            ..Default::default()
        }];
        assert_eq!(select_queue_family_index(&families).unwrap(), 0);
    }

    #[test]
    fn consumer_selects_supported_global_priority_variants() {
        assert_eq!(
            select_global_priority_extension(GlobalPriorityCapabilities {
                khr_low: true,
                ..GlobalPriorityCapabilities::default()
            }),
            Some(GlobalPriorityExtension::Khr)
        );
        assert_eq!(
            select_global_priority_extension(GlobalPriorityCapabilities {
                ext_low: true,
                ..GlobalPriorityCapabilities::default()
            }),
            Some(GlobalPriorityExtension::Ext)
        );
        assert_eq!(
            select_global_priority_extension(GlobalPriorityCapabilities {
                khr_low: false,
                ext_low: false,
            }),
            None
        );
        assert_eq!(
            select_global_priority_extension(GlobalPriorityCapabilities {
                khr_low: true,
                ext_low: true,
            }),
            Some(GlobalPriorityExtension::Khr)
        );
    }
}
