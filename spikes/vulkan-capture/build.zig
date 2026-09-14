const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{ .preferred_optimize_mode = .ReleaseFast });
    const vkroots = b.dependency("vkroots", .{});
    const vulkan_headers = b.dependency("vulkan_headers", .{});

    const layer_module = b.createModule(.{
        .target = target,
        .optimize = optimize,
        .link_libc = true,
        .link_libcpp = true,
        .pic = true,
    });
    layer_module.addIncludePath(vkroots.path(""));
    layer_module.addIncludePath(vulkan_headers.path("include"));
    layer_module.addIncludePath(b.path("src"));
    layer_module.addCSourceFile(.{
        .file = b.path("src/layer.cpp"),
        .flags = &.{
            "-std=c++20",
            "-fvisibility=hidden",
            "-Wno-nullability-completeness",
        },
    });
    const layer = b.addLibrary(.{
        .name = "scorepeek_vulkan_capture_spike",
        .linkage = .dynamic,
        .root_module = layer_module,
    });
    layer.setVersionScript(b.path("layer/exports.map"));
    b.installArtifact(layer);
    b.installFile(
        "layer/VkLayer_SCOREPEEK_capture_spike.json",
        "share/vulkan/explicit_layer.d/VkLayer_SCOREPEEK_capture_spike.json",
    );

    const consumer_module = b.createModule(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
        .link_libc = true,
        .link_libcpp = true,
    });
    consumer_module.addIncludePath(vulkan_headers.path("include"));
    consumer_module.addIncludePath(b.path("src"));
    consumer_module.addCSourceFile(.{
        .file = b.path("src/consumer_backend.cpp"),
        .flags = &.{
            "-std=c++20",
            "-Wno-nullability-completeness",
        },
    });
    const consumer = b.addExecutable(.{
        .name = "scorepeek-vulkan-capture-consumer",
        .root_module = consumer_module,
    });
    b.installArtifact(consumer);

    const tests = b.addTest(.{ .root_module = consumer_module });
    const run_tests = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run the consumer contract tests");
    test_step.dependOn(&run_tests.step);
}
