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
@main
struct Probe4 {
    static func main() async {
        try? FileManager.default.createDirectory(atPath: outDir, withIntermediateDirectories: true)
        let bundle = "com.apple.TextEdit", A = "TextEdit"
        let file = "\(outDir)/probe4.txt"
        try! "start\n".write(toFile: file, atomically: true, encoding: .utf8)
        runApp(["-g", "-a", "TextEdit", file])
        guard waitFor(10, { runningApp(bundle) != nil }), let app = runningApp(bundle) else { return }
        let pid = app.processIdentifier, appEl = AXUIElementCreateApplication(pid)
        guard waitFor(10, { docWindow(appEl, titleContains: "probe4") != nil }),
              let win = docWindow(appEl, titleContains: "probe4"),
              let area = find(win, { role($0) == "AXTextArea" }) else { return }
        sleep(1.0)
        step(A, "typing by pid makes the document edited", pid: pid) {
            var range = CFRange(location: 6, length: 0)
            AXUIElementSetAttributeValue(area, kAXSelectedTextRangeAttribute as CFString, AXValueCreate(.cfRange, &range)!)
            postText("bg typed", pid: pid); sleep(0.8)
            let v = str(area, kAXValueAttribute) ?? ""
            let title = str(win, kAXTitleAttribute) ?? ""
            let edited = attr(win, "AXDocumentModified") ?? attr(win, "AXEdited")
            return (v.contains("g typed"), "value \(v.debugDescription), title \(title.debugDescription), modified attr \(String(describing: edited))")
        }
        for (label, wait) in [("AX File > Save, wait 4 s", 4.0)] {
            step(A, label, pid: pid) {
                let e = press(menuItem(appEl, ["File", "Save"])!); sleep(wait)
                let title = str(win, kAXTitleAttribute) ?? ""
                return (disk(file).contains("g typed"), "press \(e.rawValue), title after \(title.debugDescription), disk \(disk(file).debugDescription)")
            }
        }
        // Control: the same Save with the app frontmost (this one activates TextEdit on purpose).
        let f0 = front(), c0 = cursor()
        app.activate(options: []); sleep(0.8)
        let e = press(menuItem(appEl, ["File", "Save"])!); sleep(2.0)
        record(A, "CONTROL: AX File > Save with TextEdit activated", ok: disk(file).contains("g typed"), "press \(e.rawValue), disk \(disk(file).debugDescription)", targetPid: -9, before: f0, cur0: c0)
        if let w = wid(win) { let (ok, d) = await capture(pid: pid, windowID: w, name: "probe4"); print(ok, d) }
        app.terminate(); sleep(1.0)
        if let s = runningApp(bundle) { s.forceTerminate() }
        let data = try! JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted])
        try! data.write(to: URL(fileURLWithPath: "\(outDir)/results4.json"))
    }
}
