#pragma once

#include "protocol.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct SpvkConsumerBackend SpvkConsumerBackend;

uint64_t spvk_monotonic_ns(void);
uint64_t spvk_process_id(void);
const char* spvk_getenv(const char* name);

int spvk_listen(const char* path, char* error, size_t error_size);
int spvk_accept(int listener, char* error, size_t error_size);
int spvk_receive_hello(int socket_fd, SpvkHello* hello, int* dma_buf_fd, char* error, size_t error_size);
int spvk_send_packet(int socket_fd, const SpvkPacket* packet, char* error, size_t error_size);
int spvk_receive_packet(int socket_fd, int wake_fd, SpvkPacket* packet, int timeout_ms, char* error, size_t error_size);
int spvk_event_create(char* error, size_t error_size);
int spvk_event_signal(int event_fd, char* error, size_t error_size);
int spvk_event_consume(int event_fd, char* error, size_t error_size);
void spvk_close(int fd);
void spvk_unlink(const char* path);

SpvkConsumerBackend* spvk_consumer_create(
    const SpvkHello* hello,
    int dma_buf_fd,
    char* error,
    size_t error_size);
int spvk_consumer_readback(
    SpvkConsumerBackend* backend,
    const uint8_t** pixels,
    size_t* byte_count,
    uint64_t* submit_done_ns,
    uint64_t* fence_done_ns,
    char* error,
    size_t error_size);
void spvk_consumer_destroy(SpvkConsumerBackend* backend);

int spvk_write_ppm(
    const char* path,
    const uint8_t* pixels,
    uint32_t width,
    uint32_t height,
    uint32_t vk_format,
    char* error,
    size_t error_size);
int spvk_write_text(
    const char* path,
    const uint8_t* bytes,
    size_t byte_count,
    int append,
    size_t maximum_size,
    char* error,
    size_t error_size);
int spvk_make_parent_directories(const char* path, char* error, size_t error_size);

#ifdef __cplusplus
}
#endif
