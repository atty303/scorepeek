const std = @import("std");

const c = @cImport({
    @cInclude("consumer_backend.h");
});

const maximum_diagnostic_bytes = 8 * 1024 * 1024;
const request_period_ns = 100_000_000;

const Config = struct {
    socket_path: [:0]const u8,
    diagnostic_path: [:0]const u8,
    artifact_path: [:0]const u8,
    artifact_metadata_path: [:0]const u8,
    maximum_frames: u64,
    diagnostics_enabled: bool,
    artifact_enabled: bool,
};

const Diagnostics = struct {
    path: [:0]const u8,
    enabled: bool,
    initialized: bool = false,
    degraded: bool = false,

    fn record(self: *Diagnostics, comptime format: []const u8, args: anytype) void {
        if (!self.enabled or self.degraded) return;
        var buffer: [2048]u8 = undefined;
        const bytes = std.fmt.bufPrint(&buffer, format, args) catch {
            self.degraded = true;
            return;
        };
        var error_buffer = [_:0]u8{0} ** 512;
        const result = c.spvk_write_text(
            self.path.ptr,
            bytes.ptr,
            bytes.len,
            @intFromBool(self.initialized),
            maximum_diagnostic_bytes,
            &error_buffer,
            error_buffer.len,
        );
        if (result != 0) {
            self.degraded = true;
            return;
        }
        self.initialized = true;
    }
};

const Worker = struct {
    job_event: c_int,
    done_event: c_int,
    artifact_path: [:0]const u8,
    metadata_path: [:0]const u8,
    artifact_enabled: bool,
    artifact_written: bool = false,
    stop: std.atomic.Value(bool) = .init(false),
    job_ready: std.atomic.Value(bool) = .init(false),
    done: std.atomic.Value(bool) = .init(false),
    pixels: [*]const u8 = undefined,
    byte_count: usize = 0,
    hello: c.SpvkHello = undefined,
    sequence: u64 = 0,
    digest: [32]u8 = undefined,
    success: bool = false,
    error_buffer: [512:0]u8 = [_:0]u8{0} ** 512,

    fn submit(
        self: *Worker,
        pixels: [*]const u8,
        byte_count: usize,
        hello: c.SpvkHello,
        sequence: u64,
    ) bool {
        if (self.job_ready.load(.acquire) or self.done.load(.acquire)) return false;
        self.pixels = pixels;
        self.byte_count = byte_count;
        self.hello = hello;
        self.sequence = sequence;
        self.success = false;
        @memset(&self.error_buffer, 0);
        self.job_ready.store(true, .release);
        var signal_error = [_:0]u8{0} ** 128;
        if (c.spvk_event_signal(self.job_event, &signal_error, signal_error.len) != 0) {
            self.job_ready.store(false, .release);
            return false;
        }
        return true;
    }

    fn takeDone(self: *Worker) ?WorkerResult {
        if (!self.done.load(.acquire)) return null;
        const result = WorkerResult{
            .sequence = self.sequence,
            .digest = self.digest,
            .success = self.success,
            .error_message = std.mem.sliceTo(self.error_buffer[0..], 0),
            .artifact_written = self.artifact_written,
        };
        self.done.store(false, .release);
        return result;
    }

    fn shutdown(self: *Worker) void {
        self.stop.store(true, .release);
        var signal_error = [_:0]u8{0} ** 128;
        _ = c.spvk_event_signal(self.job_event, &signal_error, signal_error.len);
    }

    fn threadMain(self: *Worker) void {
        while (true) {
            var event_error = [_:0]u8{0} ** 128;
            if (c.spvk_event_consume(self.job_event, &event_error, event_error.len) != 0) return;
            if (self.stop.load(.acquire)) return;
            if (!self.job_ready.load(.acquire)) continue;

            const frame = self.pixels[0..self.byte_count];
            std.crypto.hash.sha2.Sha256.hash(frame, &self.digest, .{});
            self.success = true;
            if (self.artifact_enabled and !self.artifact_written) {
                if (c.spvk_write_ppm(
                    self.artifact_path.ptr,
                    frame.ptr,
                    self.hello.width,
                    self.hello.height,
                    self.hello.vk_format,
                    &self.error_buffer,
                    self.error_buffer.len,
                ) != 0) {
                    self.success = false;
                } else {
                    const digest_hex = std.fmt.bytesToHex(self.digest, .lower);
                    var metadata: [1024]u8 = undefined;
                    const bytes = std.fmt.bufPrint(
                        &metadata,
                        "{{\"schema\":\"scorepeek-vulkan-capture-frame-v1\",\"sequence\":{d},\"width\":{d},\"height\":{d},\"vk_format\":{d},\"drm_fourcc\":{d},\"modifier\":{d},\"bytes\":{d},\"sha256\":\"{s}\"}}\n",
                        .{
                            self.sequence,
                            self.hello.width,
                            self.hello.height,
                            self.hello.vk_format,
                            self.hello.drm_fourcc,
                            self.hello.modifier,
                            self.byte_count,
                            digest_hex,
                        },
                    ) catch {
                        self.success = false;
                        self.job_ready.store(false, .release);
                        self.done.store(true, .release);
                        _ = c.spvk_event_signal(self.done_event, &event_error, event_error.len);
                        continue;
                    };
                    if (c.spvk_write_text(
                        self.metadata_path.ptr,
                        bytes.ptr,
                        bytes.len,
                        0,
                        4096,
                        &self.error_buffer,
                        self.error_buffer.len,
                    ) != 0) {
                        self.success = false;
                    } else {
                        self.artifact_written = true;
                    }
                }
            }
            self.job_ready.store(false, .release);
            self.done.store(true, .release);
            _ = c.spvk_event_signal(self.done_event, &event_error, event_error.len);
        }
    }
};

