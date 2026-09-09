import AppKit
// Diagnostic recipient for verifying the NSEvent type after process-directed
// delivery. This program neither injects input nor activates the foreground app.
final class Surface: NSView {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    override var acceptsFirstResponder: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        NSColor.white.setFill(); bounds.fill()
        ("Native background event receiver" as NSString).draw(at: NSPoint(x: 30, y: 160), withAttributes: [.font: NSFont.systemFont(ofSize: 22)])
    }
    override func mouseDown(with event: NSEvent) {}
    override func mouseUp(with event: NSEvent) {}
    override func mouseDragged(with event: NSEvent) {}
    override func mouseMoved(with event: NSEvent) {}
}
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let window = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 700, height: 500), styleMask: [.titled, .closable, .resizable], backing: .buffered, defer: false)
window.title = "Lessagent Native Event Probe"
window.contentView = Surface(frame: NSRect(x: 0, y: 0, width: 700, height: 500))
window.acceptsMouseMovedEvents = true
window.orderFrontRegardless()
let monitor = NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved, .leftMouseDown, .leftMouseUp, .leftMouseDragged, .rightMouseDown, .rightMouseUp, .rightMouseDragged]) { event in
    let info: [String: Any] = ["ns_type": event.type.rawValue, "cg_type": event.cgEvent?.type.rawValue ?? 0,
        "x": event.locationInWindow.x, "y": event.locationInWindow.y, "button": event.buttonNumber,
        "pressed_buttons": NSEvent.pressedMouseButtons, "subtype": event.subtype.rawValue,
        "flags": event.modifierFlags.rawValue]
    if let data = try? JSONSerialization.data(withJSONObject: info, options: [.sortedKeys]) {
        FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
    }
    return event
}
app.run()
