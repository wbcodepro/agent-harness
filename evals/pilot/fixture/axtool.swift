// A small accessibility tool for the pilot's setup, fault injection,
// checkers and teardown. It is independent of hame's own desktop helper, so
// a checker never trusts the tool it is grading.
//
//   axtool windows <bundle>              one JSON object per window
//   axtool press-sheet <bundle> <button> press a button in the first sheet that has it
//   axtool close <bundle> <window-id>    close a window, discarding changes
//   axtool raise <bundle> <window-id>    bring a window and its app to the front
//   axtool press <bundle> <window-id> <button>  press a button in a window
//   axtool menu <bundle> <menu> <item>   choose a menu bar item, e.g. File New
//   axtool cells <bundle> <title-part>   every table cell (iWork: canvas exposed first)
//   axtool value <bundle> <role> <title> the value of the first matching element
//   axtool frame <bundle> <title-part> <name-part>  frames and texts of an element (a chart)
//   axtool frontmost                     bundle id of the frontmost app
//   axtool quit <bundle>                 ask the app to quit; force it after 5 s
//
// Built by pilot.py with `swiftc -O` and cached by source hash.

import AppKit
import ApplicationServices
import Foundation

@_silgen_name("_AXUIElementGetWindow")
func _AXUIElementGetWindow(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

func attr(_ e: AXUIElement, _ name: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(e, name as CFString, &value) == .success ? value : nil
}

func str(_ e: AXUIElement, _ name: String) -> String? {
    if let v = attr(e, name) {
        // AXDocument is a file URL string.
        if let s = v as? String, s.hasPrefix("file://"), let url = URL(string: s) { return url.path }
        if let s = v as? String { return s }
        if let u = v as? URL { return u.path }
    }
    return nil
}

func children(_ e: AXUIElement) -> [AXUIElement] {
    (attr(e, kAXChildrenAttribute) as? [AXUIElement]) ?? []
}

func app(_ bundle: String) -> NSRunningApplication? {
    NSRunningApplication.runningApplications(withBundleIdentifier: bundle).first
}

func windows(_ bundle: String) -> [(CGWindowID, AXUIElement)] {
    guard let running = app(bundle) else { return [] }
    let root = AXUIElementCreateApplication(running.processIdentifier)
    let list = (attr(root, kAXWindowsAttribute) as? [AXUIElement]) ?? []
    return list.compactMap { w in
        var id: CGWindowID = 0
        return _AXUIElementGetWindow(w, &id) == .success ? (id, w) : nil
    }
}

func find(_ e: AXUIElement, depth: Int = 0, _ match: (AXUIElement) -> Bool) -> AXUIElement? {
    if match(e) { return e }
    if depth > 12 { return nil }
    for c in children(e) {
        if let found = find(c, depth: depth + 1, match) { return found }
    }
    return nil
}

func sheets(_ w: AXUIElement) -> [AXUIElement] {
    children(w).filter { str($0, kAXRoleAttribute) == "AXSheet" }
}

func buttons(_ e: AXUIElement, depth: Int = 0) -> [String] {
    var out: [String] = []
    if str(e, kAXRoleAttribute) == "AXButton", let t = str(e, kAXTitleAttribute), !t.isEmpty {
        out.append(t)
    }
    if depth < 10 {
        for c in children(e) { out += buttons(c, depth: depth + 1) }
    }
    return out
}

func textValue(_ w: AXUIElement) -> String? {
    find(w) { str($0, kAXRoleAttribute) == "AXTextArea" }.flatMap { str($0, kAXValueAttribute) }
}

func json(_ object: Any) -> String {
    let data = try! JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    return String(data: data, encoding: .utf8)!
}

func pressButton(in e: AXUIElement, named name: String) -> Bool {
    guard let b = find(e, { str($0, kAXRoleAttribute) == "AXButton" && str($0, kAXTitleAttribute) == name })
    else { return false }
    return AXUIElementPerformAction(b, kAXPressAction as CFString) == .success
}

let args = CommandLine.arguments
guard args.count >= 2 else { print("usage: axtool <command> ..."); exit(2) }
switch args[1] {
case "windows":
    let bundle = args[2]
    var focusedId: CGWindowID = 0
    if let running = app(bundle) {
        let root = AXUIElementCreateApplication(running.processIdentifier)
        if let f = attr(root, kAXFocusedWindowAttribute) {
            _ = _AXUIElementGetWindow(f as! AXUIElement, &focusedId)
        }
    }
    for (id, w) in windows(bundle) {
        let s = sheets(w)
        print(json([
            "id": Int(id),
            "title": str(w, kAXTitleAttribute) ?? "",
            "document": str(w, kAXDocumentAttribute) ?? "",
            "sheet": !s.isEmpty,
            "sheet_buttons": s.flatMap { buttons($0) },
            "focused": id == focusedId,
            "text": textValue(w) ?? NSNull(),
        ] as [String: Any]))
    }
case "press-sheet":
    let (bundle, name) = (args[2], args[3])
    for (_, w) in windows(bundle) {
        for s in sheets(w) where pressButton(in: s, named: name) { exit(0) }
    }
    exit(1)
case "close":
    let bundle = args[2]
    let target = CGWindowID(args[3]) ?? 0
    for _ in 0..<8 {
        guard let (_, w) = windows(bundle).first(where: { $0.0 == target }) else { exit(0) }
        if let s = sheets(w).first {
            for name in ["Delete", "Delete Copy", "Don’t Save", "Don't Save", "Revert Changes", "Cancel"]
            where pressButton(in: s, named: name) { break }
        } else if let close = attr(w, kAXCloseButtonAttribute),
                  AXUIElementPerformAction(close as! AXUIElement, kAXPressAction as CFString) == .success {
        } else {
            // Panels such as Open have no close button, only Cancel.
            _ = pressButton(in: w, named: "Cancel")
        }
        usleep(400_000)
    }
    exit(windows(bundle).contains(where: { $0.0 == target }) ? 1 : 0)
case "raise":
    let bundle = args[2]
    let target = CGWindowID(args[3]) ?? 0
    guard let (_, w) = windows(bundle).first(where: { $0.0 == target }), let running = app(bundle)
    else { exit(1) }
    _ = AXUIElementPerformAction(w, kAXRaiseAction as CFString)
    _ = AXUIElementSetAttributeValue(w, kAXMainAttribute as CFString, kCFBooleanTrue)
    running.activate(options: [])
    exit(0)
case "press":
    let (bundle, target, name) = (args[2], CGWindowID(args[3]) ?? 0, args[4])
    guard let (_, w) = windows(bundle).first(where: { $0.0 == target }) else { exit(1) }
    exit(pressButton(in: w, named: name) ? 0 : 1)
case "menu":
    guard let running = app(args[2]) else { exit(1) }
    let root = AXUIElementCreateApplication(running.processIdentifier)
    guard let bar = attr(root, kAXMenuBarAttribute) else { exit(1) }
    let barItem = children(bar as! AXUIElement).first { str($0, kAXTitleAttribute) == args[3] }
    guard let menu = barItem.flatMap({ children($0).first }) else { exit(1) }
    guard let item = children(menu).first(where: { str($0, kAXTitleAttribute) == args[4] }) else { exit(1) }
    exit(AXUIElementPerformAction(item, kAXPressAction as CFString) == .success ? 0 : 1)
case "cells":
    // Every table cell of the windows whose title contains args[3], as
    // the app shows it. iWork apps expose their canvas only on request.
    guard let running = app(args[2]) else { exit(1) }
    let root = AXUIElementCreateApplication(running.processIdentifier)
    AXUIElementSetAttributeValue(root, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
    usleep(500_000)
    func range(_ e: AXUIElement, _ name: String) -> Int? {
        guard let v = attr(e, name), CFGetTypeID(v) == AXValueGetTypeID() else { return nil }
        var r = CFRange()
        AXValueGetValue(v as! AXValue, .cfRange, &r)
        return r.location
    }
    func walk(_ e: AXUIElement, _ table: String, _ depth: Int) {
        let role = str(e, kAXRoleAttribute) ?? ""
        var current = table
        if role == "AXTable" { current = str(e, kAXTitleAttribute) ?? "" }
        if role == "AXCell", let row = range(e, "AXRowIndexRange"), let col = range(e, "AXColumnIndexRange") {
            var letters = ""
            var n = col
            repeat { letters = String(UnicodeScalar(UInt8(65 + n % 26))) + letters; n = n / 26 - 1 } while n >= 0
            print(json(["table": current, "cell": "\(letters)\(row + 1)",
                        "title": str(e, kAXTitleAttribute) ?? "", "value": str(e, kAXValueAttribute) ?? ""]))
            return
        }
        if depth > 14 { return }
        for c in children(e) { walk(c, current, depth + 1) }
    }
    for (_, w) in windows(args[2]) where (str(w, kAXTitleAttribute) ?? "").contains(args[3]) {
        walk(w, "", 0)
    }
case "frame":
    // The window (title containing args[3]) and its first element whose
    // title or description contains args[4]: frames in screen points, the
    // window id, and every text inside the element.
    guard let running = app(args[2]) else { exit(1) }
    let root = AXUIElementCreateApplication(running.processIdentifier)
    AXUIElementSetAttributeValue(root, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
    usleep(300_000)
    func rect(_ e: AXUIElement) -> [Double] {
        var p = CGPoint.zero, s = CGSize.zero
        if let v = attr(e, kAXPositionAttribute) { AXValueGetValue(v as! AXValue, .cgPoint, &p) }
        if let v = attr(e, kAXSizeAttribute) { AXValueGetValue(v as! AXValue, .cgSize, &s) }
        return [Double(p.x), Double(p.y), Double(s.width), Double(s.height)]
    }
    func texts(_ e: AXUIElement, _ depth: Int) -> [String] {
        var out: [String] = []
        let role = str(e, kAXRoleAttribute) ?? ""
        if role == "AXStaticText" || role == "AXTextArea", let v = str(e, kAXValueAttribute), !v.isEmpty { out.append(v) }
        if depth < 8 { for c in children(e) { out += texts(c, depth + 1) } }
        return out
    }
    for (id, w) in windows(args[2]) where (str(w, kAXTitleAttribute) ?? "").contains(args[3]) {
        if let e = find(w, { (str($0, kAXDescriptionAttribute) ?? "").contains(args[4])
                             || (str($0, kAXTitleAttribute) ?? "").contains(args[4]) }) {
            print(json(["window_id": Int(id), "window": rect(w), "element": rect(e),
                        "name": str(e, kAXDescriptionAttribute) ?? str(e, kAXTitleAttribute) ?? "",
                        "texts": texts(e, 0)]))
            exit(0)
        }
    }
    exit(1)
case "value":
    // The value of the first element with role args[3] and title args[4]
    // in any window of the app.
    for (_, w) in windows(args[2]) {
        if let e = find(w, { str($0, kAXRoleAttribute) == args[3] && str($0, kAXTitleAttribute) == args[4] }) {
            if let n = attr(e, kAXValueAttribute) as? NSNumber { print(n.stringValue) }
            else { print(str(e, kAXValueAttribute) ?? "") }
            exit(0)
        }
    }
    exit(1)
case "frontmost":
    print(NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "")
case "quit":
    guard let running = app(args[2]) else { exit(0) }
    running.terminate()
    for _ in 0..<50 {
        if running.isTerminated { exit(0) }
        usleep(100_000)
    }
    running.forceTerminate()
    for _ in 0..<30 where !running.isTerminated { usleep(100_000) }
    exit(0)
default:
    print("unknown command \(args[1])")
    exit(2)
}
