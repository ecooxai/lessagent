#!/usr/bin/env python3
"""Native app_open launch/reuse regression over real MCP HTTP and stdio.

Run: uv run --with 'mcp>=1.20,<2' tests/app_open_mcp.py target/debug/lessagent
Only a disposable debug backend and owned AppKit fixtures are started/stopped.
No personal apps/profiles, physical input, or foreground activation are used.
Requires macOS Accessibility and Screen Recording permissions.
"""
from __future__ import annotations

import argparse
import asyncio
import base64
from contextlib import asynccontextmanager
import datetime
import json
import os
from pathlib import Path
import plistlib
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
import uuid

from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client
from mcp.client.streamable_http import streamablehttp_client

ROOT = Path(__file__).resolve().parent.parent
SOURCE = r'''
import AppKit
let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let window = NSWindow(contentRect: NSRect(x: 160, y: 160, width: 640, height: 400),
    styleMask: [.titled, .closable], backing: .buffered, defer: false)
window.title = "Lessagent app_open regression fixture"
let label = NSTextField(labelWithString: "Native launcher: owned test window")
label.frame = NSRect(x: 24, y: 160, width: 570, height: 32)
label.font = NSFont.systemFont(ofSize: 22)
window.contentView!.addSubview(label)
window.orderBack(nil)
app.run()
'''


def request(base: str, path: str, body: dict | None = None) -> dict:
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as response:
        return json.load(response)


def fixture_pids(executable: Path) -> set[int]:
    output = subprocess.check_output(["ps", "-axo", "pid=,comm="], text=True)
    pids = set()
    for line in output.splitlines():
        fields = line.strip().split(maxsplit=1)
        if (len(fields) == 2 and fields[1].endswith("/Contents/MacOS/Probe")
                and Path(fields[1]).resolve() == executable.resolve()):
            pids.add(int(fields[0]))
    return pids


def stop_fixtures(executable: Path) -> None:
    # Re-resolve executable ownership before each signal; never kill user apps.
    for sig in (signal.SIGTERM, signal.SIGKILL):
        for pid in fixture_pids(executable):
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                pass
        deadline = time.monotonic() + 5
        while fixture_pids(executable) and time.monotonic() < deadline:
            time.sleep(0.1)
        if not fixture_pids(executable):
            return
    raise RuntimeError("Owned fixture processes did not terminate")


@asynccontextmanager
async def session_for(label: str, base: str, binary: Path, port: int, data: Path):
    if label == "http":
        async with streamablehttp_client(base + "/mcp") as (read, write, _):
            async with ClientSession(read, write) as session:
                yield session
    else:
        params = StdioServerParameters(command=str(binary),
            args=["mcp", "--port", str(port), "--data-dir", str(data)])
        async with stdio_client(params) as (read, write):
            async with ClientSession(read, write) as session:
                yield session


