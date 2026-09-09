import AppKit
import ApplicationServices

// This short-lived helper owns AppKit on its main thread. It never activates an
// app, warps the system cursor, or posts to the global HID event stream.
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
            "cursor_x": p.x, "cursor_y": p.y]
}
final class PointerView: NSView {
    var leftPressed = false { didSet { needsDisplay = true } }
    override var isFlipped: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        let p = NSBezierPath()
        p.move(to: NSPoint(x: 2, y: 2))
        for point in [NSPoint(x: 3, y: 29), NSPoint(x: 10, y: 22), NSPoint(x: 16, y: 34), NSPoint(x: 22, y: 31), NSPoint(x: 16, y: 19), NSPoint(x: 27, y: 18)] { p.line(to: point) }
        p.close()
        (leftPressed
            ? NSColor(calibratedRed: 37.0 / 255, green: 99.0 / 255, blue: 235.0 / 255, alpha: 1)
            : NSColor(calibratedRed: 125.0 / 255, green: 212.0 / 255, blue: 1, alpha: 1)).setFill()
        p.fill()
        NSColor(calibratedWhite: 0.15, alpha: 0.9).setStroke()
        p.lineWidth = 1.5
        p.stroke()
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
func run(_ a: [String: Any]) throws -> [String: Any] {
    let action = a["action"] as? String ?? ""
    if action == "windows" {
        return ["windows": windows(), "accessibility": AXIsProcessTrusted(), "screen_recording": CGPreflightScreenCaptureAccess(), "desktop": state()]
    }
    guard let id = a["window_id"] as? Int, id > 0,
          let w = windows().first(where: { ($0["window_id"] as? Int) == id }),
          let pid = w["pid"] as? Int32 else { throw Failure("Target window is unavailable; call windows and select a window_id") }
    if let expected = a["pid"] as? Int32, expected != pid { throw Failure("Window owner changed; select the target again") }
    guard (w["onscreen"] as? Bool) == true else { throw Failure("Target must be on the current desktop and not minimized; it may be behind other windows") }
    let width = (w["width"] as! NSNumber).doubleValue, height = (w["height"] as! NSNumber).doubleValue
    let origin = CGPoint(x: (w["x"] as! NSNumber).doubleValue, y: (w["y"] as! NSNumber).doubleValue)
    if action == "screenshot" {
        guard let path = a["capture_path"] as? String else { throw Failure("Missing capture path") }
        let p = Process()
        p.executableURL = URL(fileURLWithPath: "/usr/sbin/screencapture")
        p.arguments = ["-x", "-o", "-l", String(id), "-t", "png", path]
        try p.run(); p.waitUntilExit()
        guard p.terminationStatus == 0, let image = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: path))) else { throw Failure("Window capture failed; enable Screen Recording") }
        // Window capture excludes our separate overlay panel. Paint its last
        // window-local position into the returned image for the agent viewer.
        if let pointer = a["virtual_pointer"] as? [String: Double],
           let x = pointer["x"], let y = pointer["y"] {
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
            PointerView().draw(.zero)
            NSGraphicsContext.restoreGraphicsState()
            if let png = bitmap.representation(using: .png, properties: [:]) {
                try png.write(to: URL(fileURLWithPath: path))
            }
        }
        return ["path": path, "mime": "image/png", "window_id": id, "pid": pid, "screen_width": image.pixelsWide, "screen_height": image.pixelsHigh, "logical_width": width, "logical_height": height, "coordinate_space": "window", "pointer_overlay": a["virtual_pointer"] != nil]
    }
    guard AXIsProcessTrusted() else { throw Failure("Enable Accessibility permission for the process running Lessagent") }
    guard let locationSymbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "CGEventSetWindowLocation") else {
        throw Failure("This macOS version lacks background window event routing; no desktop input was sent")
    }
    typealias SetWindowLocation = @convention(c) (CGEvent, CGPoint) -> Void
    let setWindowLocation = unsafeBitCast(locationSymbol, to: SetWindowLocation.self)
    let source = CGEventSource(stateID: .privateState)
    func point(_ x: String, _ y: String) throws -> CGPoint {
        guard let px = a[x] as? Double, let py = a[y] as? Double, px.isFinite, py.isFinite else { throw Failure("Missing numeric \(x), \(y)") }
        let sw = a["screen_width"] as? Double ?? width, sh = a["screen_height"] as? Double ?? height
        guard sw.isFinite, sh.isFinite, sw > 0, sh > 0 else { throw Failure("Invalid screenshot dimensions") }
        let local = CGPoint(x: px * width / sw, y: py * height / sh)
        guard local.x >= 0, local.y >= 0, local.x < width, local.y < height else { throw Failure("Coordinate outside target window") }
        return CGPoint(x: origin.x + local.x, y: origin.y + local.y)
    }
    let before = state()
    typealias MainConnection = @convention(c) () -> UInt32
    typealias WindowTags = @convention(c) (UInt32, UInt32, UnsafeMutablePointer<UInt64>, Int32) -> Int32
    var tagResult = Int32(-999)
    var tags = UInt64(a["tags"] as? Int ?? 0)
    let handle = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW)
    let connection = handle.flatMap { dlsym($0, "SLSMainConnectionID") }.map { unsafeBitCast($0, to: MainConnection.self)() } ?? 0
    let setTags = handle.flatMap { dlsym($0, "SLSSetWindowTags") }.map { unsafeBitCast($0, to: WindowTags.self) }
    let clearTags = handle.flatMap { dlsym($0, "SLSClearWindowTags") }.map { unsafeBitCast($0, to: WindowTags.self) }
    if tags != 0, let setTags = setTags { tagResult = setTags(connection, UInt32(id), &tags, 64) }
    defer { if tags != 0 && tagResult == 0 { _ = clearTags?(connection, UInt32(id), &tags, 64) } }

    typealias PSNLookup = @convention(c) (Int32, UnsafeMutablePointer<UInt32>) -> Int32
    typealias PostRecord = @convention(c) (UnsafePointer<UInt32>, UnsafePointer<UInt8>) -> Int32
    let lookup = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "GetProcessForPID").map { unsafeBitCast($0, to: PSNLookup.self) }
    let postRecord = handle.flatMap { dlsym($0, "SLPSPostEventRecordTo") }.map { unsafeBitCast($0, to: PostRecord.self) }
    var targetPSN = [UInt32](repeating: 0, count: 2)
    var record = [UInt8](repeating: 0, count: 0xf8)
    record[4] = 0xf8; record[8] = 0x0d
    for i in 0..<4 { record[0x3c+i] = UInt8(truncatingIfNeeded: id >> (8*i)) }
    if a["focus"] as? Bool == true, let lookup = lookup, let postRecord = postRecord {
        guard lookup(pid, &targetPSN) == 0 else { throw Failure("PSN lookup failed") }
        record[0x8a] = 1
        guard postRecord(targetPSN, record) == 0 else { throw Failure("Focus record failed") }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.06))
    }
    defer {
        if a["focus"] as? Bool == true, let postRecord = postRecord {
            record[0x8a] = 2; _ = postRecord(targetPSN, record)
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.06))
        }
    }
    var panel: NSPanel?
    var pointer: CGPoint?
    func show(_ p: CGPoint) {
        pointer = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
        if a["show_pointer"] as? Bool == false { return }
        if panel == nil { panel = pointerPanel() }
        let top = NSScreen.screens.first?.frame.maxY ?? 0
        panel!.setFrameOrigin(NSPoint(x: p.x - 2, y: top - p.y - 38))
        panel!.orderFrontRegardless()
        panel!.displayIfNeeded()
    }
    typealias PostToPid = @convention(c) (Int32, CGEvent) -> Void
    typealias SetInt = @convention(c) (CGEvent, UInt32, Int64) -> Void
    let postSL = handle.flatMap { dlsym($0, "SLEventPostToPid") }.map { unsafeBitCast($0, to: PostToPid.self) }
    let setInt = handle.flatMap { dlsym($0, "SLEventSetIntegerValueField") }.map { unsafeBitCast($0, to: SetInt.self) }
    let group = Int64.random(in: 1...Int64(Int32.max))
    var primer = false
    func post(_ e: CGEvent?) throws {
        guard let e = e else { throw Failure("Cannot create input event") }
        e.setIntegerValueField(.mouseEventWindowUnderMousePointer, value: Int64(id))
        e.setIntegerValueField(.mouseEventWindowUnderMousePointerThatCanHandleThisEvent, value: Int64(id))
        if a["stamp"] as? Bool == true {
            for (field, value) in [(UInt32(40), Int64(pid)), (51, Int64(id)), (58, group)] { setInt?(e,field,value) }
            setInt?(e, 0, e.type == .mouseMoved ? 2 : primer ? (e.type == .leftMouseDown ? 1 : 2) : 3)
            if primer { e.location = CGPoint(x: -1, y: -1); setWindowLocation(e, CGPoint(x: -1, y: -1)) }
        }
        if a["post"] as? String == "sl", let postSL = postSL { postSL(pid,e) }
        else { e.postToPid(pid) }
    }
    let right = a["button"] as? String == "right"
    let button: CGMouseButton = right ? .right : .left
    func mouse(_ type: CGEventType, _ p: CGPoint) throws {
        let nsType: NSEvent.EventType
        switch type {
        case .leftMouseDown: nsType = .leftMouseDown
        case .leftMouseUp: nsType = .leftMouseUp
        case .rightMouseDown: nsType = .rightMouseDown
        case .rightMouseUp: nsType = .rightMouseUp
        case .leftMouseDragged: nsType = .leftMouseDragged
        case .rightMouseDragged: nsType = .rightMouseDragged
        default: nsType = .mouseMoved
        }
        let local = CGPoint(x: p.x - origin.x, y: p.y - origin.y)
        var e = NSEvent.mouseEvent(with: nsType, location: NSPoint(x: local.x, y: height - local.y),
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: id, context: nil, eventNumber: 1, clickCount: 1, pressure: 1)?.cgEvent
        if a["cg"] as? Bool == true { e = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: p, mouseButton: button) }
        e?.location = p
        if let e = e { setWindowLocation(e, local) }
        e?.setIntegerValueField(.mouseEventButtonNumber, value: Int64(button.rawValue))
        e?.setIntegerValueField(.mouseEventSubtype, value: Int64(a["subtype"] as? Int ?? 3))
        e?.flags = CGEventFlags(rawValue: UInt64(a["flags"] as? Int ?? 0))
        e?.setIntegerValueField(.mouseEventClickState, value: 1)
        try post(e)
        show(p)
        if let view = panel?.contentView as? PointerView {
            view.leftPressed = type == .leftMouseDown || type == .leftMouseDragged
            panel?.displayIfNeeded()
        }
    }
    switch action {
    case "move", "click", "drag":
        let start = try point("x", "y")
        var points = [start]
        if action == "drag" {
            if let path = a["path"] as? [[Double]] {
                guard path.count >= 2, path.count <= 512 else { throw Failure("path needs 2 to 512 points") }
                points = try path.map { pair in
                    guard pair.count == 2 else { throw Failure("path points must be [x,y]") }
                    let sw = a["screen_width"] as? Double ?? width, sh = a["screen_height"] as? Double ?? height
                    guard sw > 0, sh > 0, pair.allSatisfy({ $0.isFinite }), pair[0] >= 0, pair[1] >= 0, pair[0] < sw, pair[1] < sh else { throw Failure("Invalid path coordinate") }
                    return CGPoint(x: origin.x + pair[0] * width / sw, y: origin.y + pair[1] * height / sh)
                }
            } else { points.append(try point("to_x", "to_y")) }
        }
        try mouse(.mouseMoved, points[0])
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.015))
        if action != "move" {
            if a["primer"] as? Bool == true {
                primer = true
                try mouse(.leftMouseDown, CGPoint(x: origin.x-1,y:origin.y-1))
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
                try mouse(.leftMouseUp, CGPoint(x: origin.x-1,y:origin.y-1))
                primer = false
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
            }
            let down: CGEventType = right ? .rightMouseDown : .leftMouseDown
            let up: CGEventType = right ? .rightMouseUp : .leftMouseUp
            try mouse(down, points[0])
            // Always release a pressed button even if a later event fails.
            var last = points[0]
            defer { try? mouse(up, last) }
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
            if action == "drag" {
                let duration = min(5, max(0.05, a["duration"] as? Double ?? (a["duration_ms"] as? Double ?? 500) / 1000))
                let lengths = zip(points, points.dropFirst()).map { hypot($1.x - $0.x, $1.y - $0.y) }
                let total = max(1, lengths.reduce(0, +))
                for i in 1..<points.count {
                    let steps = max(1, min(240, Int(ceil(lengths[i - 1] / 4))))
                    for step in 1...steps {
                        let f = Double(step) / Double(steps)
                        last = CGPoint(x: points[i - 1].x + (points[i].x - points[i - 1].x) * f, y: points[i - 1].y + (points[i].y - points[i - 1].y) * f)
                        try mouse(right ? .rightMouseDragged : .leftMouseDragged, last)
                        RunLoop.current.run(until: Date(timeIntervalSinceNow: duration * lengths[i - 1] / total / Double(steps)))
                    }
                }
            }
        }
    case "type":
        guard let text = a["text"] as? String, text.utf16.count <= 16384 else { throw Failure("Missing text or text exceeds 16384 UTF-16 units") }
        // Small chunks avoid the native event's Unicode size limit; preserve surrogate pairs.
        for character in text {
            let units = Array(String(character).utf16)
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: down)
                e?.flags = []
                units.withUnsafeBufferPointer { e?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress) }
                try post(e)
                RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
            }
        }
    case "key":
        guard let key = a["key"] as? String else { throw Failure("Missing key") }
        var parts = key.lowercased().split(separator: "+").map(String.init)
        let name = parts.popLast() ?? ""
        let codes: [String: CGKeyCode] = ["a":0,"s":1,"d":2,"f":3,"h":4,"g":5,"z":6,"x":7,"c":8,"v":9,"b":11,"q":12,"w":13,"e":14,"r":15,"y":16,"t":17,"1":18,"2":19,"3":20,"4":21,"6":22,"5":23,"9":25,"7":26,"8":28,"0":29,"o":31,"u":32,"i":34,"p":35,"enter":36,"return":36,"l":37,"j":38,"k":40,"n":45,"m":46,"tab":48,"space":49,"backspace":51,"escape":53,"esc":53,"delete":117,"home":115,"end":119,"pageup":116,"pagedown":121,"left":123,"right":124,"down":125,"up":126]
        guard let code = codes[name] else { throw Failure("Unsupported key; use type for text") }
        var flags: CGEventFlags = []
        for modifier in parts {
            switch modifier {
            case "cmd", "super", "meta": flags.insert(.maskCommand)
            case "ctrl", "control": flags.insert(.maskControl)
            case "alt", "option": flags.insert(.maskAlternate)
            case "shift": flags.insert(.maskShift)
            default: throw Failure("Unknown key modifier")
            }
        }
        for down in [true, false] {
            let e = CGEvent(keyboardEventSource: source, virtualKey: code, keyDown: down)
            e?.flags = flags
            if name.utf16.count == 1 {
                let units = Array((flags.contains(.maskShift) ? name.uppercased() : name).utf16)
                units.withUnsafeBufferPointer { e?.keyboardSetUnicodeString(stringLength: units.count, unicodeString: $0.baseAddress) }
            }
            try post(e)
            RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.01))
        }
    case "scroll":
        let p = try point("x", "y")
        guard let delta = a["delta"] as? Int32 else { throw Failure("Missing delta") }
        try mouse(.mouseMoved, p)
        let e = CGEvent(scrollWheelEvent2Source: source, units: .line, wheelCount: 1, wheel1: -max(-100, min(100, delta)), wheel2: 0, wheel3: 0)
        e?.location = p; e?.flags = []
        // Cocoa's window-number event stamp, also supplied by NSEvent for mouse events.
        if let field = CGEventField(rawValue: 51) { e?.setIntegerValueField(field, value: Int64(id)) }
        if let e = e { setWindowLocation(e, CGPoint(x: p.x - origin.x, y: p.y - origin.y)) }
        try post(e)
    default: throw Failure("Unknown background computer action")
    }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: panel == nil ? 0.04 : 0.2))
    panel?.orderOut(nil)
    var result: [String: Any] = ["ok": true, "action": action, "mode": "background", "window_id": id, "pid": pid, "coordinate_space": "window", "pointer_color": "#7DD4FF", "pointer_pressed_color": "#2563EB", "before": before, "after": state(), "tag_result": tagResult]
    if let pointer = pointer { result["virtual_pointer"] = ["x": pointer.x, "y": pointer.y] }
    return result
}
let app = NSApplication.shared
app.setActivationPolicy(.prohibited)
do {
    let data = FileHandle.standardInput.readDataToEndOfFile()
    guard let args = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw Failure("Expected JSON object") }
    let result = try run(args)
    FileHandle.standardOutput.write(try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]))
} catch {
    let result = ["error": String(describing: error)]
    FileHandle.standardOutput.write(try! JSONSerialization.data(withJSONObject: result))
    exit(1)
}
