import AppKit
import ApplicationServices
import ScreenCaptureKit
import CoreImage

// Quartz window bounds may describe a Stage Manager thumbnail, not the NSWindow
// receiving input. Resolve the exact AX window by ID (never title/order).
func nativeWindowElement(_ native: [String: Any]) -> AXUIElement? {
    guard AXIsProcessTrusted(), let pid = native["pid"] as? Int32,
          let id = native["window_id"] as? Int,
          let symbol = dlsym(UnsafeMutableRawPointer(bitPattern: -2), "_AXUIElementGetWindow") else { return nil }
    typealias GetWindow = @convention(c) (AXUIElement, UnsafeMutablePointer<CGWindowID>) -> AXError
    let getWindow = unsafeBitCast(symbol, to: GetWindow.self)
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 1)
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(app, kAXWindowsAttribute as CFString, &value) == .success,
          let elements = value as? [AXUIElement] else { return nil }
    return elements.first { element in
        var number: CGWindowID = 0
        return getWindow(element, &number) == .success && Int(number) == id
    }
}

func nativeWindowGeometry(_ native: [String: Any], required: Bool) throws -> [String: Any] {
    func unavailable() throws -> [String: Any] {
        if required { throw Failure("Exact native window geometry unavailable. Enable Accessibility and select the window again; no input was sent") }
        var result = native; result["geometry_source"] = "quartz-presentation"
        return result
    }
    guard let element = nativeWindowElement(native) else { return try unavailable() }
    var position: CFTypeRef?, size: CFTypeRef?, minimized: CFTypeRef?
    AXUIElementCopyAttributeValue(element, kAXMinimizedAttribute as CFString, &minimized)
    if required, minimized as? Bool == true { throw Failure("Native target is minimized; no input was sent") }
    guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &position) == .success,
          AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &size) == .success,
          let position = position, let size = size,
          CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID() else { return try unavailable() }
    var point = CGPoint.zero, dimensions = CGSize.zero
    guard AXValueGetValue(position as! AXValue, .cgPoint, &point),
          AXValueGetValue(size as! AXValue, .cgSize, &dimensions),
          [point.x, point.y, dimensions.width, dimensions.height].allSatisfy({ $0.isFinite }),
          dimensions.width > 1, dimensions.height > 1,
          dimensions.width <= 32768, dimensions.height <= 32768 else { return try unavailable() }
    var result = native
    for (key, value) in [("x", point.x), ("y", point.y), ("width", dimensions.width), ("height", dimensions.height)] {
        result["presentation_" + key] = native[key]
        result[key] = value
    }
    result["geometry_source"] = "accessibility-window-id"
    result["minimized"] = minimized as? Bool ?? false
    return result
}

// Some WebKit/Catalyst-style apps intentionally ignore process-addressed
// CGEvents while they are backgrounded, even though AppKit/GHOST apps accept
// them. For ordinary left-clicks on semantic controls, use the exact target
// window's Accessibility tree first. AXPress is recipient-local and does not
// activate the application or touch the global pointer. Canvas/viewport clicks
// have no eligible semantic control and continue through the CGEvent path.
func nativeAccessibilityPress(_ native: [String: Any], at point: CGPoint) -> [String: Any]? {
    guard let window = nativeWindowElement(native) else { return nil }
    let pressRoles: Set<String> = [
        "AXButton", "AXRadioButton", "AXCheckBox", "AXPopUpButton",
        "AXComboBox", "AXTextField", "AXTextArea", "AXLink",
        "AXDisclosureTriangle", "AXMenuItem"
    ]
    func stringAttribute(_ element: AXUIElement, _ name: String) -> String {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success else { return "" }
        return value as? String ?? ""
    }
    func frame(_ element: AXUIElement) -> CGRect? {
        var position: CFTypeRef?, size: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &position) == .success,
              AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &size) == .success,
              let position, let size, CFGetTypeID(position) == AXValueGetTypeID(),
              CFGetTypeID(size) == AXValueGetTypeID() else { return nil }
        var origin = CGPoint.zero, dimensions = CGSize.zero
        guard AXValueGetValue(position as! AXValue, .cgPoint, &origin),
              AXValueGetValue(size as! AXValue, .cgSize, &dimensions),
              [origin.x, origin.y, dimensions.width, dimensions.height].allSatisfy({ $0.isFinite }),
              dimensions.width > 0, dimensions.height > 0 else { return nil }
        return CGRect(origin: origin, size: dimensions)
    }
    func canPress(_ element: AXUIElement) -> Bool {
        var actions: CFArray?
        guard AXUIElementCopyActionNames(element, &actions) == .success,
              let names = actions as? [String] else { return false }
        return names.contains(kAXPressAction as String)
    }
    var visited = 0
    var best: (element: AXUIElement, depth: Int, area: CGFloat, role: String)?
    func visit(_ element: AXUIElement, depth: Int) {
        guard depth <= 20, visited < 1024 else { return }
        visited += 1
        if depth > 0, let bounds = frame(element), !bounds.contains(point) { return }
        let role = stringAttribute(element, kAXRoleAttribute)
        if pressRoles.contains(role), canPress(element), let bounds = frame(element), bounds.contains(point) {
            let area = bounds.width * bounds.height
            if best == nil || depth > best!.depth || (depth == best!.depth && area < best!.area) {
                best = (element, depth, area, role)
            }
        }
        var childrenValue: CFTypeRef?
        if AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &childrenValue) == .success,
           let children = childrenValue as? [AXUIElement] {
            for child in children { visit(child, depth: depth + 1) }
        }
    }
    visit(window, depth: 0)
    guard let best, AXUIElementPerformAction(best.element, kAXPressAction as CFString) == .success else { return nil }
    var focused: CFTypeRef?
    AXUIElementCopyAttributeValue(best.element, kAXFocusedAttribute as CFString, &focused)
    return [
        "action": "AXPress", "role": best.role,
        "title": stringAttribute(best.element, kAXTitleAttribute),
        "focused": focused as? Bool ?? false, "nodes_visited": visited
    ]
}

