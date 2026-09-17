import AppKit

// Only displays Lessagent-generated events. Never monitors the user's keyboard.
// Text payloads are deliberately represented by a character count, not echoed.
struct KeyboardNoticeState {
    var label = ""
    var application = ""
    var phases: [String] = []
    var touched: TimeInterval?
    static let lifetime: TimeInterval = 5
    func visible(at now: TimeInterval) -> Bool {
        guard let touched else { return false }
        return now - touched < Self.lifetime
    }
    mutating func begin(label: String, application: String) {
        self.label = label; self.application = application
        phases.removeAll(); touched = nil
    }
    mutating func record(_ phase: String, at now: TimeInterval) {
        if phases.last != phase { phases.append(phase) }
        if phases.count > 3 { phases.removeFirst(phases.count - 3) }
        touched = now
    }
}
final class KeyboardNotice {
    private var panel: NSPanel?
    private var heading: NSTextField?
    private var events: NSTextField?
    private var timer: Timer?
    private var state = KeyboardNoticeState()
    private var pid: Int32 = 0
    private var windowID = 0
    private var now: TimeInterval { ProcessInfo.processInfo.systemUptime }
    func begin(_ args: [String: Any], pid: Int32, windowID: Int) {
        guard let action = args["action"] as? String, ["type", "key"].contains(action) else { return }
        timer?.invalidate(); panel?.orderOut(nil)
        let label = action == "key" ? String((args["key"] as? String ?? "Key").prefix(64)) : "Text entry · \((args["text"] as? String ?? "").count) characters"
        self.pid = pid; self.windowID = windowID
        state.begin(label: label, application: NSRunningApplication(processIdentifier: pid)?.localizedName ?? "Target app")
    }
    func record(_ phase: String) {
        state.record(phase, at: now)
        timer?.invalidate()
        show()
        timer = Timer(timeInterval: KeyboardNoticeState.lifetime, repeats: false) { [weak self] _ in
            guard let self, !self.state.visible(at: self.now) else { return }
            self.panel?.orderOut(nil)
        }
        RunLoop.main.add(timer!, forMode: .common)
    }
    private func show() {
        if panel == nil {
            let p = PointerPanel(contentRect: NSRect(x: 0, y: 0, width: 320, height: 86),
                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            p.title = "Lessagent virtual keyboard"
            p.isReleasedWhenClosed = false
            p.isOpaque = false; p.backgroundColor = .clear
            p.hasShadow = true; p.hidesOnDeactivate = false
            p.ignoresMouseEvents = true; p.level = .floating
            p.animationBehavior = .none
            p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .ignoresCycle, .stationary]
            if #available(macOS 13.0, *) { p.collectionBehavior.insert(.canJoinAllApplications) }
            let view = NSView(frame: NSRect(x: 0, y: 0, width: 320, height: 86))
            view.wantsLayer = true; view.layer?.cornerRadius = 12
            view.layer?.backgroundColor = NSColor(calibratedRed: 0.10, green: 0.18, blue: 0.29, alpha: 0.96).cgColor
            let h = NSTextField(labelWithString: "")
            h.frame = NSRect(x: 16, y: 46, width: 288, height: 22)
            h.font = .systemFont(ofSize: 14, weight: .semibold); h.textColor = .white
            h.lineBreakMode = .byTruncatingMiddle
            let e = NSTextField(labelWithString: "")
            e.frame = NSRect(x: 16, y: 18, width: 288, height: 20)
            e.font = .monospacedSystemFont(ofSize: 12, weight: .medium)
            e.textColor = NSColor(calibratedRed: 0.65, green: 0.84, blue: 1, alpha: 1)
            view.addSubview(h); view.addSubview(e); p.contentView = view
            panel = p; heading = h; events = e
        }
        guard let panel, let screen = NSScreen.screens.first else { return }
        let visible = screen.visibleFrame
        // Keep the service-error corner above this independent key notice.
        panel.setFrameOrigin(NSPoint(x: visible.maxX - 336, y: visible.maxY - 188))
        heading?.stringValue = "\(state.application)  ·  \(state.label)"
        events?.stringValue = state.phases.map { phase in
            switch phase { case "down": return "↓ DOWN"; case "up": return "↑ UP"; case "click": return "✓ CLICK"; default: return phase.uppercased() }
        }.joined(separator: "   ")
        panel.orderFrontRegardless(); panel.displayIfNeeded()
    }
    func metadata() -> [String: Any] {
        ["visible": state.visible(at: now) && (panel?.isVisible ?? false), "hide_after_seconds": 5,
         "label": state.label, "application": state.application, "phases": state.phases,
         "pid": pid, "window_id": windowID, "panel_window_id": panel?.windowNumber ?? 0,
         "nonactivating": true, "text_payload_redacted": true]
    }
}
let keyboardNotice = KeyboardNotice()
