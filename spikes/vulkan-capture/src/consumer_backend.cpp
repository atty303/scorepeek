#include "consumer_backend.h"

#include <vulkan/vulkan.h>

#include <algorithm>
#include <array>
#include <bit>
#include <cerrno>
#include <chrono>
#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <ctime>
#include <dlfcn.h>
#include <fcntl.h>
#include <filesystem>
#include <memory>
#include <ranges>
#include <poll.h>
#include <string>
#include <sys/socket.h>
#include <sys/eventfd.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/un.h>
#include <unistd.h>
#include <vector>

namespace {

void set_error(char* output, size_t size, const char* format, ...) {
    if (output == nullptr || size == 0) {
        return;
    }
    va_list args;
    va_start(args, format);
    std::vsnprintf(output, size, format, args);
    va_end(args);
}

bool valid_header(uint32_t magic, uint16_t version, uint16_t type) {
    return magic == SPVK_MAGIC && version == SPVK_VERSION && type != 0;
}

bool write_all(int fd, const uint8_t* bytes, size_t count) {
    while (count != 0) {
        const ssize_t written = ::write(fd, bytes, count);
        if (written < 0) {
            if (errno == EINTR) {
                continue;
            }
            return false;
        }
        bytes += static_cast<size_t>(written);
        count -= static_cast<size_t>(written);
    }
    return true;
}

bool has_extension(
    const std::vector<VkExtensionProperties>& extensions,
    const char* required) {
    return std::ranges::any_of(extensions, [required](const auto& extension) {
        return std::strcmp(extension.extensionName, required) == 0;
    });
}

struct VulkanFunctions {
    void* library = nullptr;
    PFN_vkGetInstanceProcAddr get_instance_proc_addr = nullptr;
    PFN_vkCreateInstance create_instance = nullptr;
    PFN_vkDestroyInstance destroy_instance = nullptr;
    PFN_vkEnumeratePhysicalDevices enumerate_physical_devices = nullptr;
    PFN_vkGetPhysicalDeviceProperties2 get_physical_device_properties2 = nullptr;
    PFN_vkGetPhysicalDeviceQueueFamilyProperties get_queue_family_properties = nullptr;
    PFN_vkGetPhysicalDeviceMemoryProperties get_memory_properties = nullptr;
    PFN_vkEnumerateDeviceExtensionProperties enumerate_device_extensions = nullptr;
    PFN_vkCreateDevice create_device = nullptr;
    PFN_vkGetDeviceProcAddr get_device_proc_addr = nullptr;
    PFN_vkDestroyDevice destroy_device = nullptr;
    PFN_vkGetDeviceQueue get_device_queue = nullptr;
    PFN_vkCreateImage create_image = nullptr;
    PFN_vkDestroyImage destroy_image = nullptr;
    PFN_vkGetImageMemoryRequirements get_image_memory_requirements = nullptr;
    PFN_vkGetMemoryFdPropertiesKHR get_memory_fd_properties = nullptr;
    PFN_vkAllocateMemory allocate_memory = nullptr;
    PFN_vkFreeMemory free_memory = nullptr;
    PFN_vkBindImageMemory bind_image_memory = nullptr;
    PFN_vkCreateBuffer create_buffer = nullptr;
    PFN_vkDestroyBuffer destroy_buffer = nullptr;
    PFN_vkGetBufferMemoryRequirements get_buffer_memory_requirements = nullptr;
    PFN_vkBindBufferMemory bind_buffer_memory = nullptr;
    PFN_vkMapMemory map_memory = nullptr;
    PFN_vkUnmapMemory unmap_memory = nullptr;
    PFN_vkInvalidateMappedMemoryRanges invalidate_mapped_ranges = nullptr;
    PFN_vkCreateCommandPool create_command_pool = nullptr;
    PFN_vkDestroyCommandPool destroy_command_pool = nullptr;
    PFN_vkResetCommandPool reset_command_pool = nullptr;
    PFN_vkAllocateCommandBuffers allocate_command_buffers = nullptr;
    PFN_vkBeginCommandBuffer begin_command_buffer = nullptr;
    PFN_vkEndCommandBuffer end_command_buffer = nullptr;
    PFN_vkCmdPipelineBarrier cmd_pipeline_barrier = nullptr;
    PFN_vkCmdCopyImageToBuffer cmd_copy_image_to_buffer = nullptr;
    PFN_vkCreateFence create_fence = nullptr;
    PFN_vkDestroyFence destroy_fence = nullptr;
    PFN_vkResetFences reset_fences = nullptr;
    PFN_vkWaitForFences wait_for_fences = nullptr;
    PFN_vkQueueSubmit queue_submit = nullptr;

