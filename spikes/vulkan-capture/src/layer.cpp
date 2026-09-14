#include "protocol.h"
#include "vkroots.h"

#include <vulkan/vulkan.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>
#include <sys/socket.h>
#include <sys/un.h>
#include <thread>
#include <unistd.h>
#include <utility>
#include <vector>

namespace scorepeek_capture {

constexpr uint32_t fourcc(char a, char b, char c, char d) {
    return static_cast<uint32_t>(a) | (static_cast<uint32_t>(b) << 8u) |
        (static_cast<uint32_t>(c) << 16u) | (static_cast<uint32_t>(d) << 24u);
}

constexpr uint32_t DRM_FORMAT_ARGB8888 = fourcc('A', 'R', '2', '4');
constexpr uint32_t DRM_FORMAT_ABGR8888 = fourcc('A', 'B', '2', '4');
constexpr size_t MAX_PRESENT_SEMAPHORES = 16;

uint64_t monotonic_ns() {
    const auto value = std::chrono::steady_clock::now().time_since_epoch();
    return static_cast<uint64_t>(
        std::chrono::duration_cast<std::chrono::nanoseconds>(value).count());
}

bool supported_format(VkFormat format) {
    return format == VK_FORMAT_B8G8R8A8_UNORM ||
        format == VK_FORMAT_B8G8R8A8_SRGB ||
        format == VK_FORMAT_R8G8B8A8_UNORM ||
        format == VK_FORMAT_R8G8B8A8_SRGB;
}

uint32_t drm_format(VkFormat format) {
    return format == VK_FORMAT_B8G8R8A8_UNORM ||
            format == VK_FORMAT_B8G8R8A8_SRGB
        ? DRM_FORMAT_ARGB8888
        : DRM_FORMAT_ABGR8888;
}

bool has_name(const std::vector<const char*>& names, const char* name) {
    return std::ranges::any_of(names, [name](const char* candidate) {
        return std::strcmp(candidate, name) == 0;
    });
}

bool physical_device_has_extensions(
    const vkroots::VkPhysicalDeviceDispatch& dispatch,
    VkPhysicalDevice physical_device,
    const std::array<const char*, 3>& required) {
    uint32_t count = 0;
    if (dispatch.EnumerateDeviceExtensionProperties(
            physical_device, nullptr, &count, nullptr) != VK_SUCCESS) {
        return false;
    }
    std::vector<VkExtensionProperties> extensions(count);
    if (dispatch.EnumerateDeviceExtensionProperties(
            physical_device, nullptr, &count, extensions.data()) != VK_SUCCESS) {
        return false;
    }
    return std::ranges::all_of(required, [&](const char* name) {
        return std::ranges::any_of(extensions, [name](const auto& extension) {
            return std::strcmp(extension.extensionName, name) == 0;
        });
    });
}

uint32_t choose_device_local_memory(
    const VkPhysicalDeviceMemoryProperties& properties,
    uint32_t allowed) {
    for (uint32_t pass = 0; pass != 2; ++pass) {
        for (uint32_t index = 0; index != properties.memoryTypeCount; ++index) {
            if ((allowed & (1u << index)) == 0) {
                continue;
            }
            const auto flags = properties.memoryTypes[index].propertyFlags;
            if (pass == 0 && (flags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT) == 0) {
                continue;
            }
            return index;
        }
    }
    return UINT32_MAX;
}

enum class CapturePhase : uint32_t {
    disconnected,
    idle,
    recording,
    submitted,
    awaiting_ack,
    disabled,
};

struct DeviceState;

class SwapchainCapture {
public:
    SwapchainCapture(
        std::shared_ptr<DeviceState> device_state,
        VkSwapchainKHR swapchain,
        VkFormat format,
        VkExtent2D extent,
        std::vector<VkImage> images);
    ~SwapchainCapture();

    SwapchainCapture(const SwapchainCapture&) = delete;
    SwapchainCapture& operator=(const SwapchainCapture&) = delete;

    bool initialize();
    bool capture(
        const vkroots::VkQueueDispatch& queue_dispatch,
        VkQueue queue,
        uint32_t image_index,
        const VkPresentInfoKHR* present_info,
        VkPresentInfoKHR* replacement,
        VkSemaphore* replacement_wait);
    void disable();

    VkSwapchainKHR swapchain() const { return swapchain_; }

private:
    bool initialize_export_image();
    bool initialize_queue(const vkroots::VkQueueDispatch& dispatch, VkQueue queue);
    bool send_hello(int socket_fd);
    bool send_packet(const SpvkPacket& packet);
    void socket_loop();
    void fence_loop();
    void close_socket();
    void record_error(uint64_t sequence, SpvkErrorType error_type);

    std::shared_ptr<DeviceState> device_state_;
    VkSwapchainKHR swapchain_ = VK_NULL_HANDLE;
    VkFormat format_ = VK_FORMAT_UNDEFINED;
    VkExtent2D extent_{};
    std::vector<VkImage> images_;

