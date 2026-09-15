#define _GNU_SOURCE

#include <fcntl.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <pipewire/pipewire.h>
#include <spa/param/video/format-utils.h>

#define BYTES_PER_PIXEL 4

struct source {
    struct pw_main_loop *loop;
    struct pw_stream *stream;
    struct spa_hook listener;
    struct spa_source *frame_timer;
    struct spa_source *renegotiate_timer;
    struct spa_source *quit_timer;
    struct spa_video_info_raw format;
    int32_t stride;
};

static const struct spa_pod *build_format(
    struct spa_pod_builder *builder,
    uint32_t width,
    uint32_t height) {
    return spa_pod_builder_add_object(
        builder,
        SPA_TYPE_OBJECT_Format,
        SPA_PARAM_EnumFormat,
        SPA_FORMAT_mediaType,
        SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype,
        SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format,
        SPA_POD_Id(SPA_VIDEO_FORMAT_BGRx),
        SPA_FORMAT_VIDEO_size,
        SPA_POD_Rectangle(&SPA_RECTANGLE(width, height)),
        SPA_FORMAT_VIDEO_framerate,
        SPA_POD_Fraction(&SPA_FRACTION(30, 1)));
}

static void on_process(void *userdata) {
    struct source *source = userdata;
    struct pw_buffer *buffer = pw_stream_dequeue_buffer(source->stream);
    if (buffer == NULL) {
        return;
    }
    struct spa_data *data = &buffer->buffer->datas[0];
    const uint32_t size = source->stride * source->format.size.height;
    if (data->data != NULL && data->chunk != NULL && size <= data->maxsize) {
        memset(data->data, 0x20, size);
        data->chunk->offset = 0;
        data->chunk->size = size;
        data->chunk->stride = source->stride;
    }
    pw_stream_queue_buffer(source->stream, buffer);
}

static void on_param_changed(void *userdata, uint32_t id, const struct spa_pod *param) {
    struct source *source = userdata;
    if (param == NULL || id != SPA_PARAM_Format ||
        spa_format_video_raw_parse(param, &source->format) < 0) {
        return;
    }
    source->stride = (int32_t)(source->format.size.width * BYTES_PER_PIXEL);
    uint8_t storage[512];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    const struct spa_pod *params[] = {
        spa_pod_builder_add_object(
            &builder,
            SPA_TYPE_OBJECT_ParamBuffers,
            SPA_PARAM_Buffers,
            SPA_PARAM_BUFFERS_buffers,
            SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
            SPA_PARAM_BUFFERS_blocks,
            SPA_POD_Int(1),
            SPA_PARAM_BUFFERS_size,
            SPA_POD_Int(source->stride * (int32_t)source->format.size.height),
            SPA_PARAM_BUFFERS_stride,
            SPA_POD_Int(source->stride),
            SPA_PARAM_BUFFERS_dataType,
            SPA_POD_CHOICE_FLAGS_Int(1 << SPA_DATA_MemFd)),
    };
    pw_stream_update_params(source->stream, params, 1);
}

static void on_add_buffer(void *userdata, struct pw_buffer *buffer) {
    struct source *source = userdata;
    struct spa_data *data = &buffer->buffer->datas[0];
    data->type = SPA_DATA_MemFd;
    data->flags = SPA_DATA_FLAG_READWRITE | SPA_DATA_FLAG_MAPPABLE;
    data->fd = memfd_create("scorepeek-contract-source", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    data->mapoffset = 0;
    data->maxsize = source->stride * source->format.size.height;
    if (data->fd < 0 || ftruncate(data->fd, data->maxsize) < 0) {
        return;
    }
    data->data = mmap(
        NULL,
        data->maxsize,
        PROT_READ | PROT_WRITE,
        MAP_SHARED,
        data->fd,
        data->mapoffset);
    if (data->data == MAP_FAILED) {
        data->data = NULL;
    }
}

static void on_remove_buffer(void *userdata, struct pw_buffer *buffer) {
    (void)userdata;
    struct spa_data *data = &buffer->buffer->datas[0];
    if (data->data != NULL) {
        munmap(data->data, data->maxsize);
    }
    if (data->fd >= 0) {
        close(data->fd);
    }
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .process = on_process,
    .param_changed = on_param_changed,
    .add_buffer = on_add_buffer,
    .remove_buffer = on_remove_buffer,
};

static void trigger_frame(void *userdata, uint64_t expirations) {
    (void)expirations;
    struct source *source = userdata;
    pw_stream_trigger_process(source->stream);
}

static void renegotiate(void *userdata, uint64_t expirations) {
    (void)expirations;
    struct source *source = userdata;
    uint8_t storage[512];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    const struct spa_pod *params[] = {build_format(&builder, 800, 600)};
    pw_stream_update_params(source->stream, params, 1);
}

static void quit(void *userdata, uint64_t expirations) {
    (void)expirations;
    struct source *source = userdata;
    pw_main_loop_quit(source->loop);
}

static void stop_signal(void *userdata, int signal_number) {
    (void)signal_number;
    struct source *source = userdata;
    pw_main_loop_quit(source->loop);
}

int main(int argc, char **argv) {
    if (argc != 2) {
        return 2;
    }
    pw_init(&argc, &argv);
    struct source source = {0};
    source.format.size = SPA_RECTANGLE(640, 480);
    source.stride = 640 * BYTES_PER_PIXEL;
    source.loop = pw_main_loop_new(NULL);
    struct pw_loop *loop = pw_main_loop_get_loop(source.loop);
    pw_loop_add_signal(loop, SIGINT, stop_signal, &source);
    pw_loop_add_signal(loop, SIGTERM, stop_signal, &source);
    source.stream = pw_stream_new_simple(
        loop,
        "scorepeek-contract-source",
        pw_properties_new(
            PW_KEY_MEDIA_CLASS,
            "Video/Source",
            PW_KEY_NODE_NAME,
            argv[1],
            NULL),
        &stream_events,
        &source);
    uint8_t storage[512];
    struct spa_pod_builder builder = SPA_POD_BUILDER_INIT(storage, sizeof(storage));
    const struct spa_pod *params[] = {build_format(&builder, 640, 480)};
    if (source.stream == NULL ||
        pw_stream_connect(
            source.stream,
            PW_DIRECTION_OUTPUT,
            PW_ID_ANY,
            PW_STREAM_FLAG_DRIVER | PW_STREAM_FLAG_ALLOC_BUFFERS,
            params,
            1) < 0) {
        return 1;
    }

    source.frame_timer = pw_loop_add_timer(loop, trigger_frame, &source);
    source.renegotiate_timer = pw_loop_add_timer(loop, renegotiate, &source);
    source.quit_timer = pw_loop_add_timer(loop, quit, &source);
    struct timespec frame_start = {.tv_nsec = 1};
    struct timespec frame_interval = {.tv_nsec = 33333333};
    struct timespec renegotiate_at = {.tv_sec = 3};
    struct timespec quit_at = {.tv_sec = 9};
    pw_loop_update_timer(loop, source.frame_timer, &frame_start, &frame_interval, false);
    pw_loop_update_timer(loop, source.renegotiate_timer, &renegotiate_at, NULL, false);
    pw_loop_update_timer(loop, source.quit_timer, &quit_at, NULL, false);
    pw_main_loop_run(source.loop);

    pw_stream_destroy(source.stream);
    pw_main_loop_destroy(source.loop);
    pw_deinit();
    return 0;
}
