#!/usr/bin/env python3
"""Compile and test the production native thumbnail rectifier, without GUI input.
Usage: python3 tests/native_capture.py
Uses debug Swift compilation; no app activation, screen capture or permissions.
"""
import pathlib
import subprocess
import sys
import tempfile

if sys.platform != 'darwin':
    raise SystemExit('This native CoreImage regression requires macOS')
root = pathlib.Path(__file__).resolve().parents[1]
source = (root / 'native/window.swift').read_text()
# Compile the actual production function, not a separately maintained copy.
function = source[source.index('func rectifyNativeThumbnail('):source.index('\nfunc captureNativeThumbnail(')]
fixture = r'''
import AppKit
import CoreImage
struct Failure: Error { let message: String; init(_ message: String) { self.message = message } }
'''
checks = r'''
let application = NSApplication.shared
application.setActivationPolicy(.prohibited)
func require(_ condition: Bool, _ message: String) {
    if !condition { print("FAIL: " + message); exit(1) }
}
func bitmap(_ size: Int = 240) -> CGContext {
    CGContext(data:nil, width:size, height:size, bitsPerComponent:8, bytesPerRow:0,
              space:CGColorSpaceCreateDeviceRGB(), bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
}
let canvas = bitmap()
for (rect, components) in [
    (CGRect(x:0,y:0,width:120,height:120), [0.1,0.2,0.9,1.0]),
    (CGRect(x:120,y:0,width:120,height:120), [0.9,0.8,0.1,1.0]),
    (CGRect(x:0,y:120,width:120,height:120), [0.9,0.1,0.2,1.0]),
    (CGRect(x:120,y:120,width:120,height:120), [0.1,0.8,0.2,1.0]),
] {
    canvas.setFillColor(CGColor(colorSpace:CGColorSpaceCreateDeviceRGB(),components:components.map { CGFloat($0) })!)
    canvas.fill(rect)
}
let original = canvas.makeImage()!
let reference = NSBitmapImageRep(cgImage:original)
let variants: [[CGPoint]] = [
    [.init(x:0,y:240),.init(x:240,y:240),.init(x:240,y:0),.init(x:0,y:0)],
    [.init(x:0,y:193),.init(x:235,y:238),.init(x:237,y:46),.init(x:1,y:0)],
    [.init(x:4,y:235),.init(x:238,y:190),.init(x:235,y:4),.init(x:0,y:42)],
    [.init(x:20,y:237),.init(x:220,y:230),.init(x:235,y:8),.init(x:4,y:2)],
]
for (index, corners) in variants.enumerated() {
    let transform = CIFilter(name:"CIPerspectiveTransform")!
    transform.setValue(CIImage(cgImage:original),forKey:kCIInputImageKey)
    for (key,point) in zip(["inputTopLeft","inputTopRight","inputBottomRight","inputBottomLeft"],corners) {
        transform.setValue(CIVector(cgPoint:point),forKey:key)
    }
    let distorted = transform.outputImage!
    let image = CIContext(options:[.cacheIntermediates:false]).createCGImage(distorted,from:distorted.extent)!
    let actual = try rectifyNativeThumbnail(image,aspect:1)
    require(actual.pixelsWide == actual.pixelsHigh,"rectified aspect ratio \(index)")
    for fx in [0.25,0.75] {
        for fy in [0.25,0.75] {
            let expected = reference.colorAt(x:Int(fx*240),y:Int(fy*240))!.usingColorSpace(.deviceRGB)!
            let found = actual.colorAt(x:Int(fx*Double(actual.pixelsWide)),y:Int(fy*Double(actual.pixelsHigh)))!.usingColorSpace(.deviceRGB)!
            require(abs(expected.redComponent-found.redComponent)<0.12 &&
                    abs(expected.greenComponent-found.greenComponent)<0.12 &&
                    abs(expected.blueComponent-found.blueComponent)<0.12,
                    "corner identity/orientation \(index) \(fx),\(fy)")
        }
    }
    print("PASS: native perspective/orientation variant \(index)")
}
for aspect in [0.0, Double.nan, Double.infinity, Double.leastNonzeroMagnitude] {
    do { _ = try rectifyNativeThumbnail(original,aspect:aspect); require(false,"invalid aspect accepted") }
    catch { print("PASS: invalid/overflowing aspect rejected") }
}
for image in [bitmap(8).makeImage()!, bitmap().makeImage()!] {
    do { _ = try rectifyNativeThumbnail(image,aspect:1); require(false,"empty/tiny image accepted") }
    catch { print("PASS: empty or tiny image rejected") }
}
let circle = bitmap()
circle.setFillColor(CGColor(gray:1,alpha:1)); circle.fillEllipse(in:CGRect(x:0,y:0,width:240,height:240))
do { _ = try rectifyNativeThumbnail(circle.makeImage()!,aspect:1); require(false,"non-window alpha edges accepted") }
catch { print("PASS: ambiguous/non-window alpha edges rejected") }
print("PASS: 11 production rectifier checks; no GUI capture or activation")
'''
with tempfile.TemporaryDirectory(prefix='lessagent-native-capture-test-') as directory:
    directory = pathlib.Path(directory)
    main = directory/'main.swift'
    main.write_text(fixture + function + checks)
    binary = directory/'native-capture-test'
    subprocess.run(['xcrun','swiftc','-Onone','-g',str(main),'-o',str(binary)],check=True)
    subprocess.run([str(binary)],check=True)