    VkImage export_image_ = VK_NULL_HANDLE;
    VkDeviceMemory export_memory_ = VK_NULL_HANDLE;
    int export_fd_ = -1;
    uint64_t allocation_size_ = 0;
    uint64_t modifier_ = 0;
    uint32_t plane_count_ = 0;
    std::array<SpvkPlaneLayout, SPVK_MAX_PLANES> plane_layouts_{};
    std::array<uint8_t, VK_UUID_SIZE> device_uuid_{};

    VkCommandPool command_pool_ = VK_NULL_HANDLE;
    VkCommandBuffer command_buffer_ = VK_NULL_HANDLE;
    VkFence fence_ = VK_NULL_HANDLE;
    VkSemaphore present_semaphore_ = VK_NULL_HANDLE;
    uint32_t queue_family_ = UINT32_MAX;
    bool first_export_ = true;

    std::atomic<CapturePhase> phase_{CapturePhase::disconnected};
    std::atomic<bool> stop_{false};
    std::atomic<bool> pending_{false};
    std::atomic<int> socket_fd_{-1};
    std::atomic<uint64_t> run_id_{0};
    std::atomic<uint64_t> pending_sequence_{0};
    std::atomic<uint64_t> pending_request_ns_{0};
    std::atomic<uint64_t> requests_{0};
    std::atomic<uint64_t> captures_{0};
    std::atomic<uint64_t> busy_drops_{0};
    std::atomic<uint64_t> coalesced_drops_{0};
    std::atomic<uint64_t> submitted_sequence_{0};
    std::atomic<uint64_t> submitted_request_ns_{0};
    std::atomic<uint64_t> submitted_present_ns_{0};
    std::atomic<uint64_t> submitted_done_ns_{0};
    std::mutex send_mutex_;
    std::thread socket_thread_;
    std::thread fence_thread_;
};

struct DeviceState {
    DeviceState(
        const vkroots::VkDeviceDispatch* dispatch_value,
        VkDevice device_value,
        VkPhysicalDevice physical_value,
        std::string socket_path_value,
        bool enabled_value)
        : dispatch(dispatch_value),
          device(device_value),
          physical_device(physical_value),
          socket_path(std::move(socket_path_value)),
          enabled(enabled_value) {}

