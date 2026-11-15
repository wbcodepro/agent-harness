// Background-execution probe: can an agent operate TextEdit, Numbers and
// Pages without becoming frontmost or moving the pointer?
// Every step records the frontmost app and the cursor before and after.
import Cocoa
import ApplicationServices
import ScreenCaptureKit

@_silgen_name("_AXUIElementGetWindow")
func _AXUIElementGetWindow(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

let outDir = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/bgprobe"
var results: [[String: Any]] = []

func front() -> (Int32, String) {
    let a = NSWorkspace.shared.frontmostApplication
    return (a?.processIdentifier ?? -1, a?.bundleIdentifier ?? "?")
}
func cursor() -> CGPoint { CGEvent(source: nil)?.location ?? .zero }
func sleep(_ s: Double) { Thread.sleep(forTimeInterval: s) }

func record(_ app: String, _ name: String, ok: Bool, _ detail: String, targetPid: Int32,
            before: (Int32, String), cur0: CGPoint) {
    let after = front()
    let cur1 = cursor()
    let tookFront = after.0 == targetPid
    let moved = hypot(cur1.x - cur0.x, cur1.y - cur0.y) > 0.5
    results.append(["app": app, "test": name, "ok": ok, "detail": detail,
                    "front_before": before.1, "front_after": after.1,
                    "target_became_frontmost": tookFront, "cursor_moved": moved])
    let flag = ok ? "PASS" : "FAIL"
    print("[\(flag)] \(app) \(name): \(detail) | front \(before.1) -> \(after.1)\(tookFront ? " (TARGET TOOK FRONT)" : "")\(moved ? " (cursor moved)" : "")")
}

func step(_ app: String, _ name: String, pid: Int32, _ body: () -> (Bool, String)) {
    let f0 = front(), c0 = cursor()
    let (ok, detail) = body()
    record(app, name, ok: ok, detail, targetPid: pid, before: f0, cur0: c0)
}

// --- AX helpers ---
func attr(_ e: AXUIElement, _ n: String) -> CFTypeRef? {
    var v: CFTypeRef?
    return AXUIElementCopyAttributeValue(e, n as CFString, &v) == .success ? v : nil
}
func str(_ e: AXUIElement, _ n: String) -> String? { attr(e, n) as? String }
func children(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? [] }
func find(_ e: AXUIElement, depth: Int = 0, _ pred: (AXUIElement) -> Bool) -> AXUIElement? {
    if pred(e) { return e }
    if depth > 30 { return nil }
    for c in children(e) { if let f = find(c, depth: depth + 1, pred) { return f } }
    return nil
}
func findAll(_ e: AXUIElement, depth: Int = 0, into out: inout [AXUIElement], _ pred: (AXUIElement) -> Bool) {
    if pred(e) { out.append(e) }
    if depth > 30 { return }
    for c in children(e) { findAll(c, depth: depth + 1, into: &out, pred) }
}
func role(_ e: AXUIElement) -> String { str(e, kAXRoleAttribute) ?? "" }
func windows(_ app: AXUIElement) -> [AXUIElement] { (attr(app, kAXWindowsAttribute) as? [AXUIElement]) ?? [] }
func wid(_ w: AXUIElement) -> CGWindowID? {
    var id: CGWindowID = 0
    return _AXUIElementGetWindow(w, &id) == .success && id != 0 ? id : nil
}
func press(_ e: AXUIElement) -> AXError { AXUIElementPerformAction(e, kAXPressAction as CFString) }
func setValue(_ e: AXUIElement, _ v: String) -> AXError {
    AXUIElementSetAttributeValue(e, kAXValueAttribute as CFString, v as CFString)
}
func menuItem(_ app: AXUIElement, _ path: [String]) -> AXUIElement? {
    guard let bar = attr(app, kAXMenuBarAttribute) else { return nil }
    var cur = bar as! AXUIElement
    for name in path {
        let kids = children(cur)
        // A menu bar item holds one AXMenu whose children are the items.
        var candidates: [AXUIElement] = []
        for k in kids {
            if role(k) == "AXMenu" { candidates += children(k) } else { candidates.append(k) }
        }
        guard let next = candidates.first(where: { (str($0, kAXTitleAttribute) ?? "") == name }) else { return nil }
        cur = next
    }
    return cur
}

// --- input to a pid ---
func postText(_ text: String, pid: pid_t) {
    let src = CGEventSource(stateID: .privateState)
    for ch in text {
        let units = Array(String(ch).utf16)
        for down in [true, false] {
            guard let ev = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: down) else { continue }
            units.withUnsafeBufferPointer { ev.keyboardSetUnicodeString(stringLength: $0.count, unicodeString: $0.baseAddress) }
            ev.postToPid(pid)
        }
        usleep(8000)
    }
}
func postKey(_ code: CGKeyCode, _ flags: CGEventFlags, pid: pid_t) {
    let src = CGEventSource(stateID: .privateState)
    for down in [true, false] {
        guard let ev = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down) else { continue }
        ev.flags = flags
        ev.postToPid(pid)
    }
    usleep(20000)
}
let kS: CGKeyCode = 1, kG: CGKeyCode = 5, kReturn: CGKeyCode = 36