const WorkerResult = struct {
    sequence: u64,
    digest: [32]u8,
    success: bool,
    error_message: []const u8,
    artifact_written: bool,
};

fn buildConfig(init: std.process.Init, run_id: u64) !Config {
    const allocator = init.arena.allocator();
    const runtime_dir = init.environ_map.get("XDG_RUNTIME_DIR") orelse "/tmp";
    const state_root = init.environ_map.get("XDG_STATE_HOME") orelse blk: {
        const home = init.environ_map.get("HOME") orelse "/tmp";
        break :blk try std.fmt.allocPrint(allocator, "{s}/.local/state", .{home});
    };
    var config = Config{
        .socket_path = try std.fmt.allocPrintSentinel(
            allocator,
            "{s}/scorepeek-vulkan-capture-spike.sock",
            .{runtime_dir},
            0,
        ),
        .diagnostic_path = try std.fmt.allocPrintSentinel(
            allocator,
            "{s}/scorepeek/vulkan-capture-spike/latest.ndjson",
            .{state_root},
            0,
        ),
        .artifact_path = try std.fmt.allocPrintSentinel(
            allocator,
            "{s}/scorepeek/vulkan-capture-spike/frame-{d}.ppm",
            .{ state_root, run_id },
            0,
        ),
        .artifact_metadata_path = try std.fmt.allocPrintSentinel(
            allocator,
            "{s}/scorepeek/vulkan-capture-spike/frame-{d}.json",
            .{ state_root, run_id },
            0,
        ),
        .maximum_frames = 0,
        .diagnostics_enabled = true,
        .artifact_enabled = true,
    };

    const args = try init.minimal.args.toSlice(allocator);
    var index: usize = 1;
    while (index < args.len) : (index += 1) {
        const argument = args[index];
        if (std.mem.eql(u8, argument, "--socket")) {
            index += 1;
            if (index >= args.len) return error.MissingArgument;
            config.socket_path = try allocator.dupeZ(u8, args[index]);
        } else if (std.mem.eql(u8, argument, "--diagnostics")) {
            index += 1;
            if (index >= args.len) return error.MissingArgument;
            config.diagnostic_path = try allocator.dupeZ(u8, args[index]);
        } else if (std.mem.eql(u8, argument, "--artifact")) {
            index += 1;
            if (index >= args.len) return error.MissingArgument;
            config.artifact_path = try allocator.dupeZ(u8, args[index]);
            config.artifact_metadata_path = try std.fmt.allocPrintSentinel(
                allocator,
                "{s}.json",
                .{args[index]},
                0,
            );
        } else if (std.mem.eql(u8, argument, "--frames")) {
            index += 1;
            if (index >= args.len) return error.MissingArgument;
            config.maximum_frames = try std.fmt.parseInt(u64, args[index], 10);
        } else if (std.mem.eql(u8, argument, "--no-diagnostics")) {
            config.diagnostics_enabled = false;
        } else if (std.mem.eql(u8, argument, "--no-artifact")) {
            config.artifact_enabled = false;
        } else {
            return error.UnknownArgument;
        }
    }
    return config;
}