    const vkroots::VkDeviceDispatch* dispatch;
    VkDevice device;
    VkPhysicalDevice physical_device;
    std::string socket_path;
    bool enabled;
    std::atomic<SwapchainCapture*> active{nullptr};
    std::mutex captures_mutex;
    std::vector<std::unique_ptr<SwapchainCapture>> captures;
};

std::shared_ptr<DeviceState> get_device_state(const vkroots::VkDeviceDispatch& dispatch) {
    if (!dispatch.UserData.has()) {
        return {};
    }
    return vkroots::userdata_cast<std::shared_ptr<DeviceState>>(dispatch.UserData);
}

SwapchainCapture::SwapchainCapture(
    std::shared_ptr<DeviceState> device_state,
    VkSwapchainKHR swapchain,
    VkFormat format,
    VkExtent2D extent,
    std::vector<VkImage> images)
    : device_state_(std::move(device_state)),
      swapchain_(swapchain),
      format_(format),
      extent_(extent),
      images_(std::move(images)) {}

SwapchainCapture::~SwapchainCapture() {
    disable();
    if (socket_thread_.joinable()) {
        socket_thread_.join();
    }
    if (fence_thread_.joinable()) {
        fence_thread_.join();
    }
    const auto* dispatch = device_state_->dispatch;
    const VkDevice device = device_state_->device;
    if (present_semaphore_ != VK_NULL_HANDLE) {
        dispatch->DestroySemaphore(device, present_semaphore_, nullptr);
    }
    if (fence_ != VK_NULL_HANDLE) {
        dispatch->DestroyFence(device, fence_, nullptr);
    }
    if (command_pool_ != VK_NULL_HANDLE) {
        dispatch->DestroyCommandPool(device, command_pool_, nullptr);
    }
    if (export_image_ != VK_NULL_HANDLE) {
        dispatch->DestroyImage(device, export_image_, nullptr);
    }
    if (export_memory_ != VK_NULL_HANDLE) {
        dispatch->FreeMemory(device, export_memory_, nullptr);
    }
    if (export_fd_ >= 0) {
        ::close(export_fd_);
    }
}

bool SwapchainCapture::initialize() {
    if (!initialize_export_image()) {
        phase_.store(CapturePhase::disabled, std::memory_order_release);
        return false;
    }
    socket_thread_ = std::thread([this] { socket_loop(); });
    fence_thread_ = std::thread([this] { fence_loop(); });
    return true;
}

bool SwapchainCapture::initialize_export_image() {
    const auto* device_dispatch = device_state_->dispatch;
    const auto* physical_dispatch = device_dispatch->pPhysicalDeviceDispatch;
    const VkPhysicalDevice physical = device_state_->physical_device;
    const VkDevice device = device_state_->device;

    VkPhysicalDeviceIDProperties ids{
        .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES,
    };
    VkPhysicalDeviceProperties2 properties{
        .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
        .pNext = &ids,
    };
    physical_dispatch->GetPhysicalDeviceProperties2(physical, &properties);
    std::copy(std::begin(ids.deviceUUID), std::end(ids.deviceUUID), device_uuid_.begin());

    VkDrmFormatModifierPropertiesListEXT modifier_properties{
        .sType = VK_STRUCTURE_TYPE_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT,
    };
    VkFormatProperties2 format_properties{
        .sType = VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2,
        .pNext = &modifier_properties,
    };
    physical_dispatch->GetPhysicalDeviceFormatProperties2(
        physical, format_, &format_properties);
    if (modifier_properties.drmFormatModifierCount == 0) {
        return false;
    }
    std::vector<VkDrmFormatModifierPropertiesEXT> available(
        modifier_properties.drmFormatModifierCount);
    modifier_properties.pDrmFormatModifierProperties = available.data();
    physical_dispatch->GetPhysicalDeviceFormatProperties2(
        physical, format_, &format_properties);

    std::vector<uint64_t> modifiers;
    for (const auto& candidate : available) {
        constexpr VkFormatFeatureFlags required =
            VK_FORMAT_FEATURE_TRANSFER_SRC_BIT | VK_FORMAT_FEATURE_TRANSFER_DST_BIT;
        if ((candidate.drmFormatModifierTilingFeatures & required) == required &&
            candidate.drmFormatModifierPlaneCount <= SPVK_MAX_PLANES) {
            modifiers.push_back(candidate.drmFormatModifier);
        }
    }
    if (modifiers.empty()) {
        return false;
    }

    VkImageDrmFormatModifierListCreateInfoEXT modifier_list{
        .sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT,
        .drmFormatModifierCount = static_cast<uint32_t>(modifiers.size()),
        .pDrmFormatModifiers = modifiers.data(),
    };
    VkExternalMemoryImageCreateInfo external_info{
        .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
        .pNext = &modifier_list,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
    };
    VkImageCreateInfo image_info{
        .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
        .pNext = &external_info,
        .imageType = VK_IMAGE_TYPE_2D,
        .format = format_,
        .extent = { extent_.width, extent_.height, 1 },
        .mipLevels = 1,
        .arrayLayers = 1,
        .samples = VK_SAMPLE_COUNT_1_BIT,
        .tiling = VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT,
        .usage = VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT,
        .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
        .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
    };
    if (device_dispatch->CreateImage(
            device, &image_info, nullptr, &export_image_) != VK_SUCCESS) {
        return false;
    }

    VkMemoryRequirements requirements{};
    device_dispatch->GetImageMemoryRequirements(
        device, export_image_, &requirements);
    allocation_size_ = requirements.size;
    VkPhysicalDeviceMemoryProperties memory_properties{};
    physical_dispatch->GetPhysicalDeviceMemoryProperties(
        physical, &memory_properties);
    const uint32_t memory_type = choose_device_local_memory(
        memory_properties, requirements.memoryTypeBits);
    if (memory_type == UINT32_MAX) {
        return false;
    }
    VkExportMemoryAllocateInfo export_info{
        .sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
    };
    VkMemoryDedicatedAllocateInfo dedicated_info{
        .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
        .pNext = &export_info,
        .image = export_image_,
    };
    VkMemoryAllocateInfo allocate_info{
        .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
        .pNext = &dedicated_info,
        .allocationSize = requirements.size,
        .memoryTypeIndex = memory_type,
    };
    if (device_dispatch->AllocateMemory(
            device, &allocate_info, nullptr, &export_memory_) != VK_SUCCESS) {
        return false;
    }
    if (device_dispatch->BindImageMemory(
            device, export_image_, export_memory_, 0) != VK_SUCCESS) {
        return false;
    }
    VkMemoryGetFdInfoKHR fd_info{
        .sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR,
        .memory = export_memory_,
        .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
    };
    if (device_dispatch->GetMemoryFdKHR(
            device, &fd_info, &export_fd_) != VK_SUCCESS) {
        return false;
    }
    VkImageDrmFormatModifierPropertiesEXT chosen{
        .sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT,
    };
    if (device_dispatch->GetImageDrmFormatModifierPropertiesEXT(
            device, export_image_, &chosen) != VK_SUCCESS) {
        return false;
    }
    modifier_ = chosen.drmFormatModifier;
    const auto found = std::ranges::find_if(available, [&](const auto& candidate) {
        return candidate.drmFormatModifier == modifier_;
    });
    if (found == available.end() ||
        found->drmFormatModifierPlaneCount == 0 ||
        found->drmFormatModifierPlaneCount > SPVK_MAX_PLANES) {
        return false;
    }
    plane_count_ = found->drmFormatModifierPlaneCount;
    for (uint32_t index = 0; index != plane_count_; ++index) {
        VkImageSubresource subresource{
            .aspectMask = static_cast<VkImageAspectFlags>(
                VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT_EXT << index),
            .mipLevel = 0,
            .arrayLayer = 0,
        };
        VkSubresourceLayout layout{};
        device_dispatch->GetImageSubresourceLayout(
            device, export_image_, &subresource, &layout);
        plane_layouts_[index] = SpvkPlaneLayout{
            .offset = layout.offset,
            .size = layout.size,
            .row_pitch = layout.rowPitch,
            .array_pitch = layout.arrayPitch,
            .depth_pitch = layout.depthPitch,
        };
    }
    return true;
}

bool SwapchainCapture::initialize_queue(
    const vkroots::VkQueueDispatch& queue_dispatch,
    VkQueue queue) {
    if (command_pool_ != VK_NULL_HANDLE) {
        return true;
    }
    const auto* dispatch = queue_dispatch.pDeviceDispatch;
    for (const auto& queue_info : dispatch->DeviceQueueInfos) {
        for (uint32_t index = 0; index != queue_info.queueCount; ++index) {
            VkQueue candidate = VK_NULL_HANDLE;
            dispatch->GetDeviceQueue(
                dispatch->Device, queue_info.queueFamilyIndex, index, &candidate);
            if (candidate == queue) {
                queue_family_ = queue_info.queueFamilyIndex;
                break;
            }
        }
        if (queue_family_ != UINT32_MAX) {
            break;
        }
    }
    if (queue_family_ == UINT32_MAX) {
        return false;
    }
    VkCommandPoolCreateInfo pool_info{
        .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
        .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
        .queueFamilyIndex = queue_family_,
    };
    if (dispatch->CreateCommandPool(
            dispatch->Device, &pool_info, nullptr, &command_pool_) != VK_SUCCESS) {
        return false;
    }
    VkCommandBufferAllocateInfo allocate_info{
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
        .commandPool = command_pool_,
        .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
        .commandBufferCount = 1,
    };
    if (dispatch->AllocateCommandBuffers(
            dispatch->Device, &allocate_info, &command_buffer_) != VK_SUCCESS) {
        return false;
    }
    VkFenceCreateInfo fence_info{ .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
    if (dispatch->CreateFence(
            dispatch->Device, &fence_info, nullptr, &fence_) != VK_SUCCESS) {
        return false;
    }
    VkSemaphoreCreateInfo semaphore_info{
        .sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,
    };
    if (dispatch->CreateSemaphore(
            dispatch->Device, &semaphore_info, nullptr, &present_semaphore_) != VK_SUCCESS) {
        return false;
    }
    return true;
}

bool SwapchainCapture::capture(
    const vkroots::VkQueueDispatch& queue_dispatch,
    VkQueue queue,
    uint32_t image_index,
    const VkPresentInfoKHR* present_info,
    VkPresentInfoKHR* replacement,
    VkSemaphore* replacement_wait) {
    if (!pending_.exchange(false, std::memory_order_acq_rel)) {
        return false;
    }
    CapturePhase expected = CapturePhase::idle;
    if (!phase_.compare_exchange_strong(
            expected, CapturePhase::recording, std::memory_order_acq_rel)) {
        busy_drops_.fetch_add(1, std::memory_order_relaxed);
        return false;
    }
    const uint64_t sequence = pending_sequence_.load(std::memory_order_acquire);
    const uint64_t request_ns = pending_request_ns_.load(std::memory_order_acquire);
    const uint64_t present_ns = monotonic_ns();
    if (image_index >= images_.size() ||
        present_info->waitSemaphoreCount > MAX_PRESENT_SEMAPHORES ||
        !initialize_queue(queue_dispatch, queue)) {
        phase_.store(CapturePhase::idle, std::memory_order_release);
        record_error(sequence, SPVK_ERROR_QUEUE_UNSUPPORTED);
        return false;
    }

    const auto* dispatch = queue_dispatch.pDeviceDispatch;
    const VkDevice device = dispatch->Device;
    if (dispatch->ResetFences(device, 1, &fence_) != VK_SUCCESS ||
        dispatch->ResetCommandPool(device, command_pool_, 0) != VK_SUCCESS) {
        phase_.store(CapturePhase::idle, std::memory_order_release);
        record_error(sequence, SPVK_ERROR_SUBMIT_FAILED);
        return false;
    }
    VkCommandBufferBeginInfo begin_info{
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
        .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
    };
    if (dispatch->BeginCommandBuffer(command_buffer_, &begin_info) != VK_SUCCESS) {
        phase_.store(CapturePhase::idle, std::memory_order_release);
        record_error(sequence, SPVK_ERROR_SUBMIT_FAILED);
        return false;
    }

    VkImageMemoryBarrier source{
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
        .srcAccessMask = VK_ACCESS_MEMORY_READ_BIT,
        .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
        .oldLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,
        .newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .image = images_[image_index],
        .subresourceRange = {
            .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
            .baseMipLevel = 0,
            .levelCount = 1,
            .baseArrayLayer = 0,
            .layerCount = 1,
        },
    };
    VkImageMemoryBarrier destination{
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
        .srcAccessMask = 0,
        .dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
        .oldLayout = first_export_ ? VK_IMAGE_LAYOUT_UNDEFINED : VK_IMAGE_LAYOUT_GENERAL,
        .newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        .srcQueueFamilyIndex = first_export_ ? VK_QUEUE_FAMILY_IGNORED : VK_QUEUE_FAMILY_EXTERNAL,
        .dstQueueFamilyIndex = first_export_ ? VK_QUEUE_FAMILY_IGNORED : queue_family_,
        .image = export_image_,
        .subresourceRange = {
            .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
            .baseMipLevel = 0,
            .levelCount = 1,
            .baseArrayLayer = 0,
            .layerCount = 1,
        },
    };
    std::array barriers{source, destination};
    dispatch->CmdPipelineBarrier(
        command_buffer_,
        VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
        VK_PIPELINE_STAGE_TRANSFER_BIT,
        0,
        0,
        nullptr,
        0,
        nullptr,
        static_cast<uint32_t>(barriers.size()),
        barriers.data());

    VkImageCopy copy{
        .srcSubresource = {
            .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
            .mipLevel = 0,
            .baseArrayLayer = 0,
            .layerCount = 1,
        },
        .srcOffset = { 0, 0, 0 },
        .dstSubresource = {
            .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
            .mipLevel = 0,
            .baseArrayLayer = 0,
            .layerCount = 1,
        },
        .dstOffset = { 0, 0, 0 },
        .extent = { extent_.width, extent_.height, 1 },
    };
    dispatch->CmdCopyImage(
        command_buffer_,
        images_[image_index],
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        export_image_,
        VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
        1,
        &copy);

    source.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
    source.dstAccessMask = VK_ACCESS_MEMORY_READ_BIT;
    source.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
    source.newLayout = VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
    destination.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT;
    destination.dstAccessMask = 0;
    destination.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL;
    destination.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    destination.srcQueueFamilyIndex = queue_family_;
    destination.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL;
    barriers = {source, destination};
    dispatch->CmdPipelineBarrier(
        command_buffer_,
        VK_PIPELINE_STAGE_TRANSFER_BIT,
        VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT | VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
        0,
        0,
        nullptr,
        0,
        nullptr,
        static_cast<uint32_t>(barriers.size()),
        barriers.data());
    if (dispatch->EndCommandBuffer(command_buffer_) != VK_SUCCESS) {
        phase_.store(CapturePhase::idle, std::memory_order_release);
        record_error(sequence, SPVK_ERROR_SUBMIT_FAILED);
        return false;
    }

    std::array<VkPipelineStageFlags, MAX_PRESENT_SEMAPHORES> wait_stages{};
    std::fill_n(
        wait_stages.begin(),
        present_info->waitSemaphoreCount,
        VK_PIPELINE_STAGE_TRANSFER_BIT);
    VkSubmitInfo submit_info{
        .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
        .waitSemaphoreCount = present_info->waitSemaphoreCount,
        .pWaitSemaphores = present_info->pWaitSemaphores,
        .pWaitDstStageMask = wait_stages.data(),
        .commandBufferCount = 1,
        .pCommandBuffers = &command_buffer_,
        .signalSemaphoreCount = 1,
        .pSignalSemaphores = &present_semaphore_,
    };
    const VkResult result = dispatch->QueueSubmit(queue, 1, &submit_info, fence_);
    const uint64_t submit_done_ns = monotonic_ns();
    if (result != VK_SUCCESS) {
        phase_.store(CapturePhase::idle, std::memory_order_release);
        record_error(sequence, SPVK_ERROR_SUBMIT_FAILED);
        return false;
    }

    first_export_ = false;
    captures_.fetch_add(1, std::memory_order_relaxed);
    submitted_sequence_.store(sequence, std::memory_order_release);
    submitted_request_ns_.store(request_ns, std::memory_order_release);
    submitted_present_ns_.store(present_ns, std::memory_order_release);
    submitted_done_ns_.store(submit_done_ns, std::memory_order_release);
    phase_.store(CapturePhase::submitted, std::memory_order_release);
    phase_.notify_one();

    *replacement = *present_info;
    *replacement_wait = present_semaphore_;
    replacement->waitSemaphoreCount = 1;
    replacement->pWaitSemaphores = replacement_wait;
    return true;
}

void SwapchainCapture::disable() {
    stop_.store(true, std::memory_order_release);
    phase_.store(CapturePhase::disabled, std::memory_order_release);
    phase_.notify_all();
    close_socket();
}

void SwapchainCapture::close_socket() {
    const int fd = socket_fd_.exchange(-1, std::memory_order_acq_rel);
    if (fd >= 0) {
        ::shutdown(fd, SHUT_RDWR);
        ::close(fd);
    }
}

bool SwapchainCapture::send_hello(int socket_fd) {
    SpvkHello hello{
        .magic = SPVK_MAGIC,
        .version = SPVK_VERSION,
        .type = SPVK_MESSAGE_HELLO,
        .size = sizeof(SpvkHello),
        .width = extent_.width,
        .height = extent_.height,
        .vk_format = static_cast<uint32_t>(format_),
        .drm_fourcc = drm_format(format_),
        .plane_count = plane_count_,
        .modifier = modifier_,
        .allocation_size = allocation_size_,
    };
    std::copy(device_uuid_.begin(), device_uuid_.end(), std::begin(hello.device_uuid));
    std::copy_n(plane_layouts_.begin(), plane_count_, std::begin(hello.planes));

    std::array<uint8_t, CMSG_SPACE(sizeof(int))> control{};
    iovec vector{ .iov_base = &hello, .iov_len = sizeof(hello) };
    msghdr message{};
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    message.msg_control = control.data();
    message.msg_controllen = control.size();
    cmsghdr* header = CMSG_FIRSTHDR(&message);
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(int));
    std::memcpy(CMSG_DATA(header), &export_fd_, sizeof(int));
    return ::sendmsg(socket_fd, &message, MSG_NOSIGNAL) ==
        static_cast<ssize_t>(sizeof(hello));
}

bool SwapchainCapture::send_packet(const SpvkPacket& packet) {
    std::lock_guard lock(send_mutex_);
    const int fd = socket_fd_.load(std::memory_order_acquire);
    return fd >= 0 && ::send(fd, &packet, sizeof(packet), MSG_NOSIGNAL) ==
        static_cast<ssize_t>(sizeof(packet));
}

void SwapchainCapture::record_error(uint64_t sequence, SpvkErrorType error_type) {
    SpvkPacket packet{
        .magic = SPVK_MAGIC,
        .version = SPVK_VERSION,
        .type = SPVK_MESSAGE_ERROR,
        .run_id = run_id_.load(std::memory_order_acquire),
        .sequence = sequence,
        .requests = requests_.load(std::memory_order_relaxed),
        .captures = captures_.load(std::memory_order_relaxed),
        .busy_drops = busy_drops_.load(std::memory_order_relaxed),
        .coalesced_drops = coalesced_drops_.load(std::memory_order_relaxed),
        .status = error_type,
    };
    send_packet(packet);
}

void SwapchainCapture::socket_loop() {
    while (!stop_.load(std::memory_order_acquire)) {
        const int fd = ::socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0);
        if (fd < 0) {
            std::this_thread::sleep_for(std::chrono::milliseconds(250));
            continue;
        }
        sockaddr_un address{};
        address.sun_family = AF_UNIX;
        if (device_state_->socket_path.size() >= sizeof(address.sun_path)) {
            ::close(fd);
            phase_.store(CapturePhase::disabled, std::memory_order_release);
            return;
        }
        std::strcpy(address.sun_path, device_state_->socket_path.c_str());
        if (::connect(fd, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) != 0) {
            ::close(fd);
            std::this_thread::sleep_for(std::chrono::milliseconds(250));
            continue;
        }
        socket_fd_.store(fd, std::memory_order_release);
        if (!send_hello(fd)) {
            close_socket();
            continue;
        }
        SpvkPacket acknowledgement{};
        const ssize_t received = ::recv(fd, &acknowledgement, sizeof(acknowledgement), 0);
        if (received != static_cast<ssize_t>(sizeof(acknowledgement)) ||
            acknowledgement.magic != SPVK_MAGIC ||
            acknowledgement.version != SPVK_VERSION ||
            acknowledgement.type != SPVK_MESSAGE_HELLO_ACK) {
            close_socket();
            continue;
        }
        run_id_.store(acknowledgement.run_id, std::memory_order_release);
        phase_.store(CapturePhase::idle, std::memory_order_release);

        bool retry = true;
        while (!stop_.load(std::memory_order_acquire)) {
            SpvkPacket packet{};
            const ssize_t packet_size = ::recv(fd, &packet, sizeof(packet), 0);
            if (packet_size != static_cast<ssize_t>(sizeof(packet)) ||
                packet.magic != SPVK_MAGIC || packet.version != SPVK_VERSION ||
                packet.run_id != run_id_.load(std::memory_order_acquire)) {
                retry = captures_.load(std::memory_order_acquire) == 0;
                break;
            }
            if (packet.type == SPVK_MESSAGE_REQUEST) {
                requests_.fetch_add(1, std::memory_order_relaxed);
                if (phase_.load(std::memory_order_acquire) != CapturePhase::idle) {
                    busy_drops_.fetch_add(1, std::memory_order_relaxed);
                    continue;
                }
                if (pending_.load(std::memory_order_acquire)) {
                    coalesced_drops_.fetch_add(1, std::memory_order_relaxed);
                    continue;
                }
                pending_sequence_.store(packet.sequence, std::memory_order_relaxed);
                pending_request_ns_.store(packet.request_ns, std::memory_order_relaxed);
                bool expected = false;
                if (!pending_.compare_exchange_strong(
                        expected, true, std::memory_order_release,
                        std::memory_order_relaxed)) {
                    coalesced_drops_.fetch_add(1, std::memory_order_relaxed);
                }
            } else if (packet.type == SPVK_MESSAGE_ACK) {
                if (phase_.load(std::memory_order_acquire) == CapturePhase::awaiting_ack &&
                    packet.sequence == submitted_sequence_.load(std::memory_order_acquire)) {
                    phase_.store(CapturePhase::idle, std::memory_order_release);
                }
            }
        }
        close_socket();
        pending_.store(false, std::memory_order_release);
        if (!retry || stop_.load(std::memory_order_acquire)) {
            phase_.store(CapturePhase::disabled, std::memory_order_release);
            phase_.notify_all();
            return;
        }
        phase_.store(CapturePhase::disconnected, std::memory_order_release);
    }
}

