import AppKit
import ApplicationServices

// This persistent helper owns AppKit and idle timers on its main thread. It never activates an
// foreground app, warps the system cursor, or posts to the global HID event stream.
struct Failure: Error, CustomStringConvertible {
    let description: String
    init(_ message: String) { description = message }
}
func windows() -> [[String: Any]] {
    let entries = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] ?? []
    return entries.filter { ($0[kCGWindowLayer as String] as? Int) == 0 }.compactMap { w in
        guard let bounds = w[kCGWindowBounds as String] as? NSDictionary,
              let rect = CGRect(dictionaryRepresentation: bounds), rect.width > 1, rect.height > 1 else { return nil }
        return ["window_id": w[kCGWindowNumber as String] ?? 0,
                "pid": w[kCGWindowOwnerPID as String] ?? 0,
                "app": w[kCGWindowOwnerName as String] ?? "",
                "title": w[kCGWindowName as String] ?? "",
                "onscreen": w[kCGWindowIsOnscreen as String] ?? false,
                "x": rect.minX, "y": rect.minY, "width": rect.width, "height": rect.height]
    }
}
func state() -> [String: Any] {
    let p = CGEvent(source: nil)?.location ?? .zero
    return ["frontmost_pid": NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0,
            "frontmost_bundle": NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "",
            "cursor_x": p.x, "cursor_y": p.y,
            // Counts only: no key values or user text are recorded. These let
            // diagnostics distinguish concurrent human input from automation.
            "physical_left_down_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .leftMouseDown),
            "physical_right_down_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .rightMouseDown),
            "physical_key_down_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .keyDown),
            "front_window_id": windows().first(where: { $0["onscreen"] as? Bool == true })?["window_id"] ?? 0]
}
enum PointerPhase: String {
    case active, transparent, hidden
    static func at(idle: TimeInterval) -> PointerPhase {
        if idle >= 30 { return .hidden }
        if idle >= 10 { return .transparent }
        return .active
    }
}
func paintPointer(leftPressed: Bool, transparent: Bool) {
    let p = NSBezierPath()
    p.move(to: NSPoint(x: 2, y: 2))
    for point in [NSPoint(x: 3, y: 29), NSPoint(x: 10, y: 22), NSPoint(x: 16, y: 34), NSPoint(x: 22, y: 31), NSPoint(x: 16, y: 19), NSPoint(x: 27, y: 18)] { p.line(to: point) }
    p.close()
    // From 10 to 30 seconds only the outline remains; do not order the panel out.
    if !transparent {
        (leftPressed
            ? NSColor(calibratedRed: 37.0 / 255, green: 99.0 / 255, blue: 235.0 / 255, alpha: 1)
            : NSColor(calibratedRed: 125.0 / 255, green: 212.0 / 255, blue: 1, alpha: 1)).setFill()
        p.fill()
    }
    NSColor(calibratedWhite: 0.15, alpha: transparent ? 0.45 : 0.9).setStroke()
    p.lineWidth = 1.5
    p.stroke()
}
final class PointerView: NSView {
    var leftPressed = false { didSet { needsDisplay = true } }
    var transparent = false { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSGraphicsContext.current?.cgContext.clear(dirtyRect)
        paintPointer(leftPressed: leftPressed, transparent: transparent)
    }
}
final class PointerPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}
func pointerPanel() -> NSPanel {
    let panel = PointerPanel(contentRect: NSRect(x: 0, y: 0, width: 36, height: 40),
                             styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
    panel.isOpaque = false
    panel.backgroundColor = .clear
    panel.hasShadow = true
    panel.ignoresMouseEvents = true
    panel.hidesOnDeactivate = false
    panel.level = .floating
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle]
    panel.contentView = PointerView(frame: NSRect(x: 0, y: 0, width: 36, height: 40))
    return panel
}
// Lives across MCP/API actions. Only native input resets the monotonic idle
// clock; screenshots/windows are observations, never pointer activity.
final class VirtualPointer {
    private var panel: NSPanel?
    private var timers = [Timer]()
    private var touched: TimeInterval?
    private var enabled = true
    private(set) var windowID: Int?
    private(set) var ownerPID: Int32?
    private(set) var local: CGPoint?
    private(set) var leftPressed = false
    var idle: TimeInterval { touched.map { max(0, ProcessInfo.processInfo.systemUptime - $0) } ?? 30 }
    var phase: PointerPhase { enabled && local != nil ? PointerPhase.at(idle: idle) : .hidden }
    func activity(window: Int, pid: Int32, local point: CGPoint?, origin: CGPoint, show: Bool, pressed: Bool = false, presentationPoint: CGPoint? = nil) {
        if windowID != window || ownerPID != pid { local = nil }
        windowID = window; ownerPID = pid
        if let point = point { local = point }
        enabled = show; leftPressed = pressed
        touched = ProcessInfo.processInfo.systemUptime
        for timer in timers { timer.invalidate() }
        timers.removeAll()
        if enabled, let point = local {
            if panel == nil { panel = pointerPanel() }
            let top = NSScreen.screens.first?.frame.maxY ?? 0
            let screenPoint = presentationPoint ?? CGPoint(x: origin.x + point.x, y: origin.y + point.y)
            panel!.setFrameOrigin(NSPoint(x: screenPoint.x - 2, y: top - screenPoint.y - 38))
            for delay in [10.0, 30.0] {
                let timer = Timer(timeInterval: delay, repeats: false) { [weak self] _ in self?.refresh() }
                RunLoop.main.add(timer, forMode: .common)
                timers.append(timer)
            }
        }
        refresh()
    }
    func refresh() {
        guard let panel = panel else { return }
        if phase == .hidden { panel.orderOut(nil); return }
        if let view = panel.contentView as? PointerView {
            view.leftPressed = leftPressed
            view.transparent = phase == .transparent
        }
        panel.hasShadow = phase == .active
        panel.orderFrontRegardless()
        panel.displayIfNeeded()
    }
    func metadata() -> [String: Any] {
        var info: [String: Any] = ["phase": phase.rawValue, "visible": phase != .hidden,
            "fill_alpha": phase == .active ? 1.0 : 0.0, "idle_seconds": idle,
            "panel_visible": panel?.isVisible ?? false, "panel_window_id": panel?.windowNumber ?? 0,
            "left_pressed": leftPressed, "transparent_after_seconds": 10, "hide_after_seconds": 30]
        if let windowID = windowID { info["window_id"] = windowID }
        if let ownerPID = ownerPID { info["pid"] = ownerPID }
        if let local = local { info["x"] = local.x; info["y"] = local.y }
        return info
    }
}
let virtualPointer = VirtualPointer()