async def exercise(session: ClientSession, label: str, base: str, bundle: Path,
                   identifier: str, report_dir: Path) -> dict:
    await session.initialize()
    tools = (await session.list_tools()).tools
    matches = [tool for tool in tools if tool.name == "app_open"]
    assert len(matches) == 1, "app_open must be advertised exactly once"
    schema = matches[0].inputSchema
    assert schema["properties"]["summary"]["maxLength"] == 500
    assert schema["properties"]["new_instance"]["default"] is True
    assert not matches[0].annotations.readOnlyHint
    sequence = 0
    workspace = None
    observations = []

    async def call(name: str, *, expected_error: str | None = None, **args):
        nonlocal sequence
        sequence += 1
        summary = ("Verified restored launcher discovery and a disposable test environment. "
                   "This call checks native launch/reuse and preserves the user's apps and profile data.")
        metadata = dict(summary=summary, agent="app-open-regression", model="test-client",
                        main_task="Verify restored app_open", current_task=f"{label}: {name}",
                        progress=50, quality=90,
                        current_timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat())
        if workspace:
            args["workspace"] = workspace
        if name == "app_open":
            args["capture_path"] = str(report_dir.relative_to(ROOT) / f"{label}-{sequence:02d}.png")
        result = await session.call_tool(name, {**metadata, **args})
        value = result.structuredContent["result"]
        if expected_error is not None:
            assert result.isError and expected_error in value.get("error", ""), value
            return value
        assert not result.isError, value
        if name == "app_open":
            assert value["pid"] > 0 and value["window_id"] > 0, value
            assert value["automatic_screenshot"] is True, value
            images = [block for block in result.content if block.type == "image"]
            assert len(images) == 1 and images[0].mimeType == "image/png"
            raw = base64.b64decode(images[0].data, validate=True)
            assert raw[:8] == b"\x89PNG\r\n\x1a\n"
            width, height = struct.unpack(">II", raw[16:24])
            assert width > 0 and height > 0
            assert (width, height) == (value["width"], value["height"])
            assert (width, height) == (value["screen_width"], value["screen_height"])
            assert value["focus_changed"] is False, value
            assert value["before"]["frontmost_pid"] == value["after"]["frontmost_pid"]
            assert value["before"]["front_window_id"] == value["after"]["front_window_id"]
            assert value["input_events_posted"] == 0, value
            # A human may keep using the mouse while this non-input test runs.
            # Assert cursor stability when the physical event counter is stable;
            # otherwise retain both observations rather than blame external motion.
            if value["before"].get("physical_mouse_move_count") == value["after"].get("physical_mouse_move_count"):
                for axis in ["cursor_x", "cursor_y"]:
                    assert value["before"][axis] == value["after"][axis], value
            observations.append({key: value.get(key) for key in [
                "pid", "window_id", "app", "reused", "delivery", "focus_changed",
                "automatic_screenshot", "width", "height", "path", "capture_quality", "before", "after"]})
        return value

    workspace = (await call("workspace_open", path=str(ROOT)))["id"]
    settings = request(base, "/api/state")["settings"]
    settings["computer_enabled"] = False
    request(base, "/api/action/settings", settings)
    await call("app_open", app=str(bundle), new_instance=False,
               summary="雪" * 500, expected_error="Enable computer control")
    settings["computer_enabled"] = True
    request(base, "/api/action/settings", settings)
    before = await call("list_windows")
    assert before["accessibility"] and before["screen_recording"], "Missing macOS permissions"

    await call("app_open", app=str(bundle), summary="雪" * 501,
               expected_error="summary must be at most 500")
    await call("app_open", app=str(bundle), new_instance="false",
               expected_error="new_instance must be a boolean")
    await call("app_open", app=str(bundle), action="click",
               expected_error="app_open does not accept action")
    assert not fixture_pids(bundle / "Contents/MacOS/Probe"), "Rejected requests launched an app"

    # Reuse mode also launches normally when the application is not running.
    first = await call("app_open", app=str(bundle), new_instance=False)
    assert first["reused"] is False
    # Freshly launched bundles must also be reusable before LS indexes them.
    assert first["app"] == identifier, ("unexpected bundle identity", first["app"], identifier)
    (report_dir / f"{label}-first-launch.json").write_text(json.dumps(observations, indent=2))
    diagnostic = """import AppKit
let name = ProcessInfo.processInfo.environment["LESSAGENT_TEST_BUNDLE_ID"]!
let running = NSRunningApplication.runningApplications(withBundleIdentifier: name)
let result: [String: Any] = ["identifier": name,
    "resolvedURL": NSWorkspace.shared.urlForApplication(withBundleIdentifier: name)?.path ?? "unavailable",
    "running": running.map { ["pid": $0.processIdentifier, "url": $0.bundleURL?.path ?? "unavailable"] as [String: Any] }]
let data = try! JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
print(String(data: data, encoding: .utf8)!)
"""
    query = subprocess.run(["swift", "-e", diagnostic], text=True, capture_output=True,
                           env={**os.environ, "LESSAGENT_TEST_BUNDLE_ID": identifier}, timeout=30)
    (report_dir / f"{label}-bundle-lookup.log").write_text(query.stdout + query.stderr)
    assert query.returncode == 0, query.stderr
    reused = await call("app_open", app=identifier, new_instance=False)
    assert reused["reused"] is True
    assert (reused["pid"], reused["window_id"]) == (first["pid"], first["window_id"])
    # Preserve the restored implementation's default: another instance, same profile.
    default_new = await call("app_open", app=str(bundle))
    explicit_new = await call("app_open", app=str(bundle), new_instance=True)
    assert default_new["reused"] is False and explicit_new["reused"] is False
    assert len({item["pid"] for item in [first, default_new, explicit_new]}) == 3
    assert len({item["window_id"] for item in [first, default_new, explicit_new]}) == 3
    owned = fixture_pids(bundle / "Contents/MacOS/Probe")
    await call("app_open", app=identifier, new_instance=False, expected_error="no unique window")
    assert fixture_pids(bundle / "Contents/MacOS/Probe") == owned
    print(f"{label}: app_open launch, reuse, default/explicit new instance, ambiguity, "
          "permissions, 500-character metadata and native screenshots PASS", flush=True)
    return {"status": "PASS", "observations": observations, "calls": sequence}


