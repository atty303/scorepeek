//! Owned Vulkan image, staging, command, and fence resources.

use crate::contract::{ImageContract, byte_count};
use crate::device::choose_memory_type;
use crate::import::ImportedImage;
use crate::readback::ReadbackProfile;
use ash::{Entry, Instance, vk};
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::Instant;

struct PartialResources<'a> {
    device: &'a ash::Device,
    image: vk::Image,
    image_memory: vk::DeviceMemory,
    staging: vk::Buffer,
    staging_memory: vk::DeviceMemory,
    staging_mapped: bool,
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
            if self.staging_mapped {
                self.device.unmap_memory(self.staging_memory);
            }
            self.device.free_memory(self.staging_memory, None);
            self.device.destroy_image(self.image, None);
            self.device.free_memory(self.image_memory, None);
            self.device.destroy_device(None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub(crate) fn create_resources(
    entry: Entry,
    instance: Instance,
    physical: vk::PhysicalDevice,
    device: ash::Device,
    queue: vk::Queue,
    queue_family: u32,
    queue_flags: vk::QueueFlags,
    global_priority_low: bool,
    contract: ImageContract,
    dma_buf: OwnedFd,
) -> Result<ImportedImage, String> {
    let mut cleanup = PartialResources {
        device: &device,
        image: vk::Image::null(),
        image_memory: vk::DeviceMemory::null(),
        staging: vk::Buffer::null(),
        staging_memory: vk::DeviceMemory::null(),
        staging_mapped: false,
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
        let staging_mapped = device
            .map_memory(
                staging_memory,
                0,
                vk::WHOLE_SIZE,
                vk::MemoryMapFlags::empty(),
            )
            .map_err(|error| format!("vkMapMemory(staging): {error}"))?;
        cleanup.staging_mapped = true;
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
        record_readback_commands(&device, command, image, staging, queue_family, contract)?;
        cleanup.armed = false;
        drop(cleanup);
        Ok(ImportedImage {
            _entry: entry,
            instance,
            device,
            queue,
            image,
            image_memory,
            staging,
            staging_memory,
            staging_mapped: staging_mapped as usize,
            staging_coherent,
            command_pool,
            command,
            fence,
            contract,
            profile: ReadbackProfile {
                queue_family,
                queue_flags: queue_flags.as_raw(),
                global_priority_low,
                commands_prerecorded: true,
                staging_persistently_mapped: true,
            },
            clock: Instant::now(),
        })
    }
}

unsafe fn record_readback_commands(
    device: &ash::Device,
    command: vk::CommandBuffer,
    image: vk::Image,
    staging: vk::Buffer,
    queue_family: u32,
    contract: ImageContract,
) -> Result<(), String> {
    // SAFETY: the caller owns every handle and does not submit the command buffer while recording.
    unsafe {
        device
            .begin_command_buffer(command, &vk::CommandBufferBeginInfo::default())
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
            .dst_queue_family_index(queue_family)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier(
            command,
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
                width: contract.width,
                height: contract.height,
                depth: 1,
            });
        device.cmd_copy_image_to_buffer(
            command,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            staging,
            &[copy],
        );
        let release = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_READ)
            .old_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(queue_family)
            .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
            .image(image)
            .subresource_range(range);
        let host = vk::BufferMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(staging)
            .size(vk::WHOLE_SIZE);
        device.cmd_pipeline_barrier(
            command,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::HOST | vk::PipelineStageFlags::BOTTOM_OF_PIPE,
            vk::DependencyFlags::empty(),
            &[],
            &[host],
            &[release],
        );
        device
            .end_command_buffer(command)
            .map_err(|error| format!("vkEndCommandBuffer: {error}"))?;
    }
    Ok(())
}