// Background input uses two process-addressed macOS paths and never posts to the
// global HID tap. SkyLight reaches Chromium/Catalyst surfaces that filter the
// public PID post; the public post is retained for AppKit mouse compatibility.
private let rtldDefault = UnsafeMutableRawPointer(bitPattern: -2)
private let skyLightHandle: UnsafeMutableRawPointer? = dlopen(
    "/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",
    RTLD_NOW | RTLD_GLOBAL
)
func nativeSymbol(_ name: String) -> UnsafeMutableRawPointer? {
    _ = skyLightHandle
    return dlsym(rtldDefault, name)
}
// Recipient-local responder lease. It sends no global activation request and
// never posts input to the global HID stream. Exact-window make-key records are
// needed so background text fields keep their native keyboard responder.
final class BackgroundResponder {
    typealias Lookup = @convention(c) (Int32, UnsafeMutablePointer<UInt32>) -> Int32
    typealias Post = @convention(c) (UnsafePointer<UInt32>, UnsafePointer<UInt8>) -> Int32
    private var psn = [UInt32](repeating: 0, count: 2)
    private var record = [UInt8](repeating: 0, count: 0xf8)
    private var post: Post?
    private var armed = false
    let wasBackground: Bool
    init(pid: Int32, window: Int) throws {
        wasBackground = NSWorkspace.shared.frontmostApplication?.processIdentifier != pid
        if !wasBackground { return }
        _ = skyLightHandle
        guard let lookupSymbol = nativeSymbol("GetProcessForPID"),
              let postSymbol = nativeSymbol("SLPSPostEventRecordTo") else {
            throw Failure("Background responder routing is unavailable on this macOS version; no desktop input was sent")
        }
        let lookup = unsafeBitCast(lookupSymbol, to: Lookup.self)
        let send = unsafeBitCast(postSymbol, to: Post.self)
        guard lookup(pid, &psn) == 0 else { throw Failure("Target process is unavailable") }
        record[4] = 0xf8; record[8] = 0x0d
        for i in 0..<4 { record[0x3c + i] = UInt8(truncatingIfNeeded: window >> (8 * i)) }
        record[0x8a] = 1
        guard send(psn, record) == 0 else { throw Failure("Cannot prepare the background window responder") }
        post = send; armed = true

        // Establish the exact NSWindow keyboard/text responder without asking
        // WindowServer to raise or foreground the application.
        var keyRecord = [UInt8](repeating: 0, count: 0xf8)
        keyRecord[4] = 0xf8; keyRecord[0x3a] = 0x10
        for i in 0..<4 { keyRecord[0x3c+i] = UInt8(truncatingIfNeeded: window >> (8*i)) }
        for i in 0x20..<0x30 { keyRecord[i] = 0xff }
        for kind: UInt8 in [1,2] {
            keyRecord[8] = kind
            guard send(psn,keyRecord) == 0 else {
                finish(); throw Failure("Cannot prepare the exact window responder")
            }
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.06))
    }
    func finish() {
        guard armed else { return }
        armed = false
        record[0x8a] = 2
        _ = post?(psn, record)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.06))
    }
    deinit { finish() }
}