// --- capture ---
func capture(pid: pid_t, windowID: CGWindowID, name: String) async -> (Bool, String) {
    do {
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let w = content.windows.first(where: { $0.windowID == windowID }) else {
            return (false, "window \(windowID) not in shareable content")
        }
        let filter = SCContentFilter(desktopIndependentWindow: w)
        let cfg = SCStreamConfiguration()
        cfg.width = Int(w.frame.width) * 2; cfg.height = Int(w.frame.height) * 2
        let img = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: cfg)
        let rep = NSBitmapImageRep(cgImage: img)
        let path = "\(outDir)/\(name).png"
        try rep.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        // Spread of a sample of pixels: a blank capture is one colour.
        var lo = 255.0, hi = 0.0
        for y in stride(from: 0, to: rep.pixelsHigh, by: max(1, rep.pixelsHigh / 40)) {
            for x in stride(from: 0, to: rep.pixelsWide, by: max(1, rep.pixelsWide / 40)) {
                if let c = rep.colorAt(x: x, y: y) {
                    let l = Double(c.redComponent + c.greenComponent + c.blueComponent) * 85
                    lo = min(lo, l); hi = max(hi, l)
                }
            }
        }
        return (hi - lo > 20, "\(img.width)x\(img.height), spread \(Int(hi - lo)), onScreen=\(w.isOnScreen), saved \(name).png")
    } catch {
        return (false, "capture error: \(error)")
    }
}

func runApp(_ args: [String]) {
    let p = Process(); p.executableURL = URL(fileURLWithPath: "/usr/bin/open"); p.arguments = args
    try? p.run(); p.waitUntilExit()
}
func runningApp(_ bundle: String) -> NSRunningApplication? {
    NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first
}
func waitFor(_ seconds: Double, _ cond: () -> Bool) -> Bool {
    let end = Date().addingTimeInterval(seconds)
    while Date() < end { if cond() { return true }; sleep(0.2) }
    return cond()
}
func docWindow(_ appEl: AXUIElement, titleContains: String) -> AXUIElement? {
    windows(appEl).first { (str($0, kAXTitleAttribute) ?? "").contains(titleContains) }
}