    bool load_global(char* error, size_t error_size) {
        library = ::dlopen("libvulkan.so.1", RTLD_NOW | RTLD_LOCAL);
        if (library == nullptr) {
            set_error(error, error_size, "dlopen(libvulkan.so.1): %s", ::dlerror());
            return false;
        }
        get_instance_proc_addr = reinterpret_cast<PFN_vkGetInstanceProcAddr>(
            ::dlsym(library, "vkGetInstanceProcAddr"));
        if (get_instance_proc_addr == nullptr) {
            set_error(error, error_size, "vkGetInstanceProcAddr is unavailable");
            return false;
        }
        create_instance = reinterpret_cast<PFN_vkCreateInstance>(
            get_instance_proc_addr(VK_NULL_HANDLE, "vkCreateInstance"));
        if (create_instance == nullptr) {
            set_error(error, error_size, "vkCreateInstance is unavailable");
            return false;
        }
        return true;
    }

    bool load_instance(VkInstance instance, char* error, size_t error_size) {
#define LOAD_INSTANCE(member, name)                                                        \
    member = reinterpret_cast<PFN_vk##name>(get_instance_proc_addr(instance, "vk" #name)); \
    if (member == nullptr) {                                                               \
        set_error(error, error_size, "vk" #name " is unavailable");                       \
        return false;                                                                      \
    }
        LOAD_INSTANCE(destroy_instance, DestroyInstance)
        LOAD_INSTANCE(enumerate_physical_devices, EnumeratePhysicalDevices)
        LOAD_INSTANCE(get_physical_device_properties2, GetPhysicalDeviceProperties2)
        LOAD_INSTANCE(get_queue_family_properties, GetPhysicalDeviceQueueFamilyProperties)
        LOAD_INSTANCE(get_memory_properties, GetPhysicalDeviceMemoryProperties)
        LOAD_INSTANCE(enumerate_device_extensions, EnumerateDeviceExtensionProperties)
        LOAD_INSTANCE(create_device, CreateDevice)
        LOAD_INSTANCE(get_device_proc_addr, GetDeviceProcAddr)
#undef LOAD_INSTANCE
        return true;
    }

    bool load_device(VkDevice device, char* error, size_t error_size) {
#define LOAD_DEVICE(member, name)                                                       \
    member = reinterpret_cast<PFN_vk##name>(get_device_proc_addr(device, "vk" #name)); \
    if (member == nullptr) {                                                            \
        set_error(error, error_size, "vk" #name " is unavailable");                    \
        return false;                                                                   \
    }
        LOAD_DEVICE(destroy_device, DestroyDevice)
        LOAD_DEVICE(get_device_queue, GetDeviceQueue)
        LOAD_DEVICE(create_image, CreateImage)
        LOAD_DEVICE(destroy_image, DestroyImage)
        LOAD_DEVICE(get_image_memory_requirements, GetImageMemoryRequirements)
        LOAD_DEVICE(get_memory_fd_properties, GetMemoryFdPropertiesKHR)
        LOAD_DEVICE(allocate_memory, AllocateMemory)
        LOAD_DEVICE(free_memory, FreeMemory)
        LOAD_DEVICE(bind_image_memory, BindImageMemory)
        LOAD_DEVICE(create_buffer, CreateBuffer)
        LOAD_DEVICE(destroy_buffer, DestroyBuffer)
        LOAD_DEVICE(get_buffer_memory_requirements, GetBufferMemoryRequirements)
        LOAD_DEVICE(bind_buffer_memory, BindBufferMemory)
        LOAD_DEVICE(map_memory, MapMemory)
        LOAD_DEVICE(unmap_memory, UnmapMemory)
        LOAD_DEVICE(invalidate_mapped_ranges, InvalidateMappedMemoryRanges)
        LOAD_DEVICE(create_command_pool, CreateCommandPool)
        LOAD_DEVICE(destroy_command_pool, DestroyCommandPool)
        LOAD_DEVICE(reset_command_pool, ResetCommandPool)
        LOAD_DEVICE(allocate_command_buffers, AllocateCommandBuffers)
        LOAD_DEVICE(begin_command_buffer, BeginCommandBuffer)
        LOAD_DEVICE(end_command_buffer, EndCommandBuffer)
        LOAD_DEVICE(cmd_pipeline_barrier, CmdPipelineBarrier)
        LOAD_DEVICE(cmd_copy_image_to_buffer, CmdCopyImageToBuffer)
        LOAD_DEVICE(create_fence, CreateFence)
        LOAD_DEVICE(destroy_fence, DestroyFence)
        LOAD_DEVICE(reset_fences, ResetFences)
        LOAD_DEVICE(wait_for_fences, WaitForFences)
        LOAD_DEVICE(queue_submit, QueueSubmit)
#undef LOAD_DEVICE
        return true;
    }

    ~VulkanFunctions() {
        if (library != nullptr) {
            ::dlclose(library);
        }
    }
};

uint32_t choose_memory_type(
    const VkPhysicalDeviceMemoryProperties& properties,
    uint32_t allowed,
    VkMemoryPropertyFlags required,
    VkMemoryPropertyFlags preferred,
    bool* coherent) {
    for (uint32_t pass = 0; pass != 2; ++pass) {
        for (uint32_t index = 0; index != properties.memoryTypeCount; ++index) {
            if ((allowed & (1u << index)) == 0) {
                continue;
            }
            const auto flags = properties.memoryTypes[index].propertyFlags;
            if ((flags & required) != required) {
                continue;
            }
            if (pass == 0 && (flags & preferred) != preferred) {
                continue;
            }
            if (coherent != nullptr) {
                *coherent = (flags & VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) != 0;
            }
            return index;
        }
    }
    return UINT32_MAX;
}

} // namespace

struct SpvkConsumerBackend {
    VulkanFunctions functions;
    SpvkHello hello{};
    VkInstance instance = VK_NULL_HANDLE;
    VkPhysicalDevice physical_device = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE;
    uint32_t queue_family = 0;
    VkQueue queue = VK_NULL_HANDLE;
    VkImage image = VK_NULL_HANDLE;
    VkDeviceMemory image_memory = VK_NULL_HANDLE;
    VkBuffer staging_buffer = VK_NULL_HANDLE;
    VkDeviceMemory staging_memory = VK_NULL_HANDLE;
    uint8_t* mapped = nullptr;
    size_t mapped_size = 0;
    bool staging_coherent = false;
    VkCommandPool command_pool = VK_NULL_HANDLE;
    VkCommandBuffer command_buffer = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;