void SwapchainCapture::fence_loop() {
    while (!stop_.load(std::memory_order_acquire)) {
        CapturePhase phase = phase_.load(std::memory_order_acquire);
        if (phase == CapturePhase::disabled) {
            return;
        }
        if (phase != CapturePhase::submitted) {
            phase_.wait(phase, std::memory_order_acquire);
            continue;
        }
        const auto* dispatch = device_state_->dispatch;
        const VkResult result = dispatch->WaitForFences(
            device_state_->device, 1, &fence_, VK_TRUE, UINT64_MAX);
        const uint64_t fence_ns = monotonic_ns();
        if (result != VK_SUCCESS) {
            record_error(
                submitted_sequence_.load(std::memory_order_acquire),
                SPVK_ERROR_FENCE_FAILED);
            phase_.store(CapturePhase::disabled, std::memory_order_release);
            return;
        }
        CapturePhase expected = CapturePhase::submitted;
        if (!phase_.compare_exchange_strong(
                expected, CapturePhase::awaiting_ack, std::memory_order_acq_rel)) {
            continue;
        }
        SpvkPacket packet{
            .magic = SPVK_MAGIC,
            .version = SPVK_VERSION,
            .type = SPVK_MESSAGE_READY,
            .run_id = run_id_.load(std::memory_order_acquire),
            .sequence = submitted_sequence_.load(std::memory_order_acquire),
            .request_ns = submitted_request_ns_.load(std::memory_order_acquire),
            .present_ns = submitted_present_ns_.load(std::memory_order_acquire),
            .submit_done_ns = submitted_done_ns_.load(std::memory_order_acquire),
            .fence_done_ns = fence_ns,
            .requests = requests_.load(std::memory_order_relaxed),
            .captures = captures_.load(std::memory_order_relaxed),
            .busy_drops = busy_drops_.load(std::memory_order_relaxed),
            .coalesced_drops = coalesced_drops_.load(std::memory_order_relaxed),
            .status = SPVK_ERROR_NONE,
        };
        if (!send_packet(packet)) {
            phase_.store(CapturePhase::disabled, std::memory_order_release);
            close_socket();
            return;
        }
    }
}