fn packet(run_id: u64, message_type: u16, sequence: u64) c.SpvkPacket {
    return .{
        .magic = c.SPVK_MAGIC,
        .version = c.SPVK_VERSION,
        .type = message_type,
        .run_id = run_id,
        .sequence = sequence,
        .request_ns = 0,
        .present_ns = 0,
        .submit_done_ns = 0,
        .fence_done_ns = 0,
        .requests = 0,
        .captures = 0,
        .busy_drops = 0,
        .coalesced_drops = 0,
        .status = c.SPVK_ERROR_NONE,
        .reserved = 0,
    };
}

fn cError(buffer: []const u8) []const u8 {
    return std.mem.sliceTo(buffer, 0);
}

fn sendPacket(fd: c_int, value: *const c.SpvkPacket, error_buffer: []u8) !void {
    @memset(error_buffer, 0);
    if (c.spvk_send_packet(fd, value, error_buffer.ptr, error_buffer.len) != 0) {
        return error.IpcFailure;
    }
}

const SessionOutcome = enum { disconnected, complete, worker_failure };

const RunState = struct {
    request_sequence: u64 = 0,
    completed_frames: u64 = 0,
    last_digest: [32]u8 = [_]u8{0} ** 32,
    artifact_written: bool = false,
};

fn runSession(
    listener: c_int,
    run_id: u64,
    config: Config,
    diagnostics: *Diagnostics,
    state: *RunState,
) !SessionOutcome {
    var error_buffer = [_:0]u8{0} ** 512;
    const socket_fd = c.spvk_accept(listener, &error_buffer, error_buffer.len);
    if (socket_fd < 0) return error.AcceptFailure;
    defer c.spvk_close(socket_fd);

    var hello: c.SpvkHello = undefined;
    var dma_buf_fd: c_int = -1;
    if (c.spvk_receive_hello(
        socket_fd,
        &hello,
        &dma_buf_fd,
        &error_buffer,
        error_buffer.len,
    ) != 0) {
        diagnostics.record(
            "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"ipc.handshake\",\"status\":\"error\",\"error.type\":\"protocol_invalid\"}}\n",
            .{run_id},
        );
        std.debug.print("consumer handshake failure: {s}\n", .{cError(&error_buffer)});
        return .disconnected;
    }
    diagnostics.record(
        "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"ipc.handshake\",\"status\":\"success\",\"width\":{d},\"height\":{d},\"vk_format\":{d},\"drm_fourcc\":{d},\"modifier\":{d},\"plane_count\":{d}}}\n",
        .{ run_id, hello.width, hello.height, hello.vk_format, hello.drm_fourcc, hello.modifier, hello.plane_count },
    );

    const backend = c.spvk_consumer_create(
        &hello,
        dma_buf_fd,
        &error_buffer,
        error_buffer.len,
    ) orelse {
        diagnostics.record(
            "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"vulkan.import\",\"status\":\"error\",\"error.type\":\"import_failed\"}}\n",
            .{run_id},
        );
        std.debug.print("consumer Vulkan import failure: {s}\n", .{cError(&error_buffer)});
        return error.ImportFailure;
    };
    defer c.spvk_consumer_destroy(backend);

    const job_event = c.spvk_event_create(&error_buffer, error_buffer.len);
    if (job_event < 0) return error.EventFailure;
    defer c.spvk_close(job_event);
    const done_event = c.spvk_event_create(&error_buffer, error_buffer.len);
    if (done_event < 0) return error.EventFailure;
    defer c.spvk_close(done_event);

    var worker = Worker{
        .job_event = job_event,
        .done_event = done_event,
        .artifact_path = config.artifact_path,
        .metadata_path = config.artifact_metadata_path,
        .artifact_enabled = config.artifact_enabled,
        .artifact_written = state.artifact_written,
    };
    const worker_thread = try std.Thread.spawn(.{}, Worker.threadMain, .{&worker});
    defer {
        worker.shutdown();
        worker_thread.join();
    }

    var hello_ack = packet(run_id, c.SPVK_MESSAGE_HELLO_ACK, 0);
    sendPacket(socket_fd, &hello_ack, &error_buffer) catch return .disconnected;

    var next_request_ns = c.spvk_monotonic_ns();
    var active_sequence: ?u64 = null;

    while (config.maximum_frames == 0 or state.completed_frames < config.maximum_frames) {
        const now = c.spvk_monotonic_ns();
        if (now >= next_request_ns) {
            state.request_sequence += 1;
            var request = packet(run_id, c.SPVK_MESSAGE_REQUEST, state.request_sequence);
            request.request_ns = now;
            sendPacket(socket_fd, &request, &error_buffer) catch return .disconnected;
            diagnostics.record(
                "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"capture.request\",\"sequence\":{d},\"time_ns\":{d},\"status\":\"success\"}}\n",
                .{ run_id, state.request_sequence, now },
            );
            next_request_ns = now + request_period_ns;
        }

        var incoming: c.SpvkPacket = undefined;
        const receive_result = c.spvk_receive_packet(
            socket_fd,
            done_event,
            &incoming,
            10,
            &error_buffer,
            error_buffer.len,
        );
        if (receive_result < 0) {
            diagnostics.record(
                "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"ipc.session\",\"status\":\"degraded\",\"event\":\"producer_disconnected\"}}\n",
                .{run_id},
            );
            return .disconnected;
        }
        if (receive_result == 2) {
            if (c.spvk_event_consume(done_event, &error_buffer, error_buffer.len) != 0) {
                return error.EventFailure;
            }
            const result = worker.takeDone() orelse continue;
            if (active_sequence == null or active_sequence.? != result.sequence) {
                return error.WorkerContractViolation;
            }
            state.last_digest = result.digest;
            state.completed_frames += 1;
            state.artifact_written = result.artifact_written;
            const digest_hex = std.fmt.bytesToHex(result.digest, .lower);
            diagnostics.record(
                "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"worker.consume\",\"sequence\":{d},\"status\":\"{s}\",\"sha256\":\"{s}\",\"artifact_written\":{s}}}\n",
                .{
                    run_id,
                    result.sequence,
                    if (result.success) "success" else "error",
                    digest_hex,
                    if (result.artifact_written) "true" else "false",
                },
            );
            var acknowledgement = packet(run_id, c.SPVK_MESSAGE_ACK, result.sequence);
            sendPacket(socket_fd, &acknowledgement, &error_buffer) catch return .disconnected;
            active_sequence = null;
            if (!result.success) {
                std.debug.print("consumer worker failure: {s}\n", .{result.error_message});
                return .worker_failure;
            }
            continue;
        }
        if (receive_result == 0) continue;

        if (incoming.type == c.SPVK_MESSAGE_READY) {
            if (active_sequence != null) return error.MultipleReadyFrames;
            var pixels: [*c]const u8 = null;
            var byte_count: usize = 0;
            var readback_submit_ns: u64 = 0;
            var readback_fence_ns: u64 = 0;
            if (c.spvk_consumer_readback(
                backend,
                &pixels,
                &byte_count,
                &readback_submit_ns,
                &readback_fence_ns,
                &error_buffer,
                error_buffer.len,
            ) != 0) {
                diagnostics.record(
                    "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"vulkan.readback\",\"sequence\":{d},\"status\":\"error\",\"error.type\":\"readback_failed\"}}\n",
                    .{ run_id, incoming.sequence },
                );
                std.debug.print("consumer readback failure: {s}\n", .{cError(&error_buffer)});
                return error.ReadbackFailure;
            }
            diagnostics.record(
                "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"vulkan.readback\",\"sequence\":{d},\"status\":\"success\",\"request_to_present_ns\":{d},\"producer_submit_ns\":{d},\"producer_fence_ns\":{d},\"consumer_submit_ns\":{d},\"consumer_fence_ns\":{d},\"bytes\":{d},\"requests\":{d},\"captures\":{d},\"busy_drops\":{d},\"coalesced_drops\":{d}}}\n",
                .{
                    run_id,
                    incoming.sequence,
                    incoming.present_ns -| incoming.request_ns,
                    incoming.submit_done_ns -| incoming.present_ns,
                    incoming.fence_done_ns -| incoming.submit_done_ns,
                    readback_submit_ns -| incoming.fence_done_ns,
                    readback_fence_ns -| readback_submit_ns,
                    byte_count,
                    incoming.requests,
                    incoming.captures,
                    incoming.busy_drops,
                    incoming.coalesced_drops,
                },
            );
            if (!worker.submit(
                @ptrCast(pixels),
                byte_count,
                hello,
                incoming.sequence,
            )) {
                return error.WorkerBusy;
            }
            active_sequence = incoming.sequence;
        } else if (incoming.type == c.SPVK_MESSAGE_ERROR) {
            diagnostics.record(
                "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"producer.capture\",\"sequence\":{d},\"status\":\"error\",\"error.type\":{d}}}\n",
                .{ run_id, incoming.sequence, incoming.status },
            );
        } else {
            return error.ProtocolViolation;
        }
    }
    return .complete;
}