    ~SpvkConsumerBackend() {
        if (device != VK_NULL_HANDLE) {
            if (mapped != nullptr) {
                functions.unmap_memory(device, staging_memory);
            }
            if (fence != VK_NULL_HANDLE) {
                functions.destroy_fence(device, fence, nullptr);
            }
            if (command_pool != VK_NULL_HANDLE) {
                functions.destroy_command_pool(device, command_pool, nullptr);
            }
            if (staging_buffer != VK_NULL_HANDLE) {
                functions.destroy_buffer(device, staging_buffer, nullptr);
            }
            if (staging_memory != VK_NULL_HANDLE) {
                functions.free_memory(device, staging_memory, nullptr);
            }
            if (image != VK_NULL_HANDLE) {
                functions.destroy_image(device, image, nullptr);
            }
            if (image_memory != VK_NULL_HANDLE) {
                functions.free_memory(device, image_memory, nullptr);
            }
            functions.destroy_device(device, nullptr);
        }
        if (instance != VK_NULL_HANDLE) {
            functions.destroy_instance(instance, nullptr);
        }
    }
};

extern "C" uint64_t spvk_monotonic_ns(void) {
    timespec value{};
    ::clock_gettime(CLOCK_MONOTONIC, &value);
    return static_cast<uint64_t>(value.tv_sec) * 1'000'000'000ull +
        static_cast<uint64_t>(value.tv_nsec);
}

extern "C" uint64_t spvk_process_id(void) {
    return static_cast<uint64_t>(::getpid());
}

extern "C" const char* spvk_getenv(const char* name) {
    return ::getenv(name);
}

extern "C" int spvk_make_parent_directories(
    const char* path,
    char* error,
    size_t error_size) {
    std::error_code code;
    const std::filesystem::path target(path);
    const auto parent = target.parent_path();
    if (parent.empty()) {
        return 0;
    }
    std::filesystem::create_directories(parent, code);
    if (code) {
        set_error(error, error_size, "create_directories(%s): %s", parent.c_str(), code.message().c_str());
        return -1;
    }
    return 0;
}

extern "C" int spvk_listen(const char* path, char* error, size_t error_size) {
    if (std::strlen(path) >= sizeof(sockaddr_un::sun_path)) {
        set_error(error, error_size, "socket path is too long");
        return -1;
    }
    if (spvk_make_parent_directories(path, error, error_size) != 0) {
        return -1;
    }
    ::unlink(path);
    const int fd = ::socket(AF_UNIX, SOCK_SEQPACKET | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        set_error(error, error_size, "socket: %s", std::strerror(errno));
        return -1;
    }
    sockaddr_un address{};
    address.sun_family = AF_UNIX;
    std::strcpy(address.sun_path, path);
    if (::bind(fd, reinterpret_cast<const sockaddr*>(&address), sizeof(address)) != 0) {
        set_error(error, error_size, "bind(%s): %s", path, std::strerror(errno));
        ::close(fd);
        return -1;
    }
    ::chmod(path, S_IRUSR | S_IWUSR);
    if (::listen(fd, 1) != 0) {
        set_error(error, error_size, "listen: %s", std::strerror(errno));
        ::close(fd);
        ::unlink(path);
        return -1;
    }
    return fd;
}

extern "C" int spvk_accept(int listener, char* error, size_t error_size) {
    int fd;
    do {
        fd = ::accept4(listener, nullptr, nullptr, SOCK_CLOEXEC);
    } while (fd < 0 && errno == EINTR);
    if (fd < 0) {
        set_error(error, error_size, "accept: %s", std::strerror(errno));
    }
    return fd;
}

extern "C" int spvk_receive_hello(
    int socket_fd,
    SpvkHello* hello,
    int* dma_buf_fd,
    char* error,
    size_t error_size) {
    std::array<uint8_t, CMSG_SPACE(sizeof(int))> control{};
    iovec vector{ .iov_base = hello, .iov_len = sizeof(*hello) };
    msghdr message{};
    message.msg_iov = &vector;
    message.msg_iovlen = 1;
    message.msg_control = control.data();
    message.msg_controllen = control.size();
    ssize_t received;
    do {
        received = ::recvmsg(socket_fd, &message, MSG_CMSG_CLOEXEC);
    } while (received < 0 && errno == EINTR);
    if (received != static_cast<ssize_t>(sizeof(*hello))) {
        set_error(error, error_size, "invalid hello size: %zd", received);
        return -1;
    }
    if (!valid_header(hello->magic, hello->version, hello->type) ||
        hello->type != SPVK_MESSAGE_HELLO || hello->size != sizeof(*hello) ||
        hello->plane_count == 0 || hello->plane_count > SPVK_MAX_PLANES) {
        set_error(error, error_size, "invalid hello contract");
        return -1;
    }
    *dma_buf_fd = -1;
    for (cmsghdr* header = CMSG_FIRSTHDR(&message); header != nullptr;
         header = CMSG_NXTHDR(&message, header)) {
        if (header->cmsg_level == SOL_SOCKET && header->cmsg_type == SCM_RIGHTS &&
            header->cmsg_len >= CMSG_LEN(sizeof(int))) {
            std::memcpy(dma_buf_fd, CMSG_DATA(header), sizeof(int));
            break;
        }
    }
    if (*dma_buf_fd < 0) {
        set_error(error, error_size, "hello did not contain a DMA-BUF fd");
        return -1;
    }
    return 0;
}

extern "C" int spvk_send_packet(
    int socket_fd,
    const SpvkPacket* packet,
    char* error,
    size_t error_size) {
    const ssize_t sent = ::send(socket_fd, packet, sizeof(*packet), MSG_NOSIGNAL);
    if (sent != static_cast<ssize_t>(sizeof(*packet))) {
        set_error(error, error_size, "send packet: %s", sent < 0 ? std::strerror(errno) : "short write");
        return -1;
    }
    return 0;
}

extern "C" int spvk_receive_packet(
    int socket_fd,
    int wake_fd,
    SpvkPacket* packet,
    int timeout_ms,
    char* error,
    size_t error_size) {
    std::array<pollfd, 2> descriptors{
        pollfd{ .fd = socket_fd, .events = POLLIN, .revents = 0 },
        pollfd{ .fd = wake_fd, .events = POLLIN, .revents = 0 },
    };
    int ready;
    do {
        ready = ::poll(descriptors.data(), descriptors.size(), timeout_ms);
    } while (ready < 0 && errno == EINTR);
    if (ready == 0) {
        return 0;
    }
    if (ready < 0) {
        set_error(error, error_size, "poll: %s", std::strerror(errno));
        return -1;
    }
    if ((descriptors[1].revents & POLLIN) != 0) {
        return 2;
    }
    const ssize_t received = ::recv(socket_fd, packet, sizeof(*packet), 0);
    if (received == 0) {
        set_error(error, error_size, "producer disconnected");
        return -1;
    }
    if (received != static_cast<ssize_t>(sizeof(*packet)) ||
        !valid_header(packet->magic, packet->version, packet->type)) {
        set_error(error, error_size, "invalid packet");
        return -1;
    }
    return 1;
}

extern "C" int spvk_event_create(char* error, size_t error_size) {
    const int fd = ::eventfd(0, EFD_CLOEXEC);
    if (fd < 0) {
        set_error(error, error_size, "eventfd: %s", std::strerror(errno));
    }
    return fd;
}

extern "C" int spvk_event_signal(int event_fd, char* error, size_t error_size) {
    const uint64_t value = 1;
    if (::write(event_fd, &value, sizeof(value)) != static_cast<ssize_t>(sizeof(value))) {
        set_error(error, error_size, "eventfd write: %s", std::strerror(errno));
        return -1;
    }
    return 0;
}

extern "C" int spvk_event_consume(int event_fd, char* error, size_t error_size) {
    uint64_t value = 0;
    if (::read(event_fd, &value, sizeof(value)) != static_cast<ssize_t>(sizeof(value))) {
        set_error(error, error_size, "eventfd read: %s", std::strerror(errno));
        return -1;
    }
    return 0;
}

extern "C" void spvk_close(int fd) {
    if (fd >= 0) {
        ::close(fd);
    }
}

extern "C" void spvk_unlink(const char* path) {
    if (path != nullptr) {
        ::unlink(path);
    }
}

extern "C" SpvkConsumerBackend* spvk_consumer_create(
    const SpvkHello* hello,
    int dma_buf_fd,
    char* error,
    size_t error_size) {
    bool fd_owned = true;
    auto backend = std::make_unique<SpvkConsumerBackend>();
    backend->hello = *hello;
    auto fail = [&](const char* text, VkResult result = VK_SUCCESS) -> SpvkConsumerBackend* {
        if (result == VK_SUCCESS) {
            set_error(error, error_size, "%s", text);
        } else {
            set_error(error, error_size, "%s: VkResult %d", text, result);
        }
        if (fd_owned) {
            ::close(dma_buf_fd);
        }
        return nullptr;
    };

    if (hello->width == 0 || hello->height == 0 ||
        (hello->vk_format != VK_FORMAT_B8G8R8A8_UNORM &&
         hello->vk_format != VK_FORMAT_B8G8R8A8_SRGB &&
         hello->vk_format != VK_FORMAT_R8G8B8A8_UNORM &&
         hello->vk_format != VK_FORMAT_R8G8B8A8_SRGB)) {
        return fail("unsupported image contract");
    }
    if (!backend->functions.load_global(error, error_size)) {
        if (fd_owned) {
            ::close(dma_buf_fd);
        }
        return nullptr;
    }

    VkApplicationInfo application{
        .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
        .pApplicationName = "scorepeek-vulkan-capture-consumer",
        .applicationVersion = 1,
        .pEngineName = "none",
        .engineVersion = 0,
        .apiVersion = VK_API_VERSION_1_1,
    };
    VkInstanceCreateInfo instance_info{
        .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
        .pApplicationInfo = &application,
    };
    VkResult result = backend->functions.create_instance(&instance_info, nullptr, &backend->instance);
    if (result != VK_SUCCESS) {
        return fail("vkCreateInstance", result);
    }
    if (!backend->functions.load_instance(backend->instance, error, error_size)) {
        if (fd_owned) {
            ::close(dma_buf_fd);
        }
        return nullptr;
    }

    uint32_t physical_count = 0;
    result = backend->functions.enumerate_physical_devices(
        backend->instance, &physical_count, nullptr);
    if (result != VK_SUCCESS || physical_count == 0) {
        return fail("vkEnumeratePhysicalDevices", result);
    }
    std::vector<VkPhysicalDevice> physical_devices(physical_count);
    result = backend->functions.enumerate_physical_devices(
        backend->instance, &physical_count, physical_devices.data());
    if (result != VK_SUCCESS) {
        return fail("vkEnumeratePhysicalDevices", result);
    }
    for (const auto physical : physical_devices) {
        VkPhysicalDeviceIDProperties ids{ .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ID_PROPERTIES };
        VkPhysicalDeviceProperties2 properties{
            .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
            .pNext = &ids,
        };
        backend->functions.get_physical_device_properties2(physical, &properties);
        if (std::memcmp(ids.deviceUUID, hello->device_uuid, VK_UUID_SIZE) == 0) {
            backend->physical_device = physical;
            break;
        }
    }
    if (backend->physical_device == VK_NULL_HANDLE) {
        return fail("producer Vulkan device UUID is unavailable in consumer");
    }

    uint32_t extension_count = 0;
    result = backend->functions.enumerate_device_extensions(
        backend->physical_device, nullptr, &extension_count, nullptr);
    if (result != VK_SUCCESS) {
        return fail("vkEnumerateDeviceExtensionProperties", result);
    }
    std::vector<VkExtensionProperties> extensions(extension_count);
    result = backend->functions.enumerate_device_extensions(
        backend->physical_device, nullptr, &extension_count, extensions.data());
    if (result != VK_SUCCESS) {
        return fail("vkEnumerateDeviceExtensionProperties", result);
    }
    constexpr std::array required_extensions{
        VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME,
        VK_EXT_EXTERNAL_MEMORY_DMA_BUF_EXTENSION_NAME,
        VK_EXT_IMAGE_DRM_FORMAT_MODIFIER_EXTENSION_NAME,
    };
    for (const auto* required : required_extensions) {
        if (!has_extension(extensions, required)) {
            set_error(error, error_size, "required device extension is unavailable: %s", required);
            if (fd_owned) {
                ::close(dma_buf_fd);
            }
            return nullptr;
        }
    }

    uint32_t family_count = 0;
    backend->functions.get_queue_family_properties(
        backend->physical_device, &family_count, nullptr);
    std::vector<VkQueueFamilyProperties> families(family_count);
    backend->functions.get_queue_family_properties(
        backend->physical_device, &family_count, families.data());
    backend->queue_family = UINT32_MAX;
    for (uint32_t index = 0; index != family_count; ++index) {
        if ((families[index].queueFlags & VK_QUEUE_TRANSFER_BIT) != 0) {
            backend->queue_family = index;
            if ((families[index].queueFlags & VK_QUEUE_GRAPHICS_BIT) != 0) {
                break;
            }
        }
    }
    if (backend->queue_family == UINT32_MAX) {
        return fail("no transfer-capable Vulkan queue family");
    }

    const float priority = 0.0f;
    VkDeviceQueueCreateInfo queue_info{
        .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
        .queueFamilyIndex = backend->queue_family,
        .queueCount = 1,
        .pQueuePriorities = &priority,
    };
    VkDeviceCreateInfo device_info{
        .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
        .queueCreateInfoCount = 1,
        .pQueueCreateInfos = &queue_info,
        .enabledExtensionCount = static_cast<uint32_t>(required_extensions.size()),
        .ppEnabledExtensionNames = required_extensions.data(),
    };
    result = backend->functions.create_device(
        backend->physical_device, &device_info, nullptr, &backend->device);
    if (result != VK_SUCCESS) {
        return fail("vkCreateDevice", result);
    }
    if (!backend->functions.load_device(backend->device, error, error_size)) {
        if (fd_owned) {
            ::close(dma_buf_fd);
        }
        return nullptr;
    }
    backend->functions.get_device_queue(
        backend->device, backend->queue_family, 0, &backend->queue);

    std::array<VkSubresourceLayout, SPVK_MAX_PLANES> layouts{};
    for (uint32_t index = 0; index != hello->plane_count; ++index) {
        layouts[index] = VkSubresourceLayout{
            .offset = hello->planes[index].offset,
            .size = hello->planes[index].size,
            .rowPitch = hello->planes[index].row_pitch,
            .arrayPitch = hello->planes[index].array_pitch,
            .depthPitch = hello->planes[index].depth_pitch,
        };
    }
    VkImageDrmFormatModifierExplicitCreateInfoEXT modifier_info{
        .sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO_EXT,
        .drmFormatModifier = hello->modifier,
        .drmFormatModifierPlaneCount = hello->plane_count,
        .pPlaneLayouts = layouts.data(),
    };
    VkExternalMemoryImageCreateInfo external_info{
        .sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
        .pNext = &modifier_info,
        .handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
    };
    VkImageCreateInfo image_info{
        .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
        .pNext = &external_info,
        .imageType = VK_IMAGE_TYPE_2D,
        .format = static_cast<VkFormat>(hello->vk_format),
        .extent = { hello->width, hello->height, 1 },
        .mipLevels = 1,
        .arrayLayers = 1,
        .samples = VK_SAMPLE_COUNT_1_BIT,
        .tiling = VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT,
        .usage = VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT,
        .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
        .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
    };
    result = backend->functions.create_image(
        backend->device, &image_info, nullptr, &backend->image);
    if (result != VK_SUCCESS) {
        return fail("vkCreateImage(import)", result);
    }

    VkMemoryRequirements image_requirements{};
    backend->functions.get_image_memory_requirements(
        backend->device, backend->image, &image_requirements);
    VkMemoryFdPropertiesKHR fd_properties{
        .sType = VK_STRUCTURE_TYPE_MEMORY_FD_PROPERTIES_KHR,
    };
    result = backend->functions.get_memory_fd_properties(
        backend->device,
        VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
        dma_buf_fd,
        &fd_properties);
    if (result != VK_SUCCESS) {
        return fail("vkGetMemoryFdPropertiesKHR", result);
    }
    const uint32_t image_memory_bits =
        image_requirements.memoryTypeBits & fd_properties.memoryTypeBits;
    if (image_memory_bits == 0) {
        return fail("DMA-BUF has no compatible consumer memory type");
    }
    const uint32_t image_memory_type = static_cast<uint32_t>(std::countr_zero(image_memory_bits));
    VkImportMemoryFdInfoKHR import_info{
        .sType = VK_STRUCTURE_TYPE_IMPORT_MEMORY_FD_INFO_KHR,
        .handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
        .fd = dma_buf_fd,
    };
    VkMemoryDedicatedAllocateInfo dedicated_info{
        .sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
        .pNext = &import_info,
        .image = backend->image,
    };
    VkMemoryAllocateInfo image_allocate{
        .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
        .pNext = &dedicated_info,
        .allocationSize = image_requirements.size,
        .memoryTypeIndex = image_memory_type,
    };
    result = backend->functions.allocate_memory(
        backend->device, &image_allocate, nullptr, &backend->image_memory);
    if (result != VK_SUCCESS) {
        return fail("vkAllocateMemory(import)", result);
    }
    fd_owned = false;
    result = backend->functions.bind_image_memory(
        backend->device, backend->image, backend->image_memory, 0);
    if (result != VK_SUCCESS) {
        return fail("vkBindImageMemory(import)", result);
    }

    backend->mapped_size = static_cast<size_t>(hello->width) * hello->height * 4;
    VkBufferCreateInfo buffer_info{
        .sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
        .size = backend->mapped_size,
        .usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT,
        .sharingMode = VK_SHARING_MODE_EXCLUSIVE,
    };
    result = backend->functions.create_buffer(
        backend->device, &buffer_info, nullptr, &backend->staging_buffer);
    if (result != VK_SUCCESS) {
        return fail("vkCreateBuffer(staging)", result);
    }
    VkMemoryRequirements buffer_requirements{};
    backend->functions.get_buffer_memory_requirements(
        backend->device, backend->staging_buffer, &buffer_requirements);
    VkPhysicalDeviceMemoryProperties memory_properties{};
    backend->functions.get_memory_properties(
        backend->physical_device, &memory_properties);
    const uint32_t staging_memory_type = choose_memory_type(
        memory_properties,
        buffer_requirements.memoryTypeBits,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT,
        VK_MEMORY_PROPERTY_HOST_COHERENT_BIT | VK_MEMORY_PROPERTY_HOST_CACHED_BIT,
        &backend->staging_coherent);
    if (staging_memory_type == UINT32_MAX) {
        return fail("no host-visible staging memory type");
    }
    VkMemoryAllocateInfo staging_allocate{
        .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
        .allocationSize = buffer_requirements.size,
        .memoryTypeIndex = staging_memory_type,
    };
    result = backend->functions.allocate_memory(
        backend->device, &staging_allocate, nullptr, &backend->staging_memory);
    if (result != VK_SUCCESS) {
        return fail("vkAllocateMemory(staging)", result);
    }
    result = backend->functions.bind_buffer_memory(
        backend->device, backend->staging_buffer, backend->staging_memory, 0);
    if (result != VK_SUCCESS) {
        return fail("vkBindBufferMemory(staging)", result);
    }
    void* mapped = nullptr;
    result = backend->functions.map_memory(
        backend->device, backend->staging_memory, 0, VK_WHOLE_SIZE, 0, &mapped);
    if (result != VK_SUCCESS) {
        return fail("vkMapMemory(staging)", result);
    }
    backend->mapped = static_cast<uint8_t*>(mapped);

    VkCommandPoolCreateInfo pool_info{
        .sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
        .flags = VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT,
        .queueFamilyIndex = backend->queue_family,
    };
    result = backend->functions.create_command_pool(
        backend->device, &pool_info, nullptr, &backend->command_pool);
    if (result != VK_SUCCESS) {
        return fail("vkCreateCommandPool", result);
    }
    VkCommandBufferAllocateInfo command_allocate{
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
        .commandPool = backend->command_pool,
        .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
        .commandBufferCount = 1,
    };
    result = backend->functions.allocate_command_buffers(
        backend->device, &command_allocate, &backend->command_buffer);
    if (result != VK_SUCCESS) {
        return fail("vkAllocateCommandBuffers", result);
    }
    VkFenceCreateInfo fence_info{ .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };
    result = backend->functions.create_fence(
        backend->device, &fence_info, nullptr, &backend->fence);
    if (result != VK_SUCCESS) {
        return fail("vkCreateFence", result);
    }

    return backend.release();
}

extern "C" int spvk_consumer_readback(
    SpvkConsumerBackend* backend,
    const uint8_t** pixels,
    size_t* byte_count,
    uint64_t* submit_done_ns,
    uint64_t* fence_done_ns,
    char* error,
    size_t error_size) {
    auto& f = backend->functions;
    VkResult result = f.reset_fences(backend->device, 1, &backend->fence);
    if (result != VK_SUCCESS) {
        set_error(error, error_size, "vkResetFences: VkResult %d", result);
        return -1;
    }
    result = f.reset_command_pool(backend->device, backend->command_pool, 0);
    if (result != VK_SUCCESS) {
        set_error(error, error_size, "vkResetCommandPool: VkResult %d", result);
        return -1;
    }
    VkCommandBufferBeginInfo begin{
        .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
        .flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
    };
    result = f.begin_command_buffer(backend->command_buffer, &begin);
    if (result != VK_SUCCESS) {
        set_error(error, error_size, "vkBeginCommandBuffer: VkResult %d", result);
        return -1;
    }

    VkImageMemoryBarrier acquire{
        .sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
        .srcAccessMask = 0,
        .dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
        .oldLayout = VK_IMAGE_LAYOUT_GENERAL,
        .newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
        .dstQueueFamilyIndex = backend->queue_family,
        .image = backend->image,
        .subresourceRange = {
            .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
            .baseMipLevel = 0,
            .levelCount = 1,
            .baseArrayLayer = 0,
            .layerCount = 1,
        },
    };
    f.cmd_pipeline_barrier(
        backend->command_buffer,
        VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,
        VK_PIPELINE_STAGE_TRANSFER_BIT,
        0,
        0,
        nullptr,
        0,
        nullptr,
        1,
        &acquire);

    VkBufferImageCopy copy{
        .bufferOffset = 0,
        .bufferRowLength = 0,
        .bufferImageHeight = 0,
        .imageSubresource = {
            .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
            .mipLevel = 0,
            .baseArrayLayer = 0,
            .layerCount = 1,
        },
        .imageOffset = { 0, 0, 0 },
        .imageExtent = { backend->hello.width, backend->hello.height, 1 },
    };
    f.cmd_copy_image_to_buffer(
        backend->command_buffer,
        backend->image,
        VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
        backend->staging_buffer,
        1,
        &copy);

    VkImageMemoryBarrier release = acquire;
    release.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT;
    release.dstAccessMask = 0;
    release.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL;
    release.newLayout = VK_IMAGE_LAYOUT_GENERAL;
    release.srcQueueFamilyIndex = backend->queue_family;
    release.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL;
    VkBufferMemoryBarrier host_barrier{
        .sType = VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER,
        .srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
        .dstAccessMask = VK_ACCESS_HOST_READ_BIT,
        .srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
        .buffer = backend->staging_buffer,
        .offset = 0,
        .size = VK_WHOLE_SIZE,
    };
    f.cmd_pipeline_barrier(
        backend->command_buffer,
        VK_PIPELINE_STAGE_TRANSFER_BIT,
        VK_PIPELINE_STAGE_HOST_BIT | VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT,
        0,
        0,
        nullptr,
        1,
        &host_barrier,
        1,
        &release);
    result = f.end_command_buffer(backend->command_buffer);
    if (result != VK_SUCCESS) {
        set_error(error, error_size, "vkEndCommandBuffer: VkResult %d", result);
        return -1;
    }

    VkSubmitInfo submit{
        .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
        .commandBufferCount = 1,
        .pCommandBuffers = &backend->command_buffer,
    };
    result = f.queue_submit(backend->queue, 1, &submit, backend->fence);
    *submit_done_ns = spvk_monotonic_ns();
    if (result != VK_SUCCESS) {
        set_error(error, error_size, "vkQueueSubmit(readback): VkResult %d", result);
        return -1;
    }
    result = f.wait_for_fences(backend->device, 1, &backend->fence, VK_TRUE, UINT64_MAX);
    *fence_done_ns = spvk_monotonic_ns();
    if (result != VK_SUCCESS) {
        set_error(error, error_size, "vkWaitForFences(readback): VkResult %d", result);
        return -1;
    }
    if (!backend->staging_coherent) {
        VkMappedMemoryRange range{
            .sType = VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE,
            .memory = backend->staging_memory,
            .offset = 0,
            .size = VK_WHOLE_SIZE,
        };
        result = f.invalidate_mapped_ranges(backend->device, 1, &range);
        if (result != VK_SUCCESS) {
            set_error(error, error_size, "vkInvalidateMappedMemoryRanges: VkResult %d", result);
            return -1;
        }
    }
    *pixels = backend->mapped;
    *byte_count = backend->mapped_size;
    return 0;
}

extern "C" void spvk_consumer_destroy(SpvkConsumerBackend* backend) {
    delete backend;
}

extern "C" int spvk_write_ppm(
    const char* path,
    const uint8_t* pixels,
    uint32_t width,
    uint32_t height,
    uint32_t vk_format,
    char* error,
    size_t error_size) {
    if (spvk_make_parent_directories(path, error, error_size) != 0) {
        return -1;
    }
    const int fd = ::open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, S_IRUSR | S_IWUSR);
    if (fd < 0) {
        set_error(error, error_size, "open(%s): %s", path, std::strerror(errno));
        return -1;
    }
    char header[80];
    const int header_size = std::snprintf(header, sizeof(header), "P6\n%u %u\n255\n", width, height);
    bool success = write_all(fd, reinterpret_cast<const uint8_t*>(header), static_cast<size_t>(header_size));
    std::vector<uint8_t> row(static_cast<size_t>(width) * 3);
    const bool bgra = vk_format == VK_FORMAT_B8G8R8A8_UNORM ||
        vk_format == VK_FORMAT_B8G8R8A8_SRGB;
    for (uint32_t y = 0; success && y != height; ++y) {
        const uint8_t* source = pixels + static_cast<size_t>(y) * width * 4;
        for (uint32_t x = 0; x != width; ++x) {
            const size_t input = static_cast<size_t>(x) * 4;
            const size_t output = static_cast<size_t>(x) * 3;
            row[output + 0] = source[input + (bgra ? 2 : 0)];
            row[output + 1] = source[input + 1];
            row[output + 2] = source[input + (bgra ? 0 : 2)];
        }
        success = write_all(fd, row.data(), row.size());
    }
    const int close_result = ::close(fd);
    if (!success || close_result != 0) {
        set_error(error, error_size, "write(%s): %s", path, std::strerror(errno));
        ::unlink(path);
        return -1;
    }
    return 0;
}

extern "C" int spvk_write_text(
    const char* path,
    const uint8_t* bytes,
    size_t byte_count,
    int append,
    size_t maximum_size,
    char* error,
    size_t error_size) {
    if (spvk_make_parent_directories(path, error, error_size) != 0) {
        return -1;
    }
    int flags = O_WRONLY | O_CREAT | O_CLOEXEC;
    flags |= append ? O_APPEND : O_TRUNC;
    const int fd = ::open(path, flags, S_IRUSR | S_IWUSR);
    if (fd < 0) {
        set_error(error, error_size, "open(%s): %s", path, std::strerror(errno));
        return -1;
    }
    struct stat status{};
    if (::fstat(fd, &status) != 0 || status.st_size < 0 ||
        static_cast<size_t>(status.st_size) > maximum_size ||
        byte_count > maximum_size - static_cast<size_t>(status.st_size)) {
        set_error(error, error_size, "recording capacity reached");
        ::close(fd);
        return -2;
    }
    const bool success = write_all(fd, bytes, byte_count);
    const int close_result = ::close(fd);
    if (!success || close_result != 0) {
        set_error(error, error_size, "write(%s): %s", path, std::strerror(errno));
        return -1;
    }
    return 0;
}
