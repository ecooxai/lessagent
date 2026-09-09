import AppKit
import Foundation

// Browser-local input is required for independent Chrome button state. Chromium's
// AppKit event builder samples NSEvent.pressedMouseButtons (the physical mouse),
// so PID-posted drags cannot provide this isolation. Only sessions explicitly
// created by Lessagent are attached; the user's normal browser/profile is never
// debug-enabled, restarted, or silently substituted.
final class AsyncBox<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Result<T, Error>?
    func set(_ result: Result<T, Error>) { lock.lock(); value = result; lock.unlock() }
    func get() -> Result<T, Error>? { lock.lock(); defer { lock.unlock() }; return value }
}
func awaitLocal<T>(_ timeout: TimeInterval = 8, _ start: (AsyncBox<T>) -> Void) throws -> T {
    let box = AsyncBox<T>(); start(box)
    let end = Date(timeIntervalSinceNow: timeout)
    while Date() < end {
        if let value = box.get() { return try value.get() }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.002))
    }
    throw Failure("Background browser did not respond; input is not retried or sent to the desktop")
}
final class BrowserConnection {
    private let session: URLSession
    private let socket: URLSessionWebSocketTask
    private var sequence = 0
    init(_ endpoint: String) throws {
        guard let url = URL(string: endpoint), url.scheme == "ws", url.host == "127.0.0.1",
              let port = url.port, (1...65535).contains(port), url.user == nil, url.password == nil,
              url.path.hasPrefix("/devtools/browser/") else { throw Failure("Invalid local browser endpoint") }
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 8
        session = URLSession(configuration: config)
        socket = session.webSocketTask(with: url)
        socket.resume()
    }
    deinit { socket.cancel(with: .goingAway, reason: nil); session.invalidateAndCancel() }
    func call(_ method: String, _ params: [String: Any] = [:], sessionID: String? = nil) throws -> [String: Any] {
        sequence += 1
        let id = sequence
        var request: [String: Any] = ["id": id, "method": method, "params": params]
        if let sessionID = sessionID { request["sessionId"] = sessionID }
        let encoded = try JSONSerialization.data(withJSONObject: request)
        let text = String(decoding: encoded, as: UTF8.self)
        let _: Bool = try awaitLocal { box in
            self.socket.send(.string(text)) { error in
                if let error = error { box.set(.failure(error)) } else { box.set(.success(true)) }
            }
        }
        let deadline = Date(timeIntervalSinceNow: 8)
        while Date() < deadline {
            let message: URLSessionWebSocketTask.Message = try awaitLocal { box in
                self.socket.receive { box.set($0) }
            }
            let bytes: Data
            switch message { case .string(let text): bytes = Data(text.utf8); case .data(let data): bytes = data; @unknown default: continue }
            guard bytes.count <= 8 * 1024 * 1024,
                  let object = try JSONSerialization.jsonObject(with: bytes) as? [String: Any] else { throw Failure("Invalid browser response") }
            if object["id"] as? Int != id { continue }
            if let error = object["error"] { throw Failure("Browser \(method) failed: \(error). No native input fallback was used") }
            return object["result"] as? [String: Any] ?? [:]
        }
        throw Failure("Browser response deadline exceeded; inspect the target before retrying")
    }
}
struct ManagedBrowser: Codable {
    let windowID: Int
    let pid: Int32
    let endpoint: String
    let targetID: String
    let browserWindowID: Int
    let profile: String
}
func browserRoot(_ args: [String: Any]) throws -> URL {
    guard let path = args["_browser_root"] as? String, path.hasPrefix("/") else {
        throw Failure("Browser sessions require the shared Lessagent tool executor")
    }
    return URL(fileURLWithPath: path, isDirectory: true)
}
func browserRecordURL(_ root: URL, _ id: Int, _ pid: Int32) -> URL {
    root.appendingPathComponent("window-\(id)-\(pid).json")
}
func managedBrowser(_ args: [String: Any], id: Int, pid: Int32) throws -> ManagedBrowser? {
    guard args["_browser_root"] != nil else { return nil }
    let file = browserRecordURL(try browserRoot(args), id, pid)
    guard FileManager.default.fileExists(atPath: file.path) else { return nil }
    let record = try JSONDecoder().decode(ManagedBrowser.self, from: Data(contentsOf: file))
    guard record.windowID == id, record.pid == pid else { throw Failure("Browser target record mismatch") }
    return record
}
func openManagedBrowser(_ args: [String: Any]) throws -> [String: Any] {
    guard let text = args["url"] as? String, let url = URL(string: text),
          ["http", "https"].contains(url.scheme ?? ""), url.host != nil,
          url.user == nil, url.password == nil else { throw Failure("browser_open requires an HTTP(S) URL without embedded credentials") }
    // Refresh display geometry for every launch: cached MCP schemas can be stale
    // after a resolution/monitor change. No profile or process exists yet.
    guard let screen = primaryScreen() else { throw Failure("Primary display unavailable; no browser was opened") }
    let frame = screen.frame, visible = screen.visibleFrame
    let maxWidth = Int(frame.width), maxHeight = Int(frame.height)
    guard maxWidth >= 640, maxHeight >= 480, visible.width >= 640, visible.height >= 480 else {
        throw Failure("Primary display work area is too small for a 640 by 480 browser window; no browser was opened")
    }
    for field in ["width", "height"] {
        if args[field] != nil && args[field] as? Int == nil { throw Failure("Browser \(field) must be an integer") }
    }
    let requestedWidth = args["width"] as? Int ?? min(1000, maxWidth)
    let requestedHeight = args["height"] as? Int ?? min(600, maxHeight)
    guard (640...maxWidth).contains(requestedWidth), (480...maxHeight).contains(requestedHeight) else {
        throw Failure("Browser size exceeds the current primary display: maximum \(maxWidth) by \(maxHeight) logical points; no browser was opened")
    }
    let width = min(requestedWidth, Int(visible.width)), height = min(requestedHeight, Int(visible.height))
    let left = Int(visible.minX + (visible.width - Double(width)) / 2)
    // CDP uses top-left Quartz coordinates, whereas AppKit uses bottom-left.
    let top = Int(frame.maxY - visible.maxY + (visible.height - Double(height)) / 2)
    let root = try browserRoot(args)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
    let profile = root.appendingPathComponent("profile-" + UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: profile, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    let before = state()
    let launch = Process()
    launch.executableURL = URL(fileURLWithPath: "/usr/bin/open")
    launch.arguments = ["-g", "-n", "-a", "Google Chrome", "--args", "--user-data-dir=" + profile.path,
        "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0", "--no-startup-window",
        "--no-first-run", "--no-default-browser-check", "--disable-background-networking",
        "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding"]
    try launch.run(); launch.waitUntilExit()
    guard launch.terminationStatus == 0 else { throw Failure("Unable to start a separate background Chrome profile") }
    let portFile = profile.appendingPathComponent("DevToolsActivePort")
    var endpoint: String?
    for _ in 0..<150 {
        if let text = try? String(contentsOf: portFile, encoding: .utf8) {
            let lines = text.split(separator: "\n")
            if lines.count >= 2, let port = Int(lines[0]), (1...65535).contains(port), lines[1].hasPrefix("/devtools/browser/") {
                endpoint = "ws://127.0.0.1:\(port)\(lines[1])"; break
            }
        }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    }
    guard let endpoint = endpoint else { throw Failure("Separate Chrome did not expose its loopback control endpoint; the normal browser was not modified") }
    let connection = try BrowserConnection(endpoint)
    let processInfo = try connection.call("SystemInfo.getProcessInfo")
    guard let processes = processInfo["processInfo"] as? [[String: Any]],
          let browser = processes.first(where: { $0["type"] as? String == "browser" }),
          let number = browser["id"] as? NSNumber else { throw Failure("Cannot verify the managed browser process") }
    let pid = number.int32Value
    var succeeded = false
    defer { if !succeeded { _ = try? connection.call("Browser.close") } }
    let created = try connection.call("Target.createTarget", ["url": url.absoluteString, "newWindow": true, "background": true, "width": width, "height": height])
    guard let targetID = created["targetId"] as? String else { throw Failure("Chrome did not create the requested background page") }
    let targetWindow = try connection.call("Browser.getWindowForTarget", ["targetId": targetID])
    guard let browserWindowID = targetWindow["windowId"] as? Int else { throw Failure("Chrome did not identify its new window") }
    // Chrome may choose another display or restore old bounds; set and verify
    // only this new isolated window, without activation or OS pointer events.
    try connection.call("Browser.setWindowBounds", ["windowId": browserWindowID,
        "bounds": ["left": left, "top": top, "width": width, "height": height, "windowState": "normal"]])
    var fitted = false
    for _ in 0..<40 {
        let info = try connection.call("Browser.getWindowForTarget", ["targetId": targetID])
        if let bounds = info["bounds"] as? [String: Any],
           let w = bounds["width"] as? Int, let h = bounds["height"] as? Int,
           let x = bounds["left"] as? Int, let y = bounds["top"] as? Int,
           w == width, h == height, x == left, y == top { fitted = true; break }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    }
    guard fitted else { throw Failure("Chrome could not fit the new window to the current display work area; the isolated browser was closed") }
    var target: [String: Any]?
    for _ in 0..<100 {
        let candidates = windows().filter { ($0["pid"] as? Int32) == pid && ($0["onscreen"] as? Bool == true) }
        if candidates.count == 1 { target = candidates[0]; break }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    }
    guard let target = target, let id = target["window_id"] as? Int else { throw Failure("Cannot uniquely identify an onscreen managed Chrome window for PID \(pid); no input was sent") }
    let record = ManagedBrowser(windowID: id, pid: pid, endpoint: endpoint, targetID: targetID, browserWindowID: browserWindowID, profile: profile.path)
    let file = browserRecordURL(root, id, pid)
    try JSONEncoder().encode(record).write(to: file, options: .atomic)
    try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: file.path)
    succeeded = true
    return ["ok": true, "action": "browser_open", "mode": "background", "window_id": id, "pid": pid,
        "url": url.absoluteString, "delivery": "chrome-devtools", "isolated_profile": true,
        "before": before, "after": state(), "coordinate_space": "window", "input_events_posted": 0,
        "browser_size": ["requested_width": requestedWidth, "requested_height": requestedHeight,
                         "width": width, "height": height, "maximum_width": maxWidth, "maximum_height": maxHeight,
                         "visible_width": Int(visible.width), "visible_height": Int(visible.height),
                         "left": left, "top": top, "units": "logical_points",
                         "fitted_to_work_area": width != requestedWidth || height != requestedHeight]]
}