class InstanceOverrides {
public:
    static VkResult CreateDevice(
        const vkroots::VkPhysicalDeviceDispatch& dispatch,
        VkPhysicalDevice physical_device,
        const VkDeviceCreateInfo* create_info,
        const VkAllocationCallbacks* allocator,
        VkDevice* device) {
        const char* socket_path_value = std::getenv("SCOREPEEK_VK_CAPTURE_SOCKET");
        const std::string socket_path = socket_path_value == nullptr
            ? std::string{}
            : std::string{socket_path_value};
        constexpr std::array required_extensions{
            VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME,
            VK_EXT_EXTERNAL_MEMORY_DMA_BUF_EXTENSION_NAME,
            VK_EXT_IMAGE_DRM_FORMAT_MODIFIER_EXTENSION_NAME,
        };
        VkPhysicalDeviceProperties properties{};
        dispatch.GetPhysicalDeviceProperties(physical_device, &properties);
        const bool enabled = !socket_path.empty() &&
            properties.apiVersion >= VK_API_VERSION_1_1 &&
            physical_device_has_extensions(dispatch, physical_device, required_extensions);

        std::vector<const char*> extensions;
        if (create_info->enabledExtensionCount != 0) {
            extensions.assign(create_info->ppEnabledExtensionNames,
                              create_info->ppEnabledExtensionNames +
                                  create_info->enabledExtensionCount);
        }
        if (enabled) {
            for (const char* required : required_extensions) {
                if (!has_name(extensions, required)) {
                    extensions.push_back(required);
                }
            }
        }
        VkDeviceCreateInfo replacement = *create_info;
        replacement.enabledExtensionCount = static_cast<uint32_t>(extensions.size());
        replacement.ppEnabledExtensionNames = extensions.data();
        const VkResult result = dispatch.CreateDevice(
            physical_device, &replacement, allocator, device);
        if (result != VK_SUCCESS) {
            return result;
        }
        const auto* device_dispatch = vkroots::LookupDispatch(*device);
        device_dispatch->UserData.emplace<std::shared_ptr<DeviceState>>(
            std::make_shared<DeviceState>(
                device_dispatch,
                *device,
                physical_device,
                socket_path,
                enabled));
        return result;
    }
};

