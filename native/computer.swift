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
    return entries.filter {
        ($0[kCGWindowLayer as String] as? Int) == 0 ||
        ($0[kCGWindowOwnerName as String] as? String == "lessagent" &&
         $0[kCGWindowName as String] as? String == "Lessagent service alert")
    }.compactMap { w in
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
    let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? 0
    return ["frontmost_pid": frontmost,
            "frontmost_bundle": NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "",
            "focused_window_id": nativeFocusedWindowID(frontmost),
            "cursor_x": p.x, "cursor_y": p.y,
            // Counts only: no key values or user text are recorded. These let
            // diagnostics distinguish concurrent human input from automation.
            "physical_left_down_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .leftMouseDown),
            "physical_right_down_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .rightMouseDown),
            "physical_key_down_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .keyDown),
            "physical_mouse_move_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .mouseMoved),
            "physical_left_drag_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .leftMouseDragged),
            "physical_right_drag_count": CGEventSource.counterForEventType(.hidSystemState, eventType: .rightMouseDragged),
            "front_window_id": windows().first(where: { $0["onscreen"] as? Bool == true && $0["pid"] as? Int32 == frontmost })?["window_id"] ?? 0]
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
    panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle, .stationary]
    if #available(macOS 13.0, *) {
        // Remain an overlay, not a hidden member of the background app's Stage Manager set.
        panel.collectionBehavior.insert(.canJoinAllApplications)
    }
    panel.animationBehavior = .none
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

// Blender refreshes its event-state cursor from the physical cursor after modal
// transforms. Restore the last *virtual*, exact-window position before keys so
// subsequent shortcuts retain their editor context. Never query/warp HID here.
struct NativePointerContext {
    let pid: Int32
    let local: CGPoint
    let size: CGSize
}
var nativePointerContexts: [Int: NativePointerContext] = [:]
func rememberNativePointer(id: Int, pid: Int32, local: CGPoint, size: CGSize) {
    if nativePointerContexts.count >= 128 {
        let live = windows()
        nativePointerContexts = nativePointerContexts.filter { key, value in
            live.contains { $0["window_id"] as? Int == key && $0["pid"] as? Int32 == value.pid }
        }
        if nativePointerContexts.count >= 128 { nativePointerContexts.removeAll() }
    }
    nativePointerContexts[id] = NativePointerContext(pid: pid, local: local, size: size)
}


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
func keyRecipe(_ key: String) throws -> (String, CGKeyCode, CGEventFlags) {
    var parts = key.lowercased().split(separator: "+", omittingEmptySubsequences: false).map(String.init)
    let name = parts.popLast() ?? ""
    let codes: [String: CGKeyCode] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,"1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"9":25,"7":26,"8":28,"0":29,"o":31,"u":32,"i":34,"p":35,"enter":36,"return":36,"l":37,"j":38,"k":40,"n":45,"m":46,"tab":48,"space":49,"backspace":51,"escape":53,"esc":53,"delete":117,"home":115,"end":119,"pageup":116,"pagedown":121,"left":123,"right":124,"down":125,"up":126,"f1":122,"f2":120,"f3":99,"f4":118,"f5":96,"f6":97,"f7":98,"f8":100,"f9":101,"f10":109,"f11":103,"f12":111,"f13":105,"f14":107,"f15":113,"f16":106,"f17":64,"f18":79,"f19":80,"f20":90]
    let extraCodes: [String: CGKeyCode] = [
        "minus":27,"-":27,"equal":24,"=":24,"period":47,".":47,"comma":43,",":43,
        "slash":44,"/":44,"semicolon":41,";":41,"quote":39,"'":39,
        "leftbracket":33,"[":33,"rightbracket":30,"]":30,"backslash":42,"grave":50,
        "numpad0":82,"numpad1":83,"numpad2":84,"numpad3":85,"numpad4":86,
        "numpad5":87,"numpad6":88,"numpad7":89,"numpad8":91,"numpad9":92,
        "numpaddecimal":65,"numpadadd":69,"numpadsubtract":78,"numpadmultiply":67,
        "numpaddivide":75,"numpadenter":76,"numpadequal":81
    ]
    guard let code = codes[name] ?? extraCodes[name] else { throw Failure("Unsupported key; use a named key or type for text") }
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
    if name.hasPrefix("numpad") { flags.insert(.maskNumericPad) }
    return (name, code, flags)
}