// Quartz exposes presentation bounds during Mission Control/Space animations.
// Those can be a thumbnail of the real window. CDP retains the full logical
// window geometry, which must be paired with the actual captured PNG size.
func managedWindowGeometry(_ record: ManagedBrowser, native: [String: Any]) throws -> [String: Any] {
    let connection = try BrowserConnection(record.endpoint)
    let info = try connection.call("Browser.getWindowForTarget", ["targetId": record.targetID])
    guard info["windowId"] as? Int == record.browserWindowID,
          let bounds = info["bounds"] as? [String: Any] else {
        throw Failure("Managed page moved to a different window; no input was sent")
    }
    var result = native
    for (key, field) in [("x", "left"), ("y", "top"), ("width", "width"), ("height", "height")] {
        guard let value = (bounds[field] as? NSNumber)?.doubleValue, value.isFinite,
              (key != "width" && key != "height") || (value > 0 && value <= 32768) else {
            throw Failure("Managed window geometry is unavailable; no input was sent")
        }
        result["presentation_" + key] = native[key]
        result[key] = value
    }
    return result
}

// The sole JavaScript expression reads viewport geometry. It never modifies
// the DOM, calls an event handler, dispatches a DOM event, or draws on a canvas.
private let viewportExpression = "({width:innerWidth,height:innerHeight,outerWidth:outerWidth,outerHeight:outerHeight,dpr:devicePixelRatio})"
func browserControl(_ args: [String: Any], record: ManagedBrowser, window: [String: Any], points: [CGPoint], recipe: (String, CGKeyCode, CGEventFlags)?) throws -> [String: Any] {
    let connection = try BrowserConnection(record.endpoint)
    let windowInfo = try connection.call("Browser.getWindowForTarget", ["targetId": record.targetID])
    guard windowInfo["windowId"] as? Int == record.browserWindowID,
          let bounds = windowInfo["bounds"] as? [String: Any] else { throw Failure("Managed page moved to a different window; reopen it with browser_open") }
    for (native, browser) in [("x", "left"), ("y", "top"), ("width", "width"), ("height", "height")] {
        guard let a = (window[native] as? NSNumber)?.doubleValue, let b = (bounds[browser] as? NSNumber)?.doubleValue, abs(a-b) <= 3 else {
            throw Failure("Browser/window geometry mismatch for \(native): native=\(String(describing: window[native])), browser=\(String(describing: bounds[browser])); no input was sent")
        }
    }
    // Never drive a hidden tab using a different tab's screenshot.
    if let infos = try connection.call("Target.getTargets")["targetInfos"] as? [[String: Any]] {
        for info in infos where info["type"] as? String == "page" && info["targetId"] as? String != record.targetID {
            if let otherID = info["targetId"] as? String,
               let other = try? connection.call("Browser.getWindowForTarget", ["targetId": otherID]),
               other["windowId"] as? Int == record.browserWindowID {
                throw Failure("This managed window has multiple tabs; close the extra tab or open a separate browser_open window")
            }
        }
    }
    let attached = try connection.call("Target.attachToTarget", ["targetId": record.targetID, "flatten": true])
    guard let sessionID = attached["sessionId"] as? String else { throw Failure("Cannot attach to the managed page") }
    func call(_ method: String, _ params: [String: Any] = [:]) throws -> [String: Any] { try connection.call(method, params, sessionID: sessionID) }
    let evaluated = try call("Runtime.evaluate", ["expression": viewportExpression, "returnByValue": true])
    guard let result = evaluated["result"] as? [String: Any], let geometry = result["value"] as? [String: Any],
          let innerWidth = geometry["width"] as? Double, let innerHeight = geometry["height"] as? Double,
          let dpr = geometry["dpr"] as? Double, let width = (window["width"] as? NSNumber)?.doubleValue,
          let height = (window["height"] as? NSNumber)?.doubleValue, let x = (window["x"] as? NSNumber)?.doubleValue, let y = (window["y"] as? NSNumber)?.doubleValue else { throw Failure("Cannot read page viewport geometry") }
    let primaryTop = NSScreen.screens.first?.frame.maxY ?? 0
    let center = NSPoint(x: x+width/2, y: primaryTop-y-height/2)
    let backing = NSScreen.screens.first(where: { $0.frame.contains(center) })?.backingScaleFactor ?? 1
    let zoom = dpr / backing
    let insetX = (width-innerWidth*zoom)/2, insetY = height-innerHeight*zoom
    guard zoom.isFinite, zoom > 0, abs(insetX) <= 3, insetY >= 0, insetY < height/2 else {
        throw Failure("Unsupported page viewport layout; close docked developer tools/side panels and recapture the window")
    }
    let origin = CGPoint(x: x, y: y)
    let viewport = try points.map { p -> CGPoint in
        let value = CGPoint(x: (p.x-x-insetX)/zoom, y: (p.y-y-insetY)/zoom)
        guard value.x >= 0, value.y >= 0, value.x < innerWidth, value.y < innerHeight else {
            throw Failure("Point is outside the browser page. Browser chrome/title-bar input is not sent to the desktop; use browser_open to open a URL")
        }
        return value
    }
    let action = args["action"] as? String ?? ""
    let before = state()
    _ = try call("Emulation.setFocusEmulationEnabled", ["enabled": true])
    defer { _ = try? call("Emulation.setFocusEmulationEnabled", ["enabled": false]) }
    var events = 0
    let button = args["button"] as? String == "right" ? "right" : "left"
    let mask = button == "right" ? 2 : 1
    func pointer(_ p: CGPoint?, pressed: Bool = false) {
        let local = p ?? (virtualPointer.windowID == record.windowID ? virtualPointer.local : nil)
        var presentationPoint: CGPoint?
        if let local = local,
           let presented = windows().first(where: { ($0["window_id"] as? Int) == record.windowID && ($0["pid"] as? Int32) == record.pid }),
           let px = (presented["x"] as? NSNumber)?.doubleValue,
           let py = (presented["y"] as? NSNumber)?.doubleValue,
           let pw = (presented["width"] as? NSNumber)?.doubleValue,
           let ph = (presented["height"] as? NSNumber)?.doubleValue {
            presentationPoint = CGPoint(x: px + local.x*pw/width, y: py + local.y*ph/height)
        }
        virtualPointer.activity(window: record.windowID, pid: record.pid, local: p, origin: origin,
            show: args["show_pointer"] as? Bool != false, pressed: pressed, presentationPoint: presentationPoint)
    }
    // A pen pointer has its own pointer ID, so page pointer capture cannot
    // turn the human mouse stream into an agent drag.
    func mouse(_ type: String, _ p: CGPoint, pressed: Bool = false) throws {
        _ = try call("Input.dispatchMouseEvent", ["type": type, "x": p.x, "y": p.y,
            "button": type == "mouseMoved" && !pressed ? "none" : button, "buttons": pressed ? mask : 0,
            "clickCount": type == "mouseMoved" ? 0 : 1, "modifiers": 0, "pointerType": "pen", "force": pressed ? 1.0 : 0])
        events += 1
        pointer(CGPoint(x: p.x*zoom+insetX, y: p.y*zoom+insetY), pressed: pressed && button == "left")
    }
    switch action {
    case "move": try mouse("mouseMoved", viewport[0])
    case "click", "drag":
        try mouse("mouseMoved", viewport[0])
        var last = viewport[0]
        var released = false
        // Register cleanup before the press: its acknowledgement can be lost
        // even when Chrome received it. Never replay the press on failure.
        defer { if !released { try? mouse("mouseReleased", last); _ = try? call("Input.cancelDragging") } }
        try mouse("mousePressed", viewport[0], pressed: true)
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.02))
        if action == "drag" {
            let duration = args["duration"] as? Double ?? (args["duration_ms"] as? Double ?? 500)/1000
            let lengths = zip(viewport, viewport.dropFirst()).map { hypot($1.x-$0.x, $1.y-$0.y) }
            let total = max(1, lengths.reduce(0,+))
            for i in 1..<viewport.count {
                let segmentTime = max(1.0/120, duration*lengths[i-1]/total)
                let steps = max(1, min(Int(ceil(segmentTime*120)), Int(ceil(lengths[i-1]/4))))
                for step in 1...steps {
                    let f = Double(step)/Double(steps)
                    last = CGPoint(x: viewport[i-1].x+(viewport[i].x-viewport[i-1].x)*f, y: viewport[i-1].y+(viewport[i].y-viewport[i-1].y)*f)
                    try mouse("mouseMoved", last, pressed: true)
                    RunLoop.current.run(until: Date(timeIntervalSinceNow: segmentTime/Double(steps)))
                }
            }
        }
        try mouse("mouseReleased", last); released = true
    case "type":
        _ = try call("Input.insertText", ["text": args["text"] as? String ?? ""]); events += 1; pointer(nil)
    case "key":
        guard let (name, nativeCode, flags) = recipe else { throw Failure("Missing validated key recipe") }
        let named: [String: (String, Int)] = ["enter":("Enter",13),"return":("Enter",13),"tab":("Tab",9),"space":(" ",32),"backspace":("Backspace",8),"escape":("Escape",27),"esc":("Escape",27),"delete":("Delete",46),"home":("Home",36),"end":("End",35),"pageup":("PageUp",33),"pagedown":("PageDown",34),"left":("ArrowLeft",37),"right":("ArrowRight",39),"up":("ArrowUp",38),"down":("ArrowDown",40)]
        let key = named[name]?.0 ?? (flags.contains(.maskShift) ? name.uppercased() : name)
        let code = name.count == 1 ? (name.first!.isNumber ? "Digit" : "Key")+name.uppercased() : (name == "space" ? "Space" : key)
        let vk = named[name]?.1 ?? Int(name.uppercased().utf8.first ?? 0)
        let mods = (flags.contains(.maskAlternate) ? 1 : 0) | (flags.contains(.maskControl) ? 2 : 0) | (flags.contains(.maskCommand) ? 4 : 0) | (flags.contains(.maskShift) ? 8 : 0)
        var down: [String: Any] = ["type":"rawKeyDown","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":nativeCode,"modifiers":mods]
        if flags.contains(.maskCommand), name == "a" { down["commands"] = ["selectAll"] }
        if flags.contains(.maskCommand), name == "z" { down["commands"] = [flags.contains(.maskShift) ? "redo" : "undo"] }
        if mods == 0 || mods == 8 {
            if key.count == 1 { down["type"] = "keyDown"; down["text"] = key }
            else if key == "Enter" { down["type"] = "keyDown"; down["text"] = "\r" }
        }
        var up = down
        up["type"] = "keyUp"; up.removeValue(forKey: "text"); up.removeValue(forKey: "commands")
        var released = false
        defer { if !released { _ = try? call("Input.dispatchKeyEvent", up) } }
        _ = try call("Input.dispatchKeyEvent", down); events += 1
        _ = try call("Input.dispatchKeyEvent", up); events += 1; released = true; pointer(nil)
    case "scroll":
        let p = viewport[0], delta = args["delta"] as? Int ?? 0
        try mouse("mouseMoved", p)
        if delta != 0 {
            _ = try call("Input.dispatchMouseEvent", ["type":"mouseWheel","x":p.x,"y":p.y,"deltaX":0,"deltaY":delta*40,"modifiers":0,"buttons":0,"button":"none"])
            events += 1
        }
    default: throw Failure("Unsupported browser input action")
    }
    RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.05))
    return ["ok":true,"action":action,"mode":"background","window_id":record.windowID,"pid":record.pid,
        "delivery":"chrome-devtools","native_input_events_posted":0,"input_pointer_type":"pen","input_events_posted":events,
        "coordinate_space":"window","viewport_inset_x":insetX,"viewport_inset_y":insetY,"page_zoom":zoom,
        "responder_lease":false,"before":before,"after":state(),"pointer":virtualPointer.metadata(),
        "pointer_color":"#7DD4FF","pointer_pressed_color":"#2563EB"]
}