class DeviceOverrides {
public:
    static VkResult CreateSwapchainKHR(
        const vkroots::VkDeviceDispatch& dispatch,
        VkDevice device,
        const VkSwapchainCreateInfoKHR* create_info,
        const VkAllocationCallbacks* allocator,
        VkSwapchainKHR* swapchain) {
        const auto state = get_device_state(dispatch);
        VkSwapchainCreateInfoKHR replacement = *create_info;
        if (state && state->enabled && supported_format(create_info->imageFormat)) {
            replacement.imageUsage |= VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        }
        const VkResult result = dispatch.CreateSwapchainKHR(
            device, &replacement, allocator, swapchain);
        if (result != VK_SUCCESS || !state || !state->enabled ||
            !supported_format(create_info->imageFormat)) {
            return result;
        }
        uint32_t image_count = 0;
        if (dispatch.GetSwapchainImagesKHR(
                device, *swapchain, &image_count, nullptr) != VK_SUCCESS ||
            image_count == 0) {
            return result;
        }
        std::vector<VkImage> images(image_count);
        if (dispatch.GetSwapchainImagesKHR(
                device, *swapchain, &image_count, images.data()) != VK_SUCCESS) {
            return result;
        }
        auto capture = std::make_unique<SwapchainCapture>(
            state, *swapchain, create_info->imageFormat, create_info->imageExtent,
            std::move(images));
        if (!capture->initialize()) {
            return result;
        }
        SwapchainCapture* pointer = capture.get();
        {
            std::lock_guard lock(state->captures_mutex);
            state->captures.push_back(std::move(capture));
        }
        SwapchainCapture* previous =
            state->active.exchange(pointer, std::memory_order_acq_rel);
        if (previous != nullptr) {
            previous->disable();
        }
        return result;
    }