func run(_ a: [String: Any]) throws -> [String: Any] {
    let action = a["action"] as? String ?? ""
    if let mode = a["mode"] as? String, mode != "background" {
        throw Failure("macOS permits background window control only; no desktop input was sent")
    }
    if action == "system_info" { return hostSystemInfo() }
    if action == "browser_open" { return try openManagedBrowser(a) }
    if action == "app_open" { return try openNativeApplication(a) }
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
    // Exact native window routing does not depend on the Stage Manager shelf
    // being visible. AX validation below still rejects minimized input targets.
    let controlledBrowser = try managedBrowser(a, id: id, pid: pid)
    if action != "screenshot", controlledBrowser == nil,
       (w["app"] as? String == "Google Chrome" || NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "com.google.Chrome") {
        throw Failure("Unmanaged Chrome is read-only. Use browser_open, then its window_id and pid. No input was sent")
    }

    let targetWindow = try controlledBrowser.map { try managedWindowGeometry($0, native: w) } ?? nativeWindowGeometry(w, required: action != "screenshot")
    let width = (targetWindow["width"] as! NSNumber).doubleValue, height = (targetWindow["height"] as! NSNumber).doubleValue
    let origin = CGPoint(x: (targetWindow["x"] as! NSNumber).doubleValue, y: (targetWindow["y"] as! NSNumber).doubleValue)
    if action == "screenshot" {
        guard let path = a["capture_path"] as? String else { throw Failure("Missing capture path") }
        guard CGPreflightScreenCaptureAccess() else { throw Failure("Enable Screen Recording for window capture") }
        // Retry only this read-only observation during a Space transition.
        // The preceding input is never repeated.
        var captured: NSBitmapImageRep?
        var thumbnail = false
        if let browser = controlledBrowser {
            let image = try browserCapture(browser, window: targetWindow)
            guard let data = image.representation(using: .png, properties: [:]) else { throw Failure("Cannot encode browser observation") }
            try data.write(to: URL(fileURLWithPath: path), options: .atomic)
            captured = image
        } else if #available(macOS 14.0, *) {
            let (image, lowResolution) = try captureNativeWindow(id, pid: pid, geometry: targetWindow)
            thumbnail = lowResolution
            guard let data = image.representation(using: .png, properties: [:]) else { throw Failure("Cannot encode native observation") }
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
        return ["path": path, "mime": "image/png", "window_id": id, "pid": pid, "screen_width": image.pixelsWide, "screen_height": image.pixelsHigh, "logical_width": width, "logical_height": height, "coordinate_space": "window", "pointer_overlay": overlay, "pointer": virtualPointer.metadata(), "desktop": state(), "window_presentation": w, "geometry_source": targetWindow["geometry_source"] ?? "chrome-devtools", "geometry_attempts": targetWindow["geometry_attempts"] ?? 1, "capture_quality": thumbnail ? "thumbnail" : "full-resolution", "perspective_corrected": thumbnail, "capture_backend": controlledBrowser == nil ? (thumbnail ? "native-window-rectified-thumbnail" : "native-window") : "browser-surface-window-frame", "browser_chrome_captured": controlledBrowser == nil]
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
    if ["key", "type"].contains(action) { keyboardNotice.begin(a, pid: pid, windowID: id) }
    if let browser = controlledBrowser {
        return try browserControl(a, record: browser, window: targetWindow, points: points, recipe: recipe)
    }
    let isBlender = NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "org.blenderfoundation.blender"
    var keyboardContext: CGPoint?
    if isBlender && (action == "key" || action == "type") {
        guard let saved = nativePointerContexts[id], saved.pid == pid,
              abs(saved.size.width - width) <= 1, abs(saved.size.height - height) <= 1,
              saved.local.x >= 0, saved.local.y >= 0,
              saved.local.x < width, saved.local.y < height else {
            throw Failure("Blender keyboard context is unavailable or resized. Use virtual_pointer move/click in the intended editor with the current screenshot first; no keyboard input was sent")
        }
        keyboardContext = saved.local
    }
    let pointerModifiers = a["modifiers"] as? [String] ?? []
    var pointerFlags: CGEventFlags = []
    for modifier in pointerModifiers {
        switch modifier {
        case "shift": pointerFlags.insert(.maskShift)
        case "ctrl": pointerFlags.insert(.maskControl)
        case "alt": pointerFlags.insert(.maskAlternate)
        case "cmd": pointerFlags.insert(.maskCommand)
        default: throw Failure("Invalid pointer modifier; no input was sent")
        }
    }
    let before = state()
    // Input already carries an exact native window. Synthetic app-activation
    // records expand Stage Manager windows and are not background-safe.
    let wasBackground = NSWorkspace.shared.frontmostApplication?.processIdentifier != pid
    var pointer: CGPoint?
    func show(_ p: CGPoint, pressed: Bool = false) {
        pointer = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
        rememberNativePointer(id: id, pid: pid, local: pointer!, size: CGSize(width: width, height: height))
        virtualPointer.activity(window: id, pid: pid, local: pointer, origin: origin,
                                show: a["show_pointer"] as? Bool != false, pressed: pressed,
                                presentationPoint: CGPoint(x: (w["x"] as! NSNumber).doubleValue + pointer!.x * (w["width"] as! NSNumber).doubleValue / width,
                                                           y: (w["y"] as! NSNumber).doubleValue + pointer!.y * (w["height"] as! NSNumber).doubleValue / height))
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
    var accessibilityAction: [String: Any]?
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
        stampMouseRoute(event) // Cocoa/GHOST needs NSEvent.window, not only a PID.
        // Preserve the explicitly constructed Cocoa window association. No
        // synthetic activation/responder lease and no global fallback.
        event.postToPid(pid)
        eventsPosted += 1
        if ["key", "type"].contains(action) {
            if event.type == .keyDown { keyboardNotice.record("down") }
            if event.type == .keyUp { keyboardNotice.record("up") }
        }
    }
    func keyEvent(code: CGKeyCode, down: Bool, flags: CGEventFlags, text: String, kind: NSEvent.EventType? = nil) throws -> CGEvent {
        let event = NSEvent.keyEvent(with: kind ?? (down ? .keyDown : .keyUp), location: .zero,
            modifierFlags: NSEvent.ModifierFlags(rawValue: UInt(flags.rawValue)),
            timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: id,
            context: nil, characters: text, charactersIgnoringModifiers: text,
            isARepeat: false, keyCode: code)
        guard let result = event?.cgEvent else { throw Failure("Cannot create exact-window keyboard event") }
        return result
    }
    func modifierEvent(_ flags: CGEventFlags, code: CGKeyCode) throws {
        let event = try keyEvent(code: code, down: true, flags: flags, text: "", kind: .flagsChanged)
        event.flags = flags
        try postKeyboard(event)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.005))
    }
    let right = a["button"] as? String == "right"
    let middle = a["button"] as? String == "middle"
    let button: CGMouseButton = middle ? .center : (right ? .right : .left)
    let downType: CGEventType = middle ? .otherMouseDown : (right ? .rightMouseDown : .leftMouseDown)
    let upType: CGEventType = middle ? .otherMouseUp : (right ? .rightMouseUp : .leftMouseUp)
    let dragType: CGEventType = middle ? .otherMouseDragged : (right ? .rightMouseDragged : .leftMouseDragged)
    func mouse(_ type: CGEventType, _ p: CGPoint) throws {
        let local = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
        let pressed = type == .leftMouseDown || type == .rightMouseDown || type == .leftMouseDragged || type == .rightMouseDragged || type == .otherMouseDown || type == .otherMouseDragged
        let moved = type == .mouseMoved
        guard let e = CGEvent(mouseEventSource: source, mouseType: type,
            mouseCursorPosition: p, mouseButton: button) else {
            throw Failure("Cannot create mouse event")
        }
        setWindowLocation(e, local)
        e.flags = pointerFlags // Only explicit virtual modifiers; never inherit HID flags.
        stamp(e, 0, moved ? 2 : 3)
        stamp(e, 1, moved ? 0 : 1)
        stamp(e, 3, Int64(button.rawValue))
        stamp(e, 7, action == "drag" || moved ? 0 : 3)
        e.setDoubleValueField(.mouseEventPressure, value: pressed ? 1 : 0)
        try postMouse(e)
        show(p, pressed: type == .leftMouseDown || type == .leftMouseDragged)
    }
    // A context-only motion is sent once before the keyboard request. No click,
    // app activation, or change to the human pointer is involved. Other native
    // apps and Chrome keep their existing input paths.
    if let local = keyboardContext {
        try mouse(.mouseMoved, CGPoint(x: origin.x + local.x, y: origin.y + local.y))
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
    }
    let pointerModifierKeys: [(CGEventFlags, CGKeyCode)] = [(.maskShift,56),(.maskControl,59),(.maskAlternate,58),(.maskCommand,55)]
    var heldPointerModifiers: [(CGEventFlags, CGKeyCode)] = []
    var activePointerFlags: CGEventFlags = []
    func releasePointerModifiers() throws {
        while let (flag, code) = heldPointerModifiers.last {
            activePointerFlags.remove(flag)
            try modifierEvent(activePointerFlags, code: code)
            heldPointerModifiers.removeLast()
        }
    }
    defer { try? releasePointerModifiers() }
    if ["move", "click", "drag", "scroll"].contains(action) {
        for (flag, code) in pointerModifierKeys where pointerFlags.contains(flag) {
            activePointerFlags.insert(flag)
            heldPointerModifiers.append((flag, code))
            try modifierEvent(activePointerFlags, code: code)
        }
    }
    switch action {
    case "move":
        try mouse(.mouseMoved, points[0])
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
    case "click":
        // WebKit/Catalyst-style controls can ignore process-addressed CGEvents
        // while backgrounded. Prefer exact-window AXPress for an ordinary left
        // click on a semantic control; canvases/viewports and right-clicks keep
        // the existing process-window event path. Never send both paths for one
        // user click.
        if !right && !middle && pointerFlags.isEmpty, let semantic = nativeAccessibilityPress(w, at: points[0]) {
            accessibilityAction = semantic
            show(points[0])
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.04))
        } else {
            // Exact background click: prime target hit-testing with one move,
            // then send one real down/up pair. No off-screen click is inserted.
            try mouse(.mouseMoved, points[0])
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
            let down = downType
            let up = upType
            try mouse(down, points[0])
            var released = false
            defer { if !released { try? mouse(up, points[0]) } }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.028))
            try mouse(up, points[0])
            released = true
        }
    case "drag":
        // Drag is one continuous button gesture. Do not inject a primer between
        // the down and dragged events; doing so was the source of background
        // sliders seeing buttons=0 during motion.
        try mouse(.mouseMoved, points[0])
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
        let down = downType
        let up = upType
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
                try mouse(dragType, last)
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
        // surrogate pairs/emoji. Keyboard events keep their exact Cocoa window number.
        try modifierEvent([], code: 56) // Clear recipient-local cached modifiers, not HID state.
        var textShiftHeld = false
        defer { if textShiftHeld { try? modifierEvent([], code: 56) } }
        for character in text {
            let scalarText = String(character)
            let shifted: [String:String] = ["!":"1","@":"2","#":"3","$":"4","%":"5","^":"6","&":"7","*":"8","(":"9",")":"0","_":"minus","+":"equal","<":"comma",">":"period","?":"slash",":":"semicolon","\"":"quote","{":"leftbracket","}":"rightbracket","|":"backslash","~":"grave"]
            var textRecipe: (String, CGKeyCode, CGEventFlags)?
            if isBlender, scalarText.unicodeScalars.count == 1,
               let scalar = scalarText.unicodeScalars.first, (32...126).contains(scalar.value) {
                let base = shifted[scalarText] ?? (scalarText == " " ? "space" : (scalarText == "\\" ? "backslash" : (scalarText == "`" ? "grave" : scalarText.lowercased())))
                textRecipe = try? keyRecipe(base)
                if shifted[scalarText] != nil || ("A"..."Z").contains(scalarText) {
                    textRecipe?.2.insert(.maskShift)
                }
            }
            let flags = textRecipe?.2 ?? []
            let needsShift = flags.contains(.maskShift)
            if textShiftHeld != needsShift {
                try modifierEvent(needsShift ? .maskShift : [], code: 56)
                textShiftHeld = needsShift
            }
            let units = Array(scalarText.utf16)
            for down in [true, false] {
                let e = try keyEvent(code: textRecipe?.1 ?? 0, down: down, flags: flags, text: scalarText)
                e.flags = flags
                // Blender numeric/modal handlers need MINUS/PERIOD/digit identity,
                // not only a Unicode packet with virtual key 0. Text widgets still
                // receive the original character via NSEvent.characters. Keep the
                // Unicode packet route for non-ASCII and other native applications.
                if textRecipe == nil {
                    units.withUnsafeBufferPointer {
                        e.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress)
                    }
                }
                try postKeyboard(e)
                keyboardActivity()
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.008))
            }
        }
        if textShiftHeld { try modifierEvent([], code: 56); textShiftHeld = false }
    case "key":
        let (name, code, flags) = recipe!
        // Blender/GHOST and some native editors consume flagsChanged separately
        // from keyDown. Explicit local modifier transitions never touch HID.
        try modifierEvent([], code: 56)
        let modifierKeys: [(CGEventFlags, CGKeyCode)] = [(.maskShift,56),(.maskControl,59),(.maskAlternate,58),(.maskCommand,55)]
        var activeFlags: CGEventFlags = []
        var held: [(CGEventFlags, CGKeyCode)] = []
        func releaseModifiers() throws {
            while let (flag, key) = held.last {
                activeFlags.remove(flag)
                try modifierEvent(activeFlags, code: key)
                held.removeLast()
            }
        }
        defer { try? releaseModifiers() }
        for (flag, key) in modifierKeys where flags.contains(flag) {
            activeFlags.insert(flag); held.append((flag,key))
            try modifierEvent(activeFlags, code: key)
        }
        for down in [true, false] {
            let special: [String:String] = ["enter":"\r", "return":"\r", "tab":"\t", "space":" ", "backspace":"\u{8}", "escape":"\u{1b}", "esc":"\u{1b}", "delete":"\u{f728}", "left":"\u{f702}", "right":"\u{f703}", "up":"\u{f700}", "down":"\u{f701}"]
            let function = name.hasPrefix("f") ? Int(name.dropFirst()).flatMap { (1...20).contains($0) ? $0 : nil } : nil
            let functionText = function.flatMap { UnicodeScalar(0xf703 + $0) }.map(String.init)
            let navigation: [String:String] = ["home":"\u{f729}","end":"\u{f72b}","pageup":"\u{f72c}","pagedown":"\u{f72d}"]
            let punctuation: [String:String] = ["minus":"-","equal":"=","period":".","comma":",","slash":"/","semicolon":";","quote":"'","leftbracket":"[","rightbracket":"]","backslash":"\\","grave":"`","numpaddecimal":".","numpadadd":"+","numpadsubtract":"-","numpadmultiply":"*","numpaddivide":"/","numpadenter":"\r","numpadequal":"="]
            let keypadText = name.hasPrefix("numpad") && name.count == 7 ? String(name.suffix(1)) : nil
            let text = punctuation[name] ?? keypadText ?? special[name] ?? navigation[name] ?? functionText ?? (name.count == 1 ? (flags.contains(.maskShift) ? name.uppercased() : name) : "")
            let e = try keyEvent(code: code, down: down, flags: flags, text: text)
            e.flags = flags
            // Shortcut key equivalents keep hardware identity; plain printable
            // keys include Unicode for native text-input clients.
            if name.utf16.count == 1 && flags.intersection([.maskCommand, .maskControl, .maskAlternate]).isEmpty {
                let units = Array((flags.contains(.maskShift) ? name.uppercased() : name).utf16)
                units.withUnsafeBufferPointer {
                    e.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress)
                }
            }
            try postKeyboard(e)
            keyboardActivity()
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.008))
        }
        try releaseModifiers()
    case "scroll":
        let p = points[0]
        guard let delta = a["delta"] as? Int32 else { throw Failure("Missing delta") }
        try mouse(.mouseMoved, p)
        let e = CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 1,
                        wheel1: -max(-100, min(100, delta)), wheel2: 0, wheel3: 0)
        e?.location = p
        e?.flags = pointerFlags
        if let e {
            setWindowLocation(e, CGPoint(x: p.x - origin.x, y: p.y - origin.y))
            try postMouse(e)
        }
    default:
        throw Failure("Unknown background computer action")
    }
    try releasePointerModifiers()
    if ["key", "type"].contains(action) { keyboardNotice.record("click") }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.04))
    let delivery = accessibilityAction == nil ? "process-window" : "accessibility-window"
    var result: [String: Any] = ["ok": true, "action": action, "mode": "background", "window_id": id, "pid": pid, "coordinate_space": "window", "pointer_color": "#7DD4FF", "pointer_pressed_color": "#2563EB", "before": before, "after": state(), "input_events_posted": eventsPosted, "delivery": delivery, "responder_lease": false, "target_was_background": wasBackground, "pointer": virtualPointer.metadata()]
    if let accessibilityAction {
        result["accessibility_action"] = accessibilityAction
        result["accessibility_actions_performed"] = 1
    }
    result["context_events_posted"] = keyboardContext == nil ? 0 : 1
    if let local = keyboardContext {
        result["keyboard_context"] = ["window_id": id, "pid": pid, "x": local.x, "y": local.y, "source": "last-virtual-pointer"]
    }
    result["geometry_source"] = targetWindow["geometry_source"] ?? "chrome-devtools"
    result["geometry_attempts"] = targetWindow["geometry_attempts"] ?? 1
    result["target_window_before"] = w
    result["target_window_after"] = windows().first { $0["window_id"] as? Int == id } ?? [:]
    if let pointer = pointer { result["virtual_pointer"] = ["x": pointer.x, "y": pointer.y] }
    return result
}
// Self-tests exercise the production clock boundaries without Accessibility.
if CommandLine.arguments.contains("--self-test") {
    for (name, expected): (String, CGKeyCode) in [("minus",27),("period",47),("numpad1",83),("numpaddecimal",65),("shift+f4",118)] {
        let (_, code, _) = try keyRecipe(name)
        precondition(code == expected, "Incorrect modeling key code: \(name)")
    }
    let (_, _, keypadFlags) = try keyRecipe("numpad1")
    precondition(keypadFlags.contains(.maskNumericPad))
    nativePointerContexts[1] = NativePointerContext(pid: 10, local: CGPoint(x: 10,y: 20),size: CGSize(width: 100,height: 100))
    nativePointerContexts[2] = NativePointerContext(pid: 20, local: CGPoint(x: 50,y: 60),size: CGSize(width: 100,height: 100))
    precondition(nativePointerContexts[1]?.local == CGPoint(x: 10,y: 20))
    precondition(nativePointerContexts[2]?.pid == 20)
    nativePointerContexts.removeAll()
    var notice = KeyboardNoticeState()
    notice.begin(label: "g", application: "test")
    precondition(!notice.visible(at: 0))
    notice.record("down", at: 10); notice.record("up", at: 10.1); notice.record("click", at: 10.2)
    precondition(notice.phases == ["down","up","click"])
    precondition(notice.visible(at: 15.19) && !notice.visible(at: 15.21))
    print("PASS keyboard notice lifecycle and five-second expiry")

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
        var result: [String: Any]
        do {
            guard let args = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw Failure("Expected JSON object") }
            result = try run(args)
        } catch { result = ["error": String(describing: error)] }
        result["keyboard_notice"] = keyboardNotice.metadata()
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
