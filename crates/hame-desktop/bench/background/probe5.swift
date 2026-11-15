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
        cfg.showsCursor = false
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


func disk(_ p: String) -> String { (try? String(contentsOfFile: p, encoding: .utf8)) ?? "" }
func cellAt(_ table: AXUIElement, _ row: Int, _ col: Int) -> AXUIElement? {
    var cells: [AXUIElement] = []
    findAll(table, into: &cells) { role($0) == "AXCell" }
    return cells.first { e in
        guard let rr = attr(e, "AXRowIndexRange"), let cr = attr(e, "AXColumnIndexRange") else { return false }
        var r = CFRange(), c = CFRange()
        AXValueGetValue(rr as! AXValue, .cfRange, &r); AXValueGetValue(cr as! AXValue, .cfRange, &c)
        return r.location == row && c.location == col
    }
}
func quit(_ app: NSRunningApplication, _ b: String) {
    app.terminate(); sleep(1.5)
    if let s = runningApp(b) {
        for w in windows(AXUIElementCreateApplication(s.processIdentifier)) {
            var bt: [AXUIElement] = []
            findAll(w, into: &bt) { role($0) == "AXButton" && ["Delete", "Don’t Save", "Don't Save"].contains(str($0, kAXTitleAttribute) ?? "") }
            if let d = bt.first { _ = press(d) }
        }
        sleep(1.0); s.forceTerminate()
    }
}
func openIWork(_ b: String, _ name: String, _ path: String) -> (NSRunningApplication, AXUIElement, AXUIElement)? {
    runApp(["-g", "-a", name, path])
    guard waitFor(20, { runningApp(b) != nil }), let a = runningApp(b) else { return nil }
    let el = AXUIElementCreateApplication(a.processIdentifier)
    _ = waitFor(25) { docWindow(el, titleContains: "probe5") != nil }
    sleep(2.0)
    AXUIElementSetAttributeValue(el, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
    sleep(2.0)
    guard let w = docWindow(el, titleContains: "probe5") else { return nil }
    return (a, el, w)
}
@main
struct Probe5 {
    static func main() async {
        try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)
        // Pages
        let pPath = "\(outDir)/probe5.rtf"
        try! "{\\rtf1\\ansi First paragraph of the probe.\\par}".write(toFile: pPath, atomically: true, encoding: .utf8)
        if let (app, _, win) = openIWork("com.apple.iWork.Pages", "Pages", pPath) {
            let pid = app.processIdentifier
            step("Pages", "AX caret at end + postToPid typing (background)", pid: pid) {
                guard let body = find(win, { role($0) == "AXTextArea" }) else { return (false, "no AXTextArea") }
                let len = (str(body, kAXValueAttribute) ?? "").utf16.count
                var range = CFRange(location: max(0, len - 1), length: 0)
                let er = AXUIElementSetAttributeValue(body, kAXSelectedTextRangeAttribute as CFString, AXValueCreate(.cfRange, &range)!)
                postText(" pagesmark", pid: pid); sleep(1.2)
                let v = str(body, kAXValueAttribute) ?? ""
                return (v.contains("pagesmark"), "range err \(er.rawValue), value \(v.debugDescription)")
            }
            step("Pages", "AX set value on body (background)", pid: pid) {
                guard let body = find(win, { role($0) == "AXTextArea" }) else { return (false, "no AXTextArea") }
                let e = setValue(body, "Set by AX in Pages."); sleep(1.0)
                let v = str(body, kAXValueAttribute) ?? ""
                return (v.contains("Set by AX"), "err \(e.rawValue), value \(v.debugDescription)")
            }
            if let w = wid(win) { let (ok, d) = await capture(pid: pid, windowID: w, name: "pages5"); record("Pages", "capture after edits", ok: ok, d, targetPid: pid, before: front(), cur0: cursor()) }
            quit(app, "com.apple.iWork.Pages")
        }
        // Numbers
        let nPath = "\(outDir)/probe5.csv"
        try! "Item,Amount\nA,1\nB,2\n".write(toFile: nPath, atomically: true, encoding: .utf8)
        if let (app, _, win) = openIWork("com.apple.iWork.Numbers", "Numbers", nPath) {
            let pid = app.processIdentifier
            step("Numbers", "AX select cell B2 + postToPid typing + Return (background)", pid: pid) {
                guard let table = find(win, { role($0) == "AXTable" }), let cell = cellAt(table, 1, 1) else { return (false, "no table/cell") }
                let es = AXUIElementSetAttributeValue(table, kAXSelectedCellsAttribute as CFString, [cell] as CFArray)
                sleep(0.4)
                postText("777", pid: pid); postKey(kReturn, [], pid: pid); sleep(1.5)
                let v = cellAt(table, 1, 1).flatMap { str($0, kAXValueAttribute) } ?? "nil"
                return (v == "777", "select err \(es.rawValue), B2 now \(v.debugDescription)")
            }
            step("Numbers", "AX set value on cell B3 (background)", pid: pid) {
                guard let table = find(win, { role($0) == "AXTable" }), let cell = cellAt(table, 2, 1) else { return (false, "no table/cell") }
                let e = setValue(cell, "888"); sleep(1.0)
                let v = cellAt(table, 2, 1).flatMap { str($0, kAXValueAttribute) } ?? "nil"
                return (v == "888", "err \(e.rawValue), B3 now \(v.debugDescription)")
            }
            if let w = wid(win) { let (ok, d) = await capture(pid: pid, windowID: w, name: "numbers5"); record("Numbers", "capture after edits", ok: ok, d, targetPid: pid, before: front(), cur0: cursor()) }
            quit(app, "com.apple.iWork.Numbers")
        }
        // TextEdit: does activate() (which macOS may refuse to bring forward) let Save As and the panel work?
        let b = "com.apple.TextEdit"
        let file = "\(outDir)/probe5.txt"
        try! "start\n".write(toFile: file, atomically: true, encoding: .utf8)
        runApp(["-g", "-a", "TextEdit", file])
        if waitFor(10, { runningApp(b) != nil }), let app = runningApp(b) {
            let pid = app.processIdentifier, el = AXUIElementCreateApplication(pid)
            if waitFor(10, { docWindow(el, titleContains: "probe5") != nil }), let win = docWindow(el, titleContains: "probe5") {
                let dest = "\(outDir)/elsewhere"
                try? FileManager.default.createDirectory(atPath: dest, withIntermediateDirectories: true)
                step("TextEdit", "activate() then Save As… panel by AX + pid keys", pid: pid) {
                    app.activate(options: []); sleep(0.5)
                    let frontAfterActivate = front().1
                    _ = press(menuItem(el, ["File", "Save As…"])!)
                    guard waitFor(4, { find(win, { role($0) == "AXSheet" }) != nil }), let sheet = find(win, { role($0) == "AXSheet" }) else {
                        return (false, "front after activate \(frontAfterActivate); no sheet")
                    }
                    var notes = ["front after activate \(frontAfterActivate)"]
                    postKey(kG, [.maskCommand, .maskShift], pid: pid)
                    let gotGo = waitFor(3) { find(sheet, { str($0, kAXIdentifierAttribute) == "PathTextField" }) != nil }
                    notes.append("goto by pid key: \(gotGo)")
                    if gotGo, let p = find(sheet, { str($0, kAXIdentifierAttribute) == "PathTextField" }) {
                        _ = setValue(p, dest); sleep(0.4); postKey(kReturn, [], pid: pid); sleep(1.0)
                    }
                    if let n = find(sheet, { str($0, kAXIdentifierAttribute) == "saveAsNameTextField" }) { _ = setValue(n, "bg-saved.txt") }
                    var bt: [AXUIElement] = []
                    findAll(sheet, into: &bt) { role($0) == "AXButton" && (str($0, kAXTitleAttribute) ?? "") == "Save" }
                    if let s = bt.first { _ = press(s) }
                    sleep(1.5)
                    let ok = FileManager.default.fileExists(atPath: "\(dest)/bg-saved.txt")
                    if !ok, let s2 = find(win, { role($0) == "AXSheet" }) {
                        var c: [AXUIElement] = []
                        findAll(s2, into: &c) { role($0) == "AXButton" && (str($0, kAXTitleAttribute) ?? "") == "Cancel" }
                        if let x = c.first { _ = press(x) }
                    }
                    notes.append("dest \((try? FileManager.default.contentsOfDirectory(atPath: dest)) ?? [])")
                    return (ok, notes.joined(separator: ", "))
                }
            }
            app.terminate(); sleep(1.0)
            if let s = runningApp(b) { s.forceTerminate() }
        }
        let data = try! JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted])
        try! data.write(to: URL(fileURLWithPath: "\(outDir)/results5.json"))
    }
}