@main
struct Probe {
    static func main() async {
        try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)
        print("AX trusted:", AXIsProcessTrusted(), " frontmost at start:", front().1)
        await textEdit()
        await iwork(bundle: "com.apple.iWork.Numbers", app: "Numbers", file: "probe.csv",
                    contents: "Item,Amount\nA,1\nB,2\n", marker: "probe42")
        await iwork(bundle: "com.apple.iWork.Pages", app: "Pages", file: "probe.rtf",
                    contents: "{\\rtf1\\ansi First paragraph of the probe.\\par}", marker: "probe-pages-77")
        let data = try! JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted])
        try! data.write(to: URL(fileURLWithPath: "\(outDir)/results.json"))
        print("wrote \(outDir)/results.json")
    }

    static func textEdit() async {
        let bundle = "com.apple.TextEdit", A = "TextEdit"
        let dir = "\(outDir)/te"
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let file = "\(dir)/bg-probe.txt"
        try! "line one\n".write(toFile: file, atomically: true, encoding: .utf8)
        let wasRunning = runningApp(bundle) != nil
        let f0 = front(), c0 = cursor()
        runApp(["-g", "-j", "-a", "TextEdit", file])  // background and hidden
        guard waitFor(10, { runningApp(bundle) != nil }), let app = runningApp(bundle) else {
            print("TextEdit did not start"); return
        }
        let pid = app.processIdentifier
        let appEl = AXUIElementCreateApplication(pid)
        let gotWin = waitFor(10) { docWindow(appEl, titleContains: "bg-probe") != nil }
        record(A, "launch hidden (open -g -j)", ok: gotWin, "document window visible to AX: \(gotWin), hidden=\(app.isHidden)", targetPid: pid, before: f0, cur0: c0)
        guard let win = docWindow(appEl, titleContains: "bg-probe"),
              let area = find(win, { role($0) == "AXTextArea" }) else { print("no text area"); return }
        let w = wid(win)

        step(A, "AX read text", pid: pid) {
            let v = str(area, kAXValueAttribute) ?? "nil"
            return (v == "line one\n", "value \(v.debugDescription)")
        }
        step(A, "AX set value (hidden app)", pid: pid) {
            let e = setValue(area, "line one\nax set\n")
            let v = str(area, kAXValueAttribute) ?? ""
            return (e == .success && v == "line one\nax set\n", "err \(e.rawValue), reads \(v.debugDescription)")
        }
        step(A, "AX menu File > Save (hidden app)", pid: pid) {
            guard let item = menuItem(appEl, ["File", "Save"]) else { return (false, "no File > Save item") }
            let e = press(item)
            sleep(1.0)
            let disk = (try? String(contentsOfFile: file, encoding: .utf8)) ?? ""
            return (e == .success && disk == "line one\nax set\n", "err \(e.rawValue), disk \(disk.debugDescription)")
        }
        if let w { let (ok, d) = await capture(pid: pid, windowID: w, name: "textedit-hidden")
            record(A, "capture window while app hidden", ok: ok, d, targetPid: pid, before: front(), cur0: cursor()) }

        // Unhide without activating: the window is now visible behind the user's app.
        app.unhide(); sleep(0.5)
        record(A, "unhide (no activate)", ok: !app.isHidden, "hidden=\(app.isHidden)", targetPid: pid, before: f0, cur0: c0)

        step(A, "postToPid typed text into background window", pid: pid) {
            let len = (str(area, kAXValueAttribute) ?? "").utf16.count
            var range = CFRange(location: len, length: 0)
            let r = AXValueCreate(.cfRange, &range)!
            let e = AXUIElementSetAttributeValue(area, kAXSelectedTextRangeAttribute as CFString, r)
            postText("typed Tiếng Việt ✓", pid: pid)
            sleep(0.8)
            let v = str(area, kAXValueAttribute) ?? ""
            return (v.hasSuffix("typed Tiếng Việt ✓"), "select err \(e.rawValue), reads \(v.debugDescription)")
        }
        step(A, "postToPid cmd+s into background window", pid: pid) {
            postKey(kS, .maskCommand, pid: pid)
            sleep(1.0)
            let disk = (try? String(contentsOfFile: file, encoding: .utf8)) ?? ""
            return (disk.hasSuffix("typed Tiếng Việt ✓"), "disk \(disk.debugDescription)")
        }
        if let w { let (ok, d) = await capture(pid: pid, windowID: w, name: "textedit-background")
            record(A, "capture background (possibly occluded) window", ok: ok, d, targetPid: pid, before: front(), cur0: cursor()) }

        // Save As into another folder entirely in the background.
        let dest = "\(dir)/saved-elsewhere"
        try? FileManager.default.createDirectory(atPath: dest, withIntermediateDirectories: true)
        step(A, "save panel in background: Save As…, Go To Folder by key, name and Save by AX", pid: pid) {
            guard let item = menuItem(appEl, ["File", "Save As…"]) ?? menuItem(appEl, ["File", "Duplicate"]) else {
                return (false, "no Save As… item")
            }
            _ = press(item)
            guard waitFor(5, { find(win, { role($0) == "AXSheet" }) != nil }),
                  let sheet = find(win, { role($0) == "AXSheet" }) else { return (false, "no save sheet") }
            postKey(kG, [.maskCommand, .maskShift], pid: pid)
            let gotGo = waitFor(3) { find(sheet, { str($0, kAXIdentifierAttribute) == "PathTextField" }) != nil }
            var notes = ["goto field: \(gotGo)"]
            if gotGo, let path = find(sheet, { str($0, kAXIdentifierAttribute) == "PathTextField" }) {
                notes.append("set path err \(setValue(path, dest).rawValue)")
                sleep(0.3)
                postKey(kReturn, [], pid: pid)
                sleep(0.8)
            }
            if let name = find(sheet, { str($0, kAXIdentifierAttribute) == "saveAsNameTextField" }) {
                notes.append("set name err \(setValue(name, "bg-saved.txt").rawValue)")
            }
            var buttons: [AXUIElement] = []
            findAll(sheet, into: &buttons) { role($0) == "AXButton" && (str($0, kAXTitleAttribute) ?? "") == "Save" }
            if let save = buttons.first { notes.append("press Save err \(press(save).rawValue)") }
            sleep(1.2)
            let written = FileManager.default.fileExists(atPath: "\(dest)/bg-saved.txt")
            if !written, let s = find(win, { role($0) == "AXSheet" }) {
                var cancels: [AXUIElement] = []
                findAll(s, into: &cancels) { role($0) == "AXButton" && (str($0, kAXTitleAttribute) ?? "") == "Cancel" }
                if let c = cancels.first { _ = press(c) }
                notes.append("cancelled")
            }
            return (written, notes.joined(separator: ", ") + ", file written: \(written)")
        }

        step(A, "minimize by AX", pid: pid) {
            let e = AXUIElementSetAttributeValue(win, kAXMinimizedAttribute as CFString, kCFBooleanTrue)
            sleep(1.0)
            return (e == .success, "err \(e.rawValue)")
        }
        if let w { let (ok, d) = await capture(pid: pid, windowID: w, name: "textedit-minimized")
            record(A, "capture minimized window", ok: ok, d, targetPid: pid, before: front(), cur0: cursor()) }
        step(A, "AX set value while minimized", pid: pid) {
            let e = setValue(area, "minimized edit\n")
            let v = str(area, kAXValueAttribute) ?? ""
            return (e == .success && v == "minimized edit\n", "err \(e.rawValue), reads \(v.debugDescription)")
        }
        step(A, "postToPid text while minimized", pid: pid) {
            postText("MIN", pid: pid); sleep(0.6)
            let v = str(area, kAXValueAttribute) ?? ""
            return (v.contains("MIN"), "reads \(v.debugDescription)")
        }
        // Clean up: close without saving, quit if we launched it.
        _ = setValue(area, "")
        AXUIElementSetAttributeValue(win, kAXMinimizedAttribute as CFString, kCFBooleanFalse)
        if !wasRunning { app.terminate() } else if let close = menuItem(appEl, ["File", "Close"]) { _ = press(close) }
        sleep(1.0)
        if !wasRunning, let still = runningApp(bundle) { still.forceTerminate() }
    }

    static func iwork(bundle: String, app A: String, file: String, contents: String, marker: String) async {
        let dir = "\(outDir)/\(A.lowercased())"
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        let path = "\(dir)/\(file)"
        try! contents.write(toFile: path, atomically: true, encoding: .utf8)
        let wasRunning = runningApp(bundle) != nil
        let f0 = front(), c0 = cursor()
        runApp(["-g", "-a", A, path])  // background, not hidden
        guard waitFor(20, { runningApp(bundle) != nil }), let app = runningApp(bundle) else {
            record(A, "launch in background", ok: false, "did not start", targetPid: -2, before: f0, cur0: c0); return
        }
        let pid = app.processIdentifier
        let appEl = AXUIElementCreateApplication(pid)
        AXUIElementSetAttributeValue(appEl, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
        let stem = (file as NSString).deletingPathExtension
        let got = waitFor(25) { docWindow(appEl, titleContains: stem) != nil }
        record(A, "launch in background (open -g)", ok: got, "document window visible to AX: \(got)", targetPid: pid, before: f0, cur0: c0)
        guard let win = docWindow(appEl, titleContains: stem) else { return }
        sleep(2.0)
        step(A, "postToPid typed text into background document", pid: pid) {
            postText(marker, pid: pid)
            postKey(kReturn, [], pid: pid)
            sleep(1.5)
            var hits: [AXUIElement] = []
            findAll(win, into: &hits) { e in
                ((str(e, kAXValueAttribute) ?? "") + (str(e, kAXTitleAttribute) ?? "") + (str(e, kAXDescriptionAttribute) ?? "")).contains(marker)
            }
            return (!hits.isEmpty, "marker found by AX in \(hits.count) element(s): \(hits.prefix(2).map(role))")
        }
        if let w = wid(win) {
            let (ok, d) = await capture(pid: pid, windowID: w, name: "\(A.lowercased())-background")
            record(A, "capture background window", ok: ok, d, targetPid: pid, before: front(), cur0: cursor())
        }
        // Close without saving.
        if !wasRunning { app.terminate(); sleep(1.5) }
        if !wasRunning, let still = runningApp(bundle) {
            // A "save changes?" sheet blocks terminate: discard through AX.
            let el = AXUIElementCreateApplication(still.processIdentifier)
            for w in windows(el) {
                var b: [AXUIElement] = []
                findAll(w, into: &b) { role($0) == "AXButton" && ["Delete", "Don’t Save", "Don't Save"].contains(str($0, kAXTitleAttribute) ?? "") }
                if let d = b.first { _ = press(d) }
            }
            sleep(1.0)
            still.forceTerminate()
        }
    }
}
