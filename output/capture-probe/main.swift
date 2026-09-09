import AppKit
import ScreenCaptureKit

final class Box<T>: @unchecked Sendable {
    let lock=NSLock(); var result: Result<T,Error>?
    func set(_ value: Result<T,Error>) { lock.lock(); result=value; lock.unlock() }
    func get() -> Result<T,Error>? { lock.lock(); defer { lock.unlock() }; return result }
}
func wait<T>(_ start: (Box<T>)->Void) throws -> T {
    let box=Box<T>(); start(box); let end=Date(timeIntervalSinceNow:12)
    while Date()<end { if let value=box.get() { return try value.get() }; RunLoop.current.run(until:Date(timeIntervalSinceNow:0.01)) }
    throw NSError(domain:"Capture timeout",code:1)
}
let app=NSApplication.shared
app.setActivationPolicy(.accessory)
guard CGPreflightScreenCaptureAccess(),CommandLine.arguments.count==4 else { fatalError("Usage: capture window_id pid output.png; Screen Recording must already be granted") }
let id=UInt32(CommandLine.arguments[1])!,pid=Int32(CommandLine.arguments[2])!
let content:SCShareableContent=try wait { box in
    SCShareableContent.getExcludingDesktopWindows(true,onScreenWindowsOnly:false) { value,error in
        if let value=value { box.set(.success(value)) } else { box.set(.failure(error ?? NSError(domain:"No content",code:2))) }
    }
}
guard let window=content.windows.first(where:{$0.windowID==id && $0.owningApplication?.processID==pid}) else { fatalError("Exact target not available") }
let filter=SCContentFilter(desktopIndependentWindow:window)
let config=SCStreamConfiguration()
config.width=2000;config.height=1500;config.showsCursor=false
config.ignoreShadowsSingleWindow=true
config.captureResolution = .best
if #available(macOS 14.2,*) { config.includeChildWindows=false }
let image:CGImage=try wait { box in
    SCScreenshotManager.captureImage(contentFilter:filter,configuration:config) { value,error in
        if let value=value { box.set(.success(value)) } else { box.set(.failure(error ?? NSError(domain:"No image",code:3))) }
    }
}
let bitmap=NSBitmapImageRep(cgImage:image)
try bitmap.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:CommandLine.arguments[3]))
print("Captured",image.width,image.height,"window frame",window.frame,"content",filter.contentRect,"scale",filter.pointPixelScale)