async def main(binary: Path, report_dir: Path) -> None:
    report_dir.mkdir(parents=True, exist_ok=True)
    report = {"binary": str(binary), "transports": {}, "status": "RUNNING"}
    with tempfile.TemporaryDirectory(prefix="lessagent-app-open-") as temporary:
        tmp = Path(temporary).resolve()
        source = tmp / "probe.swift"
        source.write_text(SOURCE)
        bundle = tmp / "LauncherProbe.app"
        executable = bundle / "Contents/MacOS/Probe"
        executable.parent.mkdir(parents=True)
        identifier = "io.lessagent.app-open-regression." + uuid.uuid4().hex
        with (bundle / "Contents/Info.plist").open("wb") as stream:
            plistlib.dump({"CFBundleIdentifier": identifier, "CFBundleName": "LauncherProbe",
                          "CFBundleExecutable": "Probe", "CFBundlePackageType": "APPL",
                          "CFBundleVersion": "1", "LSUIElement": True}, stream)
        subprocess.run(["swiftc", "-Onone", "-g", str(source), "-o", str(executable)],
                       check=True, timeout=120)
        with socket.socket() as sock:
            sock.bind(("127.0.0.1", 0))
            port = sock.getsockname()[1]
        assert port != 3210
        base = f"http://127.0.0.1:{port}"
        data = tmp / "data"
        report["port"] = port
        env = {k: v for k, v in os.environ.items()
               if k not in {"OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY"}}
        with (report_dir / "server.log").open("w") as log:
            server = subprocess.Popen([str(binary), "serve", "--port", str(port),
                                       "--data-dir", str(data)], stdout=log, stderr=log, env=env)
            try:
                for _ in range(200):
                    if server.poll() is not None:
                        raise RuntimeError("Disposable debug backend exited")
                    try:
                        state = request(base, "/api/state")
                        break
                    except OSError:
                        await asyncio.sleep(0.1)
                else:
                    raise RuntimeError("Disposable debug backend did not become ready")
                assert state.get("debug_build") is True, "Use a debug build, not release"
                for label in ["http", "stdio"]:
                    try:
                        async with session_for(label, base, binary, port, data) as session:
                            report["transports"][label] = await exercise(
                                session, label, base, bundle, identifier, report_dir)
                    finally:
                        stop_fixtures(executable)
                report["status"] = "PASS"
            except BaseException as error:
                report["status"] = "FAIL"
                report["error"] = repr(error)
                raise
            finally:
                try:
                    stop_fixtures(executable)
                finally:
                    if server.poll() is None:
                        server.send_signal(signal.SIGINT)
                        try:
                            server.wait(timeout=10)
                        except subprocess.TimeoutExpired:
                            server.kill()
                            server.wait(timeout=5)
                    (report_dir / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print("Evidence:", report_dir, flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/debug/lessagent", type=Path)
    parser.add_argument("--output", type=Path, default=Path("output/app-open-mcp-" +
                        datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")))
    args = parser.parse_args()
    if sys.platform != "darwin":
        print("SKIP: native app_open regression requires macOS")
    else:
        asyncio.run(main(args.binary.resolve(), (ROOT / args.output).resolve()))