    static void DestroySwapchainKHR(
        const vkroots::VkDeviceDispatch& dispatch,
        VkDevice device,
        VkSwapchainKHR swapchain,
        const VkAllocationCallbacks* allocator) {
        const auto state = get_device_state(dispatch);
        if (state) {
            SwapchainCapture* active = state->active.load(std::memory_order_acquire);
            if (active != nullptr && active->swapchain() == swapchain) {
                state->active.store(nullptr, std::memory_order_release);
                active->disable();
            }
            std::lock_guard lock(state->captures_mutex);
            std::erase_if(state->captures, [swapchain](const auto& capture) {
                return capture->swapchain() == swapchain;
            });
        }
        dispatch.DestroySwapchainKHR(device, swapchain, allocator);
    }

    static VkResult QueuePresentKHR(
        const vkroots::VkQueueDispatch& dispatch,
        VkQueue queue,
        const VkPresentInfoKHR* present_info) {
        const auto state = get_device_state(*dispatch.pDeviceDispatch);
        SwapchainCapture* active = state
            ? state->active.load(std::memory_order_acquire)
            : nullptr;
        if (active == nullptr) {
            return dispatch.QueuePresentKHR(queue, present_info);
        }
        for (uint32_t index = 0; index != present_info->swapchainCount; ++index) {
            if (present_info->pSwapchains[index] != active->swapchain()) {
                continue;
            }
            VkPresentInfoKHR replacement{};
            VkSemaphore replacement_wait = VK_NULL_HANDLE;
            if (active->capture(
                    dispatch,
                    queue,
                    present_info->pImageIndices[index],
                    present_info,
                    &replacement,
                    &replacement_wait)) {
                return dispatch.QueuePresentKHR(queue, &replacement);
            }
            break;
        }
        return dispatch.QueuePresentKHR(queue, present_info);
    }

    static void DestroyDevice(
        const vkroots::VkDeviceDispatch& dispatch,
        VkDevice device,
        const VkAllocationCallbacks* allocator) {
        const auto state = get_device_state(dispatch);
        if (state) {
            state->active.store(nullptr, std::memory_order_release);
            std::lock_guard lock(state->captures_mutex);
            for (const auto& capture : state->captures) {
                capture->disable();
            }
            state->captures.clear();
        }
        dispatch.UserData.destroy();
        dispatch.DestroyDevice(device, allocator);
    }
};

} // namespace scorepeek_capture

VKROOTS_DEFINE_LAYER_INTERFACES(
    scorepeek_capture::InstanceOverrides,
    scorepeek_capture::DeviceOverrides)