pub fn main(init: std.process.Init) !void {
    const run_id = c.spvk_monotonic_ns() ^ (c.spvk_process_id() << 32);
    const config = try buildConfig(init, run_id);
    var diagnostics = Diagnostics{
        .path = config.diagnostic_path,
        .enabled = config.diagnostics_enabled,
    };
    diagnostics.record(
        "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"resource\":\"scorepeek-vulkan-capture-consumer\",\"event\":\"run_start\",\"status\":\"success\",\"completeness\":\"partial\"}}\n",
        .{run_id},
    );

    var error_buffer = [_:0]u8{0} ** 512;
    const listener = c.spvk_listen(config.socket_path.ptr, &error_buffer, error_buffer.len);
    if (listener < 0) {
        diagnostics.record(
            "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"ipc.listen\",\"status\":\"error\",\"error.type\":\"io\"}}\n",
            .{run_id},
        );
        std.debug.print("consumer listen failure: {s}\n", .{cError(&error_buffer)});
        return error.ListenFailure;
    }
    defer {
        c.spvk_close(listener);
        c.spvk_unlink(config.socket_path.ptr);
    }

    var state = RunState{};
    var worker_failure = false;
    while (config.maximum_frames == 0 or state.completed_frames < config.maximum_frames) {
        const outcome = try runSession(listener, run_id, config, &diagnostics, &state);
        switch (outcome) {
            .disconnected => diagnostics.record(
                "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"operation\":\"ipc.session\",\"status\":\"success\",\"event\":\"awaiting_reconnect\"}}\n",
                .{run_id},
            ),
            .complete => break,
            .worker_failure => {
                worker_failure = true;
                break;
            },
        }
    }

    const completeness = if (diagnostics.degraded) "partial" else "complete";
    diagnostics.record(
        "{{\"schema\":\"scorepeek-vulkan-capture-diagnostic-v1\",\"run_id\":{d},\"resource\":\"scorepeek-vulkan-capture-consumer\",\"event\":\"run_end\",\"status\":\"{s}\",\"completeness\":\"{s}\",\"frames\":{d}}}\n",
        .{ run_id, if (worker_failure) "error" else "success", completeness, state.completed_frames },
    );

    var stdout_buffer: [1024]u8 = undefined;
    var stdout_file = std.Io.File.Writer.init(.stdout(), init.io, &stdout_buffer);
    const stdout = &stdout_file.interface;
    const digest_hex = std.fmt.bytesToHex(state.last_digest, .lower);
    try stdout.print(
        "{{\"schema\":\"scorepeek-vulkan-capture-result-v1\",\"run_id\":{d},\"frames\":{d},\"last_sha256\":\"{s}\",\"diagnostics\":\"{s}\",\"artifact\":\"{s}\"}}\n",
        .{
            run_id,
            state.completed_frames,
            digest_hex,
            if (diagnostics.degraded) "partial" else if (diagnostics.enabled) "complete" else "disabled",
            if (config.artifact_enabled) config.artifact_path else "disabled",
        },
    );
    try stdout.flush();
    if (worker_failure) return error.WorkerFailure;
}

test "wire structures retain their C ABI sizes" {
    try std.testing.expectEqual(@as(usize, 232), @sizeOf(c.SpvkHello));
    try std.testing.expectEqual(@as(usize, 96), @sizeOf(c.SpvkPacket));
}

test "request cadence is ten hertz" {
    try std.testing.expectEqual(@as(u64, 100_000_000), request_period_ns);
}
