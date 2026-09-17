#!/usr/bin/env python3
"""Compile the exact production read-only selector with a disposable Swift fixture."""
from pathlib import Path
import subprocess
import tempfile
import json

ROOT=Path(__file__).resolve().parent.parent
source=(ROOT/'native/browser.swift').read_text()
start=source.index('func chromeProfileSourceRoot()')
end=source.index('private let existingProfileControlGuidance',start)
selector=source[start:end]
fixture=r'''
import Foundation
let fm = FileManager.default
let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try fm.createDirectory(at: root, withIntermediateDirectories: true)
func add(_ name: String) throws {
    let dir = root.appendingPathComponent(name)
    try fm.createDirectory(at: dir, withIntermediateDirectories: true)
    try Data("{}".utf8).write(to: dir.appendingPathComponent("Preferences"))
    try Data("retain this fixture data".utf8).write(to: dir.appendingPathComponent("marker"))
}
func config(_ profile: [String: Any]) throws {
    try JSONSerialization.data(withJSONObject: ["profile":profile]).write(to:root.appendingPathComponent("Local State"))
}
func expect(_ value: String?, _ name: String) {
    precondition(existingChromeProfileName(root) == value, name)
    print("PASS: " + name)
}
expect(nil, "no original profile gives no silent blank fallback")
try add("Profile 10"); try add("Profile 2")
expect("Profile 2", "numeric ordering chooses Profile 2 before Profile 10")
try add("Default")
expect("Default", "Default is first when picker order is absent")
try config(["last_used":"Profile 10", "last_active_profiles":["Profile 10"]])
expect("Default", "last-used and activity do not change the first profile")
try config(["profiles_order":["Profile 2","Default","Profile 10"],"last_used":"Profile 10"])
expect("Profile 2", "explicit user picker order is respected")
try config(["profiles_order":["Profile 99","Default","Profile 2"]])
expect("Default", "missing picker entries do not create empty profiles")
try config(["profiles_order":["Profile 10/../../escape","Default"]])
expect("Default", "path traversal is rejected")
try config(["profiles_order":["Default","Profile 10"],"last_used":"Profile 10"])
let before = try Data(contentsOf:root.appendingPathComponent("Local State"))
for _ in 0..<10 { expect("Default", "stable repeated selection") }
precondition(try Data(contentsOf:root.appendingPathComponent("Local State")) == before)
for name in ["Default","Profile 2","Profile 10"] {
    precondition(try String(contentsOf:root.appendingPathComponent(name).appendingPathComponent("marker"),encoding:.utf8) == "retain this fixture data")
}
print("PASS: selection is read-only and preserves profile data")
'''
# Throwing calls must be evaluated outside precondition's nonthrowing autoclosure.
fixture=fixture.replace('precondition(try Data(contentsOf:root.appendingPathComponent("Local State")) == before)', 'let after = try Data(contentsOf:root.appendingPathComponent("Local State")); precondition(after == before)')
fixture=fixture.replace('precondition(try String(contentsOf:root.appendingPathComponent(name).appendingPathComponent("marker"),encoding:.utf8) == "retain this fixture data")', 'let marker = try String(contentsOf:root.appendingPathComponent(name).appendingPathComponent("marker"),encoding:.utf8); precondition(marker == "retain this fixture data")')
with tempfile.TemporaryDirectory(prefix='lessagent-selector-') as tmp:
    temp=Path(tmp);main=temp/'main.swift';main.write_text('import Foundation\n'+selector+'\n'+fixture)
    binary=temp/'selector-tests'
    subprocess.run(['xcrun','swiftc','-Onone','-g',str(main),'-o',str(binary)],check=True)
    run=subprocess.run([str(binary),str(temp/'fixture')],text=True,capture_output=True,check=True)
    print(run.stdout,end='')
    report=ROOT/'output/profile-safety-validation';report.mkdir(parents=True,exist_ok=True)
    (report/'selection-report.json').write_text(json.dumps({'passed':True,'cases':18,'compiler_profile':'-Onone -g','output':run.stdout},indent=2))
