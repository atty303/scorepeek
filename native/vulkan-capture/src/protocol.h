#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    SPVK_MAGIC = 0x4b565053u,
    SPVK_VERSION = 5u,
    SPVK_MAX_PLANES = 4u,
};

typedef enum SpvkMessageType {
    SPVK_MESSAGE_HELLO = 1,
    SPVK_MESSAGE_HELLO_ACK = 2,
    SPVK_MESSAGE_REQUEST = 3,
    SPVK_MESSAGE_READY = 4,
    SPVK_MESSAGE_ACK = 5,
    SPVK_MESSAGE_ERROR = 6,
    SPVK_MESSAGE_ADMIT = 7,
    SPVK_MESSAGE_ADMIT_ACK = 8,
    SPVK_MESSAGE_STATUS = 9,
} SpvkMessageType;

typedef enum SpvkErrorType {
    SPVK_ERROR_NONE = 0,
    SPVK_ERROR_PROTOCOL_INVALID = 1,
    SPVK_ERROR_QUEUE_UNSUPPORTED = 2,
    SPVK_ERROR_SUBMIT_FAILED = 3,
    SPVK_ERROR_FENCE_FAILED = 4,
    SPVK_ERROR_PEER_DISCONNECTED = 5,
    SPVK_ERROR_IMPORT_FAILED = 6,
    SPVK_ERROR_READBACK_FAILED = 7,
    SPVK_ERROR_RECORDING_DEGRADED = 8,
    SPVK_ERROR_SOURCE_BUSY = 9,
} SpvkErrorType;

typedef struct SpvkPlaneLayout {
    uint64_t offset;
    uint64_t size;
    uint64_t row_pitch;
    uint64_t array_pitch;
    uint64_t depth_pitch;
} SpvkPlaneLayout;

typedef struct SpvkHello {
    uint32_t magic;
    uint16_t version;
    uint16_t type;
    uint32_t size;
    uint32_t width;
    uint32_t height;
    uint32_t vk_format;
    uint32_t drm_fourcc;
    uint32_t plane_count;
    uint32_t reserved;
    uint64_t modifier;
    uint64_t allocation_size;
    uint8_t device_uuid[16];
    SpvkPlaneLayout planes[SPVK_MAX_PLANES];
} SpvkHello;

typedef struct SpvkPacket {
    uint32_t magic;
    uint16_t version;
    uint16_t type;
    uint64_t sequence;
    uint64_t request_ns;
    uint64_t present_ns;
    uint64_t submit_done_ns;
    uint64_t present_call_ns;
    uint64_t producer_fence_ns;
    uint64_t requests;
    uint64_t captures;
    uint64_t busy_drops;
    uint64_t coalesced_drops;
    int32_t status;
    uint32_t reserved;
} SpvkPacket;

#ifdef __cplusplus
}

static_assert(sizeof(SpvkPlaneLayout) == 40);
static_assert(sizeof(SpvkHello) == 232);
static_assert(sizeof(SpvkPacket) == 96);
#endif