// Fit the four straight alpha-mask edges, excluding rounded corners. Stage
// Manager renders a perspective thumbnail; rectifying it restores a true
// window-local coordinate system, but does NOT invent full-resolution detail.
func rectifyNativeThumbnail(_ image: CGImage, aspect: Double) throws -> NSBitmapImageRep {
    let bitmap = NSBitmapImageRep(cgImage: image)
    let width = image.width, height = image.height
    guard width >= 16, height >= 16, aspect.isFinite, aspect > 0 else { throw Failure("Native thumbnail is unavailable or too small") }
    func opaque(_ x: Int, _ y: Int) -> Bool { (bitmap.colorAt(x: x, y: y)?.alphaComponent ?? 0) > 0.9 }
    func fit(_ points: [(Double, Double)]) throws -> (Double, Double) {
        guard points.count >= 4 else { throw Failure("Cannot find native thumbnail edges; no input was sent") }
        let n = Double(points.count), sx = points.reduce(0) { $0 + $1.0 }, sy = points.reduce(0) { $0 + $1.1 }
        let sxx = points.reduce(0) { $0 + $1.0 * $1.0 }, sxy = points.reduce(0) { $0 + $1.0 * $1.1 }
        let d = n * sxx - sx * sx
        guard abs(d) > 0.01 else { throw Failure("Degenerate native thumbnail edges") }
        let a = (n * sxy - sx * sy) / d, b = (sy - a * sx) / n
        guard points.allSatisfy({ abs($0.1 - a * $0.0 - b) < 3 }) else { throw Failure("Native thumbnail is transitioning; capture again before input") }
        return (a, b)
    }
    var left = [(Double, Double)](), right = left, top = left, bottom = left
    for y in (height * 3 / 10)...(height * 7 / 10) {
        if let l = (0..<width).first(where: { opaque($0, y) }), let r = (0..<width).reversed().first(where: { opaque($0, y) }) {
            left.append((Double(y), Double(l))); right.append((Double(y), Double(r)))
        }
    }
    for x in (width * 3 / 10)...(width * 7 / 10) {
        if let t = (0..<height).first(where: { opaque(x, $0) }), let b = (0..<height).reversed().first(where: { opaque(x, $0) }) {
            top.append((Double(x), Double(t))); bottom.append((Double(x), Double(b)))
        }
    }
    let l = try fit(left), r = try fit(right), t = try fit(top), b = try fit(bottom)
    func corner(_ v: (Double,Double), _ h: (Double,Double)) throws -> CIVector {
        let denominator = 1 - v.0 * h.0
        guard abs(denominator) > 0.01 else { throw Failure("Degenerate native thumbnail perspective") }
        let x = (v.0 * h.1 + v.1) / denominator, y = h.0 * x + h.1
        guard x >= -4, y >= -4, x <= Double(width) + 4, y <= Double(height) + 4 else { throw Failure("Native thumbnail boundaries are ambiguous; capture again") }
        return CIVector(x: x, y: Double(height) - y)
    }
    guard let filter = CIFilter(name: "CIPerspectiveCorrection") else { throw Failure("Native thumbnail correction is unavailable") }
    filter.setValue(CIImage(cgImage: image), forKey: kCIInputImageKey)
    filter.setValue(try corner(l,t), forKey: "inputTopLeft"); filter.setValue(try corner(r,t), forKey: "inputTopRight")
    filter.setValue(try corner(r,b), forKey: "inputBottomRight"); filter.setValue(try corner(l,b), forKey: "inputBottomLeft")
    guard let corrected = filter.outputImage, let raw = CIContext(options: [.cacheIntermediates: false]).createCGImage(corrected, from: corrected.extent) else { throw Failure("Cannot rectify native thumbnail") }
    let projectedHeight = (Double(width) / aspect).rounded()
    guard projectedHeight.isFinite, projectedHeight >= 1, projectedHeight <= 32768, width <= 32768, Double(width) * projectedHeight <= 32_000_000 else { throw Failure("Native thumbnail dimensions exceed the image limit") }
    let outWidth = width, outHeight = Int(projectedHeight)
    guard let context = CGContext(data: nil, width: outWidth, height: outHeight, bitsPerComponent: 8, bytesPerRow: 0, space: CGColorSpaceCreateDeviceRGB(), bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { throw Failure("Cannot allocate native thumbnail") }
    context.interpolationQuality = .high
    context.draw(raw, in: CGRect(x: 0, y: 0, width: outWidth, height: outHeight))
    guard let result = context.makeImage() else { throw Failure("Cannot encode native thumbnail") }
    return NSBitmapImageRep(cgImage: result)
}

func captureNativeThumbnail(_ id: Int, geometry: [String:Any]) throws -> NSBitmapImageRep {
    guard CGPreflightScreenCaptureAccess(), let symbol = nativeSymbol("CGWindowListCreateImage") else { throw Failure("Screen Recording permission or native window capture is unavailable") }
    guard geometry["geometry_source"] as? String == "accessibility-window-id" else { throw Failure("Accessibility is required to map a transformed native window accurately") }
    typealias Capture = @convention(c) (CGRect, UInt32, UInt32, UInt32) -> Unmanaged<CGImage>?
    let capture = unsafeBitCast(symbol, to: Capture.self)
    guard let image = capture(CGRect.null, 8, UInt32(id), 1 | 8)?.takeRetainedValue() else { throw Failure("Native thumbnail is unavailable; capture again without replaying input") }
    let aspect = (geometry["width"] as! NSNumber).doubleValue / (geometry["height"] as! NSNumber).doubleValue
    return try rectifyNativeThumbnail(image, aspect: aspect)
}

// Public full-window capture for ordinary background windows. Reject transformed
// descriptors before constructing an SCK stream (macOS 27 can poison replayd).
@available(macOS 14.0, *)
func captureNativeWindow(_ id: Int, pid: Int32, geometry: [String:Any]) throws -> (NSBitmapImageRep, Bool) {
    let width = (geometry["width"] as! NSNumber).doubleValue, height = (geometry["height"] as! NSNumber).doubleValue
    let pw = (geometry["presentation_width"] as? NSNumber)?.doubleValue ?? width
    let ph = (geometry["presentation_height"] as? NSNumber)?.doubleValue ?? height
    if abs(pw - width) > 3 || abs(ph - height) > 3 { return (try captureNativeThumbnail(id, geometry: geometry), true) }
    let content: SCShareableContent = try awaitLocal(8) { box in
        SCShareableContent.getExcludingDesktopWindows(true, onScreenWindowsOnly: false) { content, error in
            if let error = error { box.set(.failure(error)) }
            else if let content = content { box.set(.success(content)) }
            else { box.set(.failure(Failure("Native capture returned no shareable windows"))) }
        }
    }
    guard let window = content.windows.first(where: { Int($0.windowID) == id && $0.owningApplication?.processID == pid }) else { throw Failure("Exact native window is unavailable; no application was activated") }
    guard abs(window.frame.width - width) <= 3, abs(window.frame.height - height) <= 3 else { return (try captureNativeThumbnail(id, geometry: geometry), true) }
    let filter = SCContentFilter(desktopIndependentWindow: window)
    let config = SCStreamConfiguration()
    let scale = CGFloat(filter.pointPixelScale)
    config.width = max(1, Int(ceil(width * scale))); config.height = max(1, Int(ceil(height * scale)))
    guard config.width <= 32768, config.height <= 32768 else { throw Failure("Native capture dimensions exceed the image limit") }
    config.showsCursor = false; config.ignoreShadowsSingleWindow = true; config.ignoreGlobalClipSingleWindow = true
    let image: CGImage = try awaitLocal(8) { box in
        SCScreenshotManager.captureImage(contentFilter: filter, configuration: config) { image, error in
            if let error = error { box.set(.failure(error)) }
            else if let image = image { box.set(.success(image)) }
            else { box.set(.failure(Failure("Native window capture returned no image"))) }
        }
    }
    return (NSBitmapImageRep(cgImage: image), false)
}

// LaunchServices nonactivation is honored by AppKit apps; Blender also needs its
// own --no-window-focus flag because its startup otherwise explicitly activates.
func openNativeApplication(_ args: [String: Any]) throws -> [String: Any] {
    guard let name = args["app"] as? String, !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { throw Failure("app_open requires an installed app name, bundle ID or .app path") }
    if args["new_instance"] != nil && args["new_instance"] as? Bool == nil { throw Failure("new_instance must be a boolean") }
    let url: URL?
    if name.hasPrefix("/") { url = URL(fileURLWithPath: name) }
    else {
        let filename = name.hasSuffix(".app") ? name : name + ".app"
        let folders = ["/Applications", NSHomeDirectory() + "/Applications", "/System/Applications", "/System/Applications/Utilities"]
        url = NSWorkspace.shared.urlForApplication(withBundleIdentifier: name) ?? folders.map { URL(fileURLWithPath: $0).appendingPathComponent(filename) }.first { FileManager.default.fileExists(atPath: $0.path) }
    }
    guard let url = url, url.pathExtension.lowercased() == "app", let bundle = Bundle(url: url), let identifier = bundle.bundleIdentifier else { throw Failure("Installed application could not be resolved; no app was opened") }
    guard !identifier.hasPrefix("com.google.Chrome") else { throw Failure("Chrome requires browser_open and the persistent managed profile; no app was opened") }
    let before = state()
    let createNew = args["new_instance"] as? Bool ?? true
    func result(_ window: [String: Any], reused: Bool) -> [String: Any] {
        let after = state()
        return ["ok": true, "action": "app_open", "mode": "background", "window_id": window["window_id"]!, "pid": window["pid"]!, "app": identifier, "reused": reused, "delivery": "native-app-launch", "before": before, "after": after, "focus_changed": before["frontmost_pid"] as? Int32 != after["frontmost_pid"] as? Int32, "input_events_posted": 0]
    }
    if !createNew {
        let pids = Set(NSRunningApplication.runningApplications(withBundleIdentifier: identifier).map { $0.processIdentifier })
        let existing = windows().filter { pids.contains($0["pid"] as? Int32 ?? 0) && !($0["title"] as? String ?? "").isEmpty }
        if existing.count == 1 { return result(existing[0], reused: true) }
        if !pids.isEmpty { throw Failure("Existing application has no unique window. Use list_windows and choose window_id/pid; no app was opened or raised") }
    }
    let configuration = NSWorkspace.OpenConfiguration()
    configuration.activates = false
    configuration.addsToRecentItems = false
    configuration.createsNewApplicationInstance = createNew
    if identifier == "org.blenderfoundation.blender" {
        guard let screen = primaryScreen() else { throw Failure("Display unavailable; no app was opened") }
        let frame = screen.visibleFrame
        let width = min(1000, Int(frame.width)), height = min(600, Int(frame.height))
        guard width >= 640, height >= 480 else { throw Failure("Display work area is too small; no app was opened") }
        configuration.arguments = ["--no-window-focus", "--window-geometry", String(Int(frame.midX) - width / 2), String(Int(frame.midY) - height / 2), String(width), String(height)]
    }
    let application: NSRunningApplication = try awaitLocal(15) { box in
        NSWorkspace.shared.openApplication(at: url, configuration: configuration) { application, error in
            if let error = error { box.set(.failure(error)) }
            else if let application = application { box.set(.success(application)) }
            else { box.set(.failure(Failure("Application launch returned no process; inspect list_windows before retrying"))) }
        }
    }
    for _ in 0..<150 {
        let candidates = windows().filter { $0["pid"] as? Int32 == application.processIdentifier && !($0["title"] as? String ?? "").isEmpty }
        if candidates.count == 1 { return result(candidates[0], reused: false) }
        if application.isTerminated { throw Failure("Application exited during launch") }
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.1))
    }
    throw Failure("App launched but no unique window appeared. Inspect list_windows before retrying; it was not restarted")
}