// Capture the page surface, not its Stage Manager/Mission Control thumbnail.
// Reserve the real chrome inset so existing window-relative coordinates remain
// exact. The neutral top band explicitly marks the uncaptured browser UI.
func browserCapture(_ record: ManagedBrowser, window: [String: Any]) throws -> NSBitmapImageRep {
    let connection = try BrowserConnection(record.endpoint)
    let info = try connection.call("Browser.getWindowForTarget", ["targetId": record.targetID])
    guard info["windowId"] as? Int == record.browserWindowID else { throw Failure("Managed page moved to another window") }
    let attached = try connection.call("Target.attachToTarget", ["targetId": record.targetID, "flatten": true])
    guard let session = attached["sessionId"] as? String else { throw Failure("Cannot observe managed page") }
    let evaluated = try connection.call("Runtime.evaluate", ["expression": viewportExpression, "returnByValue": true], sessionID: session)
    guard let result = evaluated["result"] as? [String: Any], let geometry = result["value"] as? [String: Any],
          let innerWidth = geometry["width"] as? Double, let innerHeight = geometry["height"] as? Double,
          let dpr = geometry["dpr"] as? Double, let width = (window["width"] as? NSNumber)?.doubleValue,
          let height = (window["height"] as? NSNumber)?.doubleValue, let x = (window["x"] as? NSNumber)?.doubleValue,
          let y = (window["y"] as? NSNumber)?.doubleValue else { throw Failure("Cannot observe page geometry") }
    let top = NSScreen.screens.first?.frame.maxY ?? 0
    let center = NSPoint(x: x+width/2, y: top-y-height/2)
    let backing = NSScreen.screens.first(where: { $0.frame.contains(center) })?.backingScaleFactor ?? 1
    let zoom = dpr/backing
    let insetX = (width-innerWidth*zoom)/2, insetY = height-innerHeight*zoom
    guard zoom.isFinite, zoom > 0, abs(insetX) <= 3, insetY >= 0, insetY < height/2 else {
        throw Failure("Unsupported browser viewport; close docked developer tools or side panels")
    }
    let captured = try connection.call("Page.captureScreenshot", ["format":"png","fromSurface":true,"captureBeyondViewport":false], sessionID: session)
    guard let encoded = captured["data"] as? String, let bytes = Data(base64Encoded: encoded),
          let page = NSBitmapImageRep(data: bytes), let image = page.cgImage else { throw Failure("Browser returned no page image") }
    let scale = Double(page.pixelsWide)/(innerWidth*zoom)
    let fullWidth = Int((width*scale).rounded()), fullHeight = Int((height*scale).rounded())
    guard fullWidth > 0, fullHeight > 0, fullWidth <= 8192, fullHeight <= 8192,
          abs(Double(page.pixelsHigh)-innerHeight*zoom*scale) <= 3,
          let bitmap = NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:fullWidth,pixelsHigh:fullHeight,
              bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0),
          let context = NSGraphicsContext(bitmapImageRep:bitmap) else { throw Failure("Inconsistent browser screenshot geometry") }
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = context
    NSColor(calibratedWhite:0.94,alpha:1).setFill()
    NSRect(x:0,y:0,width:fullWidth,height:fullHeight).fill()
    NSImage(cgImage:image,size:NSSize(width:page.pixelsWide,height:page.pixelsHigh)).draw(in:
        NSRect(x:insetX*scale,y:0,width:Double(page.pixelsWide),height:Double(page.pixelsHigh)))
    if insetY*scale >= 20 {
        ("BACKGROUND PAGE CAPTURE  |  Browser toolbar not captured" as NSString).draw(
            at:NSPoint(x:12*scale,y:Double(fullHeight)-20*scale),
            withAttributes:[.font:NSFont.systemFont(ofSize:10*scale),.foregroundColor:NSColor.gray])
    }
    NSGraphicsContext.restoreGraphicsState()
    return bitmap
}