func keyRecipe(_ key: String) throws -> (String, CGKeyCode, CGEventFlags) {
    var parts = key.lowercased().split(separator: "+", omittingEmptySubsequences: false).map(String.init)
    let name = parts.popLast() ?? ""
    let codes: [String: CGKeyCode] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,"1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"9":25,"7":26,"8":28,"0":29,"o":31,"u":32,"i":34,"p":35,"enter":36,"return":36,"l":37,"j":38,"k":40,"n":45,"m":46,"tab":48,"space":49,"backspace":51,"escape":53,"esc":53,"delete":117,"home":115,"end":119,"pageup":116,"pagedown":121,"left":123,"right":124,"down":125,"up":126]
    guard let code = codes[name] else { throw Failure("Unsupported key; use type for text") }
    var flags: CGEventFlags = []
    for modifier in parts {
        switch modifier {
        case "cmd", "command", "super", "meta": flags.insert(.maskCommand)
        case "ctrl", "control": flags.insert(.maskControl)
        case "alt", "option": flags.insert(.maskAlternate)
        case "shift": flags.insert(.maskShift)
        default: throw Failure("Unknown key modifier")
        }
    }
    return (name, code, flags)
}

func run(_ a: [String: Any]) throws -> [String: Any] {
    let action = a["action"] as? String ?? ""
    if let mode = a["mode"] as? String, mode != "background" {
        throw Failure("macOS permits background window control only; no desktop input was sent")
    }
    if action == "browser_open" { return try openManagedBrowser(a) }
    if action == "windows" {
        let available = windows().map { window -> [String: Any] in
            var value = window
            if let id = window["window_id"] as? Int, let pid = window["pid"] as? Int32,
               (try? managedBrowser(a, id: id, pid: pid)) != nil { value["background_input"] = "chrome-devtools" }
            else if window["app"] as? String == "Google Chrome" { value["background_input"] = "requires-browser-open; unmanaged Chrome is read-only" }
            else { value["background_input"] = "native-window" }
            return value
        }
        return ["windows": available, "accessibility": AXIsProcessTrusted(), "screen_recording": CGPreflightScreenCaptureAccess(), "desktop": state(), "pointer": virtualPointer.metadata(), "helper_pid": ProcessInfo.processInfo.processIdentifier]
    }
    guard let id = a["window_id"] as? Int, id > 0,
          let w = windows().first(where: { ($0["window_id"] as? Int) == id }),
          let pid = w["pid"] as? Int32 else { throw Failure("Target window is unavailable; call windows and select a window_id") }
    guard let expected = a["pid"] as? Int32, expected == pid else { throw Failure("Missing or changed window owner; select the target again") }
    // The isolated Chrome channel addresses a verified page directly, not the
    // OS responder. Its native window capture also works on another Space.
    // Keep the visibility restriction for native PID input, never substitute
    // native/global input when a managed endpoint is unavailable.
    let controlledBrowser = try managedBrowser(a, id: id, pid: pid)
    if action != "screenshot", controlledBrowser == nil,
       (w["app"] as? String == "Google Chrome" || NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "com.google.Chrome") {
        throw Failure("Unmanaged Chrome is read-only. Use browser_open, then its window_id and pid. No input was sent")
    }

    let targetWindow = try controlledBrowser.map { try managedWindowGeometry($0, native: w) } ?? w
    guard (w["onscreen"] as? Bool) == true || controlledBrowser != nil else {
        throw Failure("Native target must be on the current desktop and not minimized; it may be behind other windows")
    }
    let width = (targetWindow["width"] as! NSNumber).doubleValue, height = (targetWindow["height"] as! NSNumber).doubleValue
    let origin = CGPoint(x: (targetWindow["x"] as! NSNumber).doubleValue, y: (targetWindow["y"] as! NSNumber).doubleValue)
    if action == "screenshot" {
        guard let path = a["capture_path"] as? String else { throw Failure("Missing capture path") }
        guard CGPreflightScreenCaptureAccess() else { throw Failure("Enable Screen Recording for window capture") }
        // Retry only this read-only observation during a Space transition.
        // The preceding input is never repeated.
        var captured: NSBitmapImageRep?
        if let browser = controlledBrowser {
            let image = try browserCapture(browser, window: targetWindow)
            guard let data = image.representation(using: .png, properties: [:]) else { throw Failure("Cannot encode browser observation") }
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            captured = image
        } else {
        for attempt in 0..<6 {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
            process.arguments = ["-x", "-o", "-l", String(id), "-t", "png", path]
            process.standardError = FileHandle.nullDevice
            try process.run(); process.waitUntilExit()
            if process.terminationStatus == 0,
               let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
               let image = NSBitmapImageRep(data: data) {
                captured = image
                break
            }
            if attempt < 5 { RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.3)) }
        }
        }
        guard let image = captured else {
            throw Failure("Window capture unavailable during a desktop transition; request another screenshot. No input was repeated")
        }
        // Window capture excludes our separate overlay panel. Paint its last
        // window-local position into the returned image for the agent viewer.
        let overlay = a["show_pointer"] as? Bool != false && virtualPointer.windowID == id && virtualPointer.ownerPID == pid && virtualPointer.phase != .hidden
        if overlay, let pointer = virtualPointer.local {
            let x = pointer.x, y = pointer.y
            let size = NSSize(width: image.pixelsWide, height: image.pixelsHigh)
            guard let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: image.pixelsWide,
                pixelsHigh: image.pixelsHigh, bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0),
                let context = NSGraphicsContext(bitmapImageRep: bitmap), let cgImage = image.cgImage else {
                throw Failure("Cannot render virtual pointer in screenshot")
            }
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = context
            NSImage(cgImage: cgImage, size: size).draw(in: NSRect(origin: .zero, size: size))
            let transform = NSAffineTransform()
            transform.translateX(by: x * size.width / width - 2 * size.width / width,
                                 yBy: size.height - y * size.height / height + 2 * size.height / height)
            transform.scaleX(by: size.width / width, yBy: -size.height / height)
            transform.concat()
            paintPointer(leftPressed: virtualPointer.leftPressed, transparent: virtualPointer.phase == .transparent)
            NSGraphicsContext.restoreGraphicsState()
            if let png = bitmap.representation(using: .png, properties: [:]) {
                try png.write(to: URL(fileURLWithPath: path))
            }
        }
        return ["path": path, "mime": "image/png", "window_id": id, "pid": pid, "screen_width": image.pixelsWide, "screen_height": image.pixelsHigh, "logical_width": width, "logical_height": height, "coordinate_space": "window", "pointer_overlay": overlay, "pointer": virtualPointer.metadata(), "desktop": state(), "capture_backend": controlledBrowser == nil ? "native-window" : "browser-surface-window-frame", "browser_chrome_captured": controlledBrowser == nil]
    }
    guard AXIsProcessTrusted() else { throw Failure("Enable Accessibility permission for the process running Lessagent") }
    guard let locationSymbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGEventSetWindowLocation") else {
        throw Failure("This macOS version lacks background window event routing; no desktop input was sent")
    }
    typealias SetWindowLocation = @convention(c) (CGEvent, CGPoint) -> Void
    let setWindowLocation = unsafeBitCast(locationSymbol, to: SetWindowLocation.self)
    guard let source = CGEventSource(stateID: .privateState) else { throw Failure("Cannot create an isolated input source; no input was sent") }
    func point(_ x: String, _ y: String) throws -> CGPoint {
        guard let px = a[x] as? Double, let py = a[y] as? Double, px.isFinite, py.isFinite else { throw Failure("Missing numeric \(x), \(y)") }
        let sw = a["screen_width"] as? Double ?? width, sh = a["screen_height"] as? Double ?? height
        guard sw.isFinite, sh.isFinite, sw > 0, sh > 0 else { throw Failure("Invalid screenshot dimensions") }
        let local = CGPoint(x: px * width / sw, y: py * height / sh)
        guard local.x >= 0, local.y >= 0, local.x < width, local.y < height else { throw Failure("Coordinate outside target window") }
        return CGPoint(x: origin.x + local.x, y: origin.y + local.y)
    }
    guard ["move", "click", "drag", "type", "key", "scroll"].contains(action) else { throw Failure("Unknown background computer action") }
    var points = [CGPoint]()
    if ["move", "click", "drag", "scroll"].contains(action) {
        points = [try point("x", "y")]
        if action == "drag" {
            if let path = a["path"] as? [[Double]] {
                guard path.count >= 2, path.count <= 512 else { throw Failure("path needs 2 to 512 points") }
                let sw = a["screen_width"] as? Double ?? width, sh = a["screen_height"] as? Double ?? height
                guard sw.isFinite, sh.isFinite, sw > 0, sh > 0 else { throw Failure("Invalid screenshot dimensions") }
                points = try path.map { pair in
                    guard pair.count == 2, pair.allSatisfy({ $0.isFinite }), pair[0] >= 0, pair[1] >= 0, pair[0] < sw, pair[1] < sh else { throw Failure("Invalid path coordinate") }
                    return CGPoint(x: origin.x + pair[0] * width / sw, y: origin.y + pair[1] * height / sh)
                }
            } else { points.append(try point("to_x", "to_y")) }
        }
    }
    let recipe = action == "key" ? try keyRecipe(a["key"] as? String ?? "") : nil
    if action == "type" { guard let text = a["text"] as? String, text.utf16.count <= 16384 else { throw Failure("Invalid text") } }
    if action == "scroll" { guard a["delta"] as? Int32 != nil else { throw Failure("Missing delta") } }
    if let browser = controlledBrowser {
        return try browserControl(a, record: browser, window: targetWindow, points: points, recipe: recipe)
    }
    let before = state()
    let responder = try BackgroundResponder(pid: pid, window: id)
    defer { responder.finish() }
    var pointer: CGPoint?
    func show(_ p: CGPoint, pressed: Bool = false) {
        pointer = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
        virtualPointer.activity(window: id, pid: pid, local: pointer, origin: origin,
                                show: a["show_pointer"] as? Bool != false, pressed: pressed)
    }
    func keyboardActivity() {
        virtualPointer.activity(window: id, pid: pid, local: nil, origin: origin,
                                show: a["show_pointer"] as? Bool != false)
    }
    typealias SetIntegerField = @convention(c) (CGEvent, UInt32, Int64) -> Void
    guard let integerSymbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "SLEventSetIntegerValueField") else {
        throw Failure("Background event stamping is unavailable; no desktop input was sent")
    }
    let setIntegerField = unsafeBitCast(integerSymbol, to: SetIntegerField.self)
    let group = Int64.random(in: 1...Int64(Int32.max))
    var eventsPosted = 0
    var usedSkyLight = false
    func stamp(_ e: CGEvent, _ field: UInt32, _ value: Int64) {
        // Public CG setters do not expose every WindowServer routing field.
        setIntegerField(e, field, value)
    }
    func stampMouseRoute(_ e: CGEvent) {
        stamp(e, 40, Int64(pid))
        stamp(e, 51, Int64(id))
        stamp(e, 58, group)
        stamp(e, 91, Int64(id))
        stamp(e, 92, Int64(id))
    }
    func postMouse(_ event: CGEvent?) throws {
        guard let event else { throw Failure("Cannot create mouse event") }
        stampMouseRoute(event)
        // One exact process-directed post. It never enters the global HID tap,
        // so the physical/system pointer is not used as a fallback.
        event.postToPid(pid)
        eventsPosted += 1
    }
    func postKeyboard(_ event: CGEvent?) throws {
        guard let event else { throw Failure("Cannot create keyboard event") }
        // Public PID keyboard delivery preserves Unicode text and native/menu
        // key semantics on the supported macOS builds. The exact-window responder
        // lease above makes it background-safe; there is never a global fallback.
        event.postToPid(pid)
        eventsPosted += 1
    }
    let right = a["button"] as? String == "right"
    let button: CGMouseButton = right ? .right : .left
    func mouse(_ type: CGEventType, _ p: CGPoint) throws {
        let local = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
        let pressed = type == .leftMouseDown || type == .rightMouseDown || type == .leftMouseDragged || type == .rightMouseDragged
        let moved = type == .mouseMoved
        guard let e = CGEvent(mouseEventSource: source, mouseType: type,
            mouseCursorPosition: p, mouseButton: button) else {
            throw Failure("Cannot create mouse event")
        }
        setWindowLocation(e, local)
        e.flags = [] // An ordinary click must never inherit physically-held modifiers.
        stamp(e, 0, moved ? 2 : 3)
        stamp(e, 1, moved ? 0 : 1)
        stamp(e, 3, Int64(button.rawValue))
        stamp(e, 7, action == "drag" || moved ? 0 : 3)
        e.setDoubleValueField(.mouseEventPressure, value: pressed ? 1 : 0)
        try postMouse(e)
        show(p, pressed: type == .leftMouseDown || type == .leftMouseDragged)
    }
    switch action {
    case "move":
        try mouse(.mouseMoved, points[0])
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
    case "click":
        // Exact background click: prime target hit-testing with one move, then
        // send one real down/up pair. No off-screen synthetic click is inserted.
        try mouse(.mouseMoved, points[0])
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
        let down: CGEventType = right ? .rightMouseDown : .leftMouseDown
        let up: CGEventType = right ? .rightMouseUp : .leftMouseUp
        try mouse(down, points[0])
        var released = false
        defer { if !released { try? mouse(up, points[0]) } }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.028))
        try mouse(up, points[0])
        released = true
    case "drag":
        // Drag is one continuous button gesture. Do not inject a primer between
        // the down and dragged events; doing so was the source of background
        // sliders seeing buttons=0 during motion.
        try mouse(.mouseMoved, points[0])
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
        let down: CGEventType = right ? .rightMouseDown : .leftMouseDown
        let up: CGEventType = right ? .rightMouseUp : .leftMouseUp
        try mouse(down, points[0])
        var last = points[0]
        var released = false
        defer { if !released { try? mouse(up, last) } }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.016))
        let duration = min(5, max(0.05, a["duration"] as? Double ?? (a["duration_ms"] as? Double ?? 500) / 1000))
        let lengths = zip(points, points.dropFirst()).map { hypot($1.x - $0.x, $1.y - $0.y) }
        let total = max(1, lengths.reduce(0, +))
        for i in 1..<points.count {
            let segmentTime = max(1.0 / 120, duration * lengths[i - 1] / total)
            let steps = max(1, min(Int(ceil(segmentTime * 120)), Int(ceil(lengths[i - 1] / 4))))
            for step in 1...steps {
                let f = Double(step) / Double(steps)
                last = CGPoint(
                    x: points[i - 1].x + (points[i].x - points[i - 1].x) * f,
                    y: points[i - 1].y + (points[i].y - points[i - 1].y) * f
                )
                try mouse(right ? .rightMouseDragged : .leftMouseDragged, last)
                RunLoop.current.run(until: Date(timeIntervalSinceNow: segmentTime / Double(steps)))
            }
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
        try mouse(up, last)
        released = true
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    case "type":
        guard let text = a["text"] as? String, text.utf16.count <= 16384 else {
            throw Failure("Missing text or text exceeds 16384 UTF-16 units")
        }
        // Small chunks avoid CGEvent's Unicode payload limit while preserving
        // surrogate pairs/emoji. Keyboard events use the authenticated SkyLight path.
        for character in text {
            let units = Array(String(character).utf16)
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down)
                e?.flags = []
                units.withUnsafeBufferPointer {
                    e?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress)
                }
                try postKeyboard(e)
                keyboardActivity()
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.008))
            }
        }
    case "key":
        let (name, code, flags) = recipe!
        for down in [true, false] {
            let e = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down)
            e?.flags = flags
            // Shortcut key equivalents keep hardware identity; plain printable
            // keys include Unicode for native text-input clients.
            if name.utf16.count == 1 && flags.intersection([.maskCommand, .maskControl, .maskAlternate]).isEmpty {
                let units = Array((flags.contains(.maskShift) ? name.uppercased() : name).utf16)
                units.withUnsafeBufferPointer {
                    e?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress)
                }
            }
            try postKeyboard(e)
            keyboardActivity()
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.008))
        }
    case "scroll":
        let p = points[0]
        guard let delta = a["delta"] as? Int32 else { throw Failure("Missing delta") }
        try mouse(.mouseMoved, p)
        let e = CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 1,
                        wheel1: -max(-100, min(100, delta)), wheel2: 0, wheel3: 0)
        e?.location = p
        e?.flags = []
        if let e {
            setWindowLocation(e, CGPoint(x: p.x - origin.x, y: p.y - origin.y))
            try postMouse(e)
        }
    default:
        throw Failure("Unknown background computer action")
    }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.04))
    responder.finish()
    var result: [String: Any] = ["ok": true, "action": action, "mode": "background", "window_id": id, "pid": pid, "coordinate_space": "window", "pointer_color": "#7DD4FF", "pointer_pressed_color": "#2563EB", "before": before, "after": state(), "input_events_posted": eventsPosted, "delivery": "process-window", "responder_lease": responder.wasBackground, "pointer": virtualPointer.metadata()]
    if let pointer = pointer { result["virtual_pointer"] = ["x": pointer.x, "y": pointer.y] }
    return result
}
// Self-tests exercise the production clock boundaries without Accessibility.
if CommandLine.arguments.contains("--self-test") {
    for (idle, expected) in [(0.0, PointerPhase.active), (9.999, .active), (10.0, .transparent), (29.999, .transparent), (30.0, .hidden), (300.0, .hidden)] {
        precondition(PointerPhase.at(idle: idle) == expected, "Idle phase at \(idle)")
    }
    print("PASS pointer phase boundaries: 0 / 9.999 / 10 / 29.999 / 30 / 300 seconds")
    exit(0)
}
let app = NSApplication.shared
// Accessory permits the nonactivating pointer panel without a Dock icon.
// Prohibited forbids window creation and can leave an apparently visible but
// completely blank panel. We never activate this app or make its panel key.
app.setActivationPolicy(.accessory)
func reply(_ data: Data) {
    autoreleasepool {
        let result: [String: Any]
        do {
            guard let args = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw Failure("Expected JSON object") }
            result = try run(args)
        } catch { result = ["error": String(describing: error)] }
        if let json = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
            FileHandle.standardOutput.write(json)
            FileHandle.standardOutput.write(Data([10]))
        }
    }
}
if CommandLine.arguments.contains("--server") {
    // The reader never runs AppKit. One main-thread command completes before
    // the next can enter, including while run() services its nested run loop.
    DispatchQueue.global(qos: .userInitiated).async {
        while let line = readLine() {
            DispatchQueue.main.sync { reply(Data(line.utf8)) }
        }
        DispatchQueue.main.async {
            // Only the extracted, private helper opts into cleanup. Never delete
            // a source-tree build when developers run the helper directly.
            if CommandLine.arguments.contains("--cleanup") {
                let directory = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
                if directory.lastPathComponent.hasPrefix("lessagent-computer-") {
                    try? FileManager.default.removeItem(at: directory)
                }
            }
            exit(0) // parent pipe closed: no orphan overlay
        }
    }
    app.run()
} else {
    reply(FileHandle.standardInput.readDataToEndOfFile())
}
