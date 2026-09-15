//! Minimal Vulkan DMA-BUF import and readback boundary for Scorepeek capture.
//!
//! Vulkan is inherently unsafe. This crate is the only runtime crate permitted to issue Vulkan
//! calls; callers receive an owned byte slice after a local fence has completed.

use std::os::fd::{AsRawFd, OwnedFd};
use std::time::Instant;

use ash::{Entry, Instance, vk};

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

#[derive(Debug)]
pub struct Readback {
    pub bytes: Box<[u8]>,
    pub submit_ns: u64,
    pub fence_ns: u64,
}

pub struct ImportedImage {
    _entry: Entry,
    instance: Instance,
    device: ash::Device,
    queue: vk::Queue,
    queue_family: u32,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    staging_coherent: bool,
    command_pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    contract: ImageContract,
    clock: Instant,
}

struct PartialResources<'a> {
    device: &'a ash::Device,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    command_pool: vk::CommandPool,
    fence: vk::Fence,
    armed: bool,
}

impl Drop for PartialResources<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: every non-null handle was created by device and has not escaped. Vulkan null
        // handles are explicitly accepted by destroy/free operations.
        unsafe {
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_buffer(self.staging, None);
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.image_memory, None);
            self.device.destroy_device(None);
        }
    }
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
            let priority = [0.0_f32];
            let queue_info = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family)
                .queue_priorities(&priority)];
            let extension_names = [
                ash::khr::external_memory_fd::NAME.as_ptr(),
                ash::ext::external_memory_dma_buf::NAME.as_ptr(),
                ash::ext::image_drm_format_modifier::NAME.as_ptr(),
            ];
            ensure_extensions(&instance, physical, &extension_names)?;
            let device_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_info)
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
            self.device
                .reset_command_pool(self.command_pool, vk::CommandPoolResetFlags::empty())
                .map_err(|error| format!("vkResetCommandPool: {error}"))?;
            self.device
                .begin_command_buffer(
                    self.command,
                    &vk::CommandBufferBeginInfo::default()
                        .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
                )
                .map_err(|error| format!("vkBeginCommandBuffer: {error}"))?;
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .level_count(1)
                .layer_count(1);
            let acquire = vk::ImageMemoryBarrier::default()
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .dst_queue_family_index(self.queue_family)
                .image(self.image)
                .subresource_range(range);
            self.device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[acquire],
            );
            let subresource = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1);
            let copy = vk::BufferImageCopy::default()
                .image_subresource(subresource)
                .image_extent(vk::Extent3D {
                    width: self.contract.width,
                    height: self.contract.height,
                    depth: 1,
                });
            self.device.cmd_copy_image_to_buffer(
                self.command,
                self.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                self.staging,
                &[copy],
            );
            let release = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_READ)
                .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(self.queue_family)
                .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .image(self.image)
                .subresource_range(range);
            let host = vk::BufferMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::HOST_READ)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .buffer(self.staging)
                .size(vk::WHOLE_SIZE);
            self.device.cmd_pipeline_barrier(
                self.command,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::HOST | vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[host],
                &[release],
            );
            self.device
                .end_command_buffer(self.command)
                .map_err(|error| format!("vkEndCommandBuffer: {error}"))?;
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
            let mapped = self
                .device
                .map_memory(
                    self.staging_memory,
                    0,
                    vk::WHOLE_SIZE,
                    vk::MemoryMapFlags::empty(),
                )
                .map_err(|error| format!("vkMapMemory(staging): {error}"))?;
            if !self.staging_coherent
                && let Err(error) =
                    self.device
                        .invalidate_mapped_memory_ranges(&[vk::MappedMemoryRange::default()
                            .memory(self.staging_memory)
                            .size(vk::WHOLE_SIZE)])
            {
                self.device.unmap_memory(self.staging_memory);
                return Err(format!("vkInvalidateMappedMemoryRanges: {error}"));
            }
            let bytes = std::slice::from_raw_parts(mapped.cast::<u8>(), byte_count)
                .to_vec()
                .into_boxed_slice();
            self.device.unmap_memory(self.staging_memory);
            Ok(Readback {
                bytes,
                submit_ns,
                fence_ns,
            })
        }
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
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.image_memory, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn create_resources(
    entry: Entry,
    instance: Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    queue_family: u32,
    contract: ImageContract,
    dma_buf: OwnedFd,
) -> Result<ImportedImage, String> {
    let mut cleanup = PartialResources {
        device: &device,
        image: vk::Image::null(),
        image_memory: vk::DeviceMemory::null(),
        staging: vk::Buffer::null(),
        staging_memory: vk::DeviceMemory::null(),
        command_pool: vk::CommandPool::null(),
        fence: vk::Fence::null(),
        armed: true,
    };
    // SAFETY: all Vulkan pointers are backed by local arrays until each call completes.
    unsafe {
        let layouts = contract.planes.map(|plane| {
            vk::SubresourceLayout::default()
                .offset(plane.offset)
                .size(plane.size)
                .row_pitch(plane.row_pitch)
                .array_pitch(plane.array_pitch)
                .depth_pitch(plane.depth_pitch)
        });
        let modifier = vk::ImageDrmFormatModifierExplicitCreateInfoEXT::default()
            .drm_format_modifier(contract.modifier)
            .plane_layouts(&layouts[..contract.plane_count as usize]);
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        external.p_next = (&raw const modifier).cast();
        let image_info = vk::ImageCreateInfo::default()
            .push_next(&mut external)
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk::Format::from_raw(contract.vk_format.cast_signed()))
            .extent(vk::Extent3D {
                width: contract.width,
                height: contract.height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::DRM_FORMAT_MODIFIER_EXT)
            .usage(vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = device
            .create_image(&image_info, None)
            .map_err(|error| format!("vkCreateImage(import): {error}"))?;
        cleanup.image = image;
        let requirements = device.get_image_memory_requirements(image);
        let fd_loader = ash::khr::external_memory_fd::Device::new(&instance, &device);
        let mut fd_properties = vk::MemoryFdPropertiesKHR::default();
        fd_loader
            .get_memory_fd_properties(
                vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT,
                dma_buf.as_raw_fd(),
                &mut fd_properties,
            )
            .map_err(|error| format!("vkGetMemoryFdPropertiesKHR: {error}"))?;
        let allowed = requirements.memory_type_bits & fd_properties.memory_type_bits;
        let memory_type = allowed.trailing_zeros();
        if memory_type >= 32 {
            return Err("DMA-BUF has no compatible memory type".to_owned());
        }
        let raw_fd = dma_buf.as_raw_fd();
        let import = vk::ImportMemoryFdInfoKHR::default()
            .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT)
            .fd(raw_fd);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        dedicated.p_next = (&raw const import).cast();
        let image_memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .push_next(&mut dedicated)
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type),
                None,
            )
            .map_err(|error| format!("vkAllocateMemory(import): {error}"))?;
        cleanup.image_memory = image_memory;
        // Vulkan owns the imported descriptor after successful allocation.
        std::mem::forget(dma_buf);
        device
            .bind_image_memory(image, image_memory, 0)
            .map_err(|error| format!("vkBindImageMemory(import): {error}"))?;

        let size = byte_count(contract)? as u64;
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::TRANSFER_DST)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|error| format!("vkCreateBuffer(staging): {error}"))?;
        cleanup.staging = staging;
        let staging_requirements = device.get_buffer_memory_requirements(staging);
        let properties = instance.get_physical_device_memory_properties(physical);
        let (staging_type, staging_coherent) =
            choose_memory_type(&properties, staging_requirements.memory_type_bits)?;
        let staging_memory = device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(staging_requirements.size)
                    .memory_type_index(staging_type),
                None,
            )
            .map_err(|error| format!("vkAllocateMemory(staging): {error}"))?;
        cleanup.staging_memory = staging_memory;
        device
            .bind_buffer_memory(staging, staging_memory, 0)
            .map_err(|error| format!("vkBindBufferMemory(staging): {error}"))?;
        let command_pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(queue_family),
                None,
            )
            .map_err(|error| format!("vkCreateCommandPool: {error}"))?;
        cleanup.command_pool = command_pool;
        let command = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|error| format!("vkAllocateCommandBuffers: {error}"))?[0];
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|error| format!("vkCreateFence: {error}"))?;
        cleanup.fence = fence;
        cleanup.armed = false;
        drop(cleanup);
        Ok(ImportedImage {
            _entry: entry,
            instance,
            device,
            queue,
            queue_family,
            image,
            image_memory,
            staging,
            staging_memory,
            staging_coherent,
            command_pool,
            command,
            fence,
            contract,
            clock: Instant::now(),
        })
    }
}

fn validate_contract(contract: ImageContract) -> Result<(), String> {
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

fn byte_count(contract: ImageContract) -> Result<usize, String> {
    usize::try_from(contract.width)
        .ok()
        .and_then(|width| width.checked_mul(contract.height as usize))
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|bytes| *bytes <= 128 * 1024 * 1024)
        .ok_or_else(|| "Vulkan image byte count exceeds the capture bound".to_owned())
}

fn select_physical_device(
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

fn select_queue_family(instance: &Instance, physical: vk::PhysicalDevice) -> Result<u32, String> {
    // SAFETY: physical belongs to instance.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
    families
        .iter()
        .enumerate()
        .filter(|(_, family)| family.queue_flags.contains(vk::QueueFlags::TRANSFER))
        .max_by_key(|(_, family)| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .and_then(|(index, _)| u32::try_from(index).ok())
        .ok_or_else(|| "no transfer-capable Vulkan queue family".to_owned())
}

fn ensure_extensions(
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

fn choose_memory_type(
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

fn nanos(clock: Instant) -> u64 {
    u64::try_from(clock.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
