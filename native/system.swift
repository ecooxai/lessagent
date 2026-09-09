import AppKit
import ApplicationServices
import Metal
import Darwin

// Read-only host facts for instruction.md; no personal or session identifiers.
private func systemText(_ name: String) -> String? {
    var size = 0
    guard sysctlbyname(name, nil, &size, nil, 0) == 0, size > 0, size < 4096 else { return nil }
    var bytes = [CChar](repeating: 0, count: size)
    guard sysctlbyname(name, &bytes, &size, nil, 0) == 0 else { return nil }
    return String(cString: bytes)
}
private func systemInteger(_ name: String) -> Int? {
    var value: Int32 = 0, size = MemoryLayout<Int32>.size
    guard sysctlbyname(name, &value, &size, nil, 0) == 0 else { return nil }
    return Int(value)
}
func primaryScreen() -> NSScreen? {
    NSScreen.screens.first { ($0.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value == CGMainDisplayID() }
}
func hostSystemInfo() -> [String: Any] {
    let process = ProcessInfo.processInfo
    var cpu: [String: Any] = ["model": systemText("machdep.cpu.brand_string") ?? "Unavailable",
                              "logical_cores": process.processorCount, "active_logical_cores": process.activeProcessorCount]
    if let physical = systemInteger("hw.physicalcpu") { cpu["physical_cores"] = physical }
    let displays: [[String: Any]] = NSScreen.screens.compactMap { screen in
        guard let id = (screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber)?.uint32Value else { return nil }
        let frame = screen.frame, visible = screen.visibleFrame
        let mode = CGDisplayCopyDisplayMode(id)
        var display: [String: Any] = ["id": id, "name": screen.localizedName, "primary": id == CGMainDisplayID(),
            "logical_width": Int(frame.width), "logical_height": Int(frame.height),
            "backing_scale_factor": screen.backingScaleFactor,
            "visible_width": Int(visible.width), "visible_height": Int(visible.height),
            "logical_origin_x": frame.minX, "logical_origin_y": frame.minY]
        if let mode = mode {
            display["pixel_width"] = mode.pixelWidth; display["pixel_height"] = mode.pixelHeight
        }
        return display
    }
    let gpus: [[String: Any]] = MTLCopyAllDevices().map { gpu in
        ["name": gpu.name, "unified_memory": gpu.hasUnifiedMemory,
         "recommended_max_working_set_bytes": gpu.recommendedMaxWorkingSetSize]
    }
    return ["os_version": process.operatingSystemVersionString, "cpu": cpu,
        "ram": ["total_bytes": process.physicalMemory], "gpus": gpus,
        "gpu_note": "Metal-visible devices. Recommended working-set size is a budget, not dedicated VRAM; unified memory shares system RAM.",
        "displays": displays,
        "display_note": "Window dimensions use logical points. Pixel dimensions are current display-mode backing pixels, not a promise about a window screenshot; use the dimensions encoded in that image.",
        "permissions": ["accessibility": AXIsProcessTrusted(), "screen_recording": CGPreflightScreenCaptureAccess()]]
}
