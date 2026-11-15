// hame desktop helper: a long-lived process that speaks line-delimited JSON
// on stdin/stdout and drives macOS applications through the Accessibility
// API, CoreGraphics events and ScreenCaptureKit.
//
// The Rust side (crates/hame-desktop) owns policy: authorization, receipts,
// the desktop lease and batching. This helper owns only mechanism, and it
// re-checks every precondition against the live state immediately before
// it acts (preflight), because the Rust side's view can be stale.
//
// Requests:  {"id": N, "op": "...", ...}
// Responses: {"id": N, "ok": true, "result": {...}}
//            {"id": N, "ok": false, "error": {"code": "...", "message": "...", "details": {...}}}
// A `cancel` request ({"op": "cancel", "target": N}) is handled on the
// reader thread, so it reaches an operation that is still running.

import AppKit
import ApplicationServices
import CoreGraphics
import Foundation
import ImageIO
import ScreenCaptureKit
import UniformTypeIdentifiers

let HELPER_PROTOCOL = 1
let HELPER_VERSION = "0.1.0"

@_silgen_name("_AXUIElementGetWindow")
func _AXUIElementGetWindow(_ element: AXUIElement, _ id: UnsafeMutablePointer<CGWindowID>) -> AXError

// MARK: - Output

let outLock = NSLock()

func emit(_ object: [String: Any]) {
    guard let data = try? JSONSerialization.data(withJSONObject: object, options: []) else {
        emit(["id": object["id"] ?? -1, "ok": false,
              "error": ["code": "internal", "message": "unencodable response"]])
        return
    }
    outLock.lock()
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data([0x0a]))
    outLock.unlock()
}

struct HelperError: Error {
    let code: String
    let message: String
    var details: [String: Any] = [:]
}

func fail(_ code: String, _ message: String, _ details: [String: Any] = [:]) -> HelperError {
    HelperError(code: code, message: message, details: details)
}

// MARK: - State

let stateLock = NSLock()
var cancelled = Set<Int>()
var simulateUntrusted = false
var nextObservation = 1
var savedClipboard: [[(NSPasteboard.PasteboardType, Data)]]? = nil

final class ElementRecord {
    let element: AXUIElement
    let role: String
    let name: String
    init(element: AXUIElement, role: String, name: String) {
        self.element = element
        self.role = role
        self.name = name
    }
}

final class ObservationRecord {
    let id: String
    let pid: pid_t
    let windowID: CGWindowID
    var elements: [String: ElementRecord] = [:]
    init(id: String, pid: pid_t, windowID: CGWindowID) {
        self.id = id
        self.pid = pid
        self.windowID = windowID
    }
}

var observations: [String: ObservationRecord] = [:]
var observationOrder: [String] = []
let MAX_OBSERVATIONS = 16

func isCancelled(_ id: Int) -> Bool {
    stateLock.lock()
    defer { stateLock.unlock() }
    return cancelled.contains(id)
}

func checkCancelled(_ id: Int, _ details: [String: Any] = [:]) throws {
    if isCancelled(id) {
        throw fail("cancelled", "The operation was cancelled.", details)
    }
}

// MARK: - Accessibility primitives

func axTrusted() -> Bool {
    if simulateUntrusted { return false }
    return AXIsProcessTrusted()
}

func requireTrusted() throws {
    if !axTrusted() {
        throw fail("permission_denied",
                   "Accessibility permission is not granted to this process. Grant it in System Settings > Privacy & Security > Accessibility to the terminal or app that runs hame, then restart hame.",
                   ["permission": "accessibility"])
    }
}

func copyAttr(_ e: AXUIElement, _ name: String) -> (CFTypeRef?, AXError) {
    var value: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(e, name as CFString, &value)
    return (err == .success ? value : nil, err)
}

func attr(_ e: AXUIElement, _ name: String) -> CFTypeRef? {
    copyAttr(e, name).0
}

func strAttr(_ e: AXUIElement, _ name: String) -> String? {
    guard let v = attr(e, name) else { return nil }
    if let s = v as? String { return s }
    if CFGetTypeID(v) == CFAttributedStringGetTypeID() {
        return (v as! NSAttributedString).string
    }
    return nil
}

func boolAttr(_ e: AXUIElement, _ name: String) -> Bool? {
    guard let v = attr(e, name) else { return nil }
    if let b = v as? Bool { return b }
    if let n = v as? NSNumber { return n.boolValue }
    return nil
}

func elementsAttr(_ e: AXUIElement, _ name: String) -> [AXUIElement] {
    guard let v = attr(e, name) else { return [] }
    if CFGetTypeID(v) == CFArrayGetTypeID() {
        return (v as? [AXUIElement]) ?? []
    }
    return []
}

func elementAttr(_ e: AXUIElement, _ name: String) -> AXUIElement? {
    guard let v = attr(e, name), CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
    return (v as! AXUIElement)
}

func frameOf(_ e: AXUIElement) -> CGRect? {
    guard let p = attr(e, kAXPositionAttribute), let s = attr(e, kAXSizeAttribute),
          CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID()
    else { return nil }
    var point = CGPoint.zero
    var size = CGSize.zero
    AXValueGetValue(p as! AXValue, .cgPoint, &point)
    AXValueGetValue(s as! AXValue, .cgSize, &size)
    return CGRect(origin: point, size: size)
}

func rectJSON(_ r: CGRect?) -> [Double]? {
    guard let r = r, r.origin.x.isFinite, r.origin.y.isFinite, r.width.isFinite, r.height.isFinite
    else { return nil }
    return [Double(r.origin.x), Double(r.origin.y), Double(r.width), Double(r.height)]
}

func valueString(_ e: AXUIElement, limit: Int) -> String? {
    guard let v = attr(e, kAXValueAttribute) else { return nil }
    var s: String?
    if let str = v as? String { s = str }
    else if CFGetTypeID(v) == CFAttributedStringGetTypeID() { s = (v as! NSAttributedString).string }
    else if let n = v as? NSNumber { s = n.stringValue }
    guard var out = s else { return nil }
    if out.count > limit {
        out = String(out.prefix(limit)) + "…"
    }
    return out
}

func windowID(of window: AXUIElement) -> CGWindowID? {
    var id: CGWindowID = 0
    let err = _AXUIElementGetWindow(window, &id)
    return err == .success && id != 0 ? id : nil
}

func roleOf(_ e: AXUIElement) -> (String?, AXError) {
    let (v, err) = copyAttr(e, kAXRoleAttribute)
    return (v as? String, err)
}

/// The human-facing name of an element: its title, description, or the
/// title of the element that labels it.
func nameOf(_ e: AXUIElement) -> String {
    if let t = strAttr(e, kAXTitleAttribute), !t.isEmpty { return t }
    if let d = strAttr(e, kAXDescriptionAttribute), !d.isEmpty { return d }
    if let labelEl = elementAttr(e, kAXTitleUIElementAttribute) {
        if let t = strAttr(labelEl, kAXValueAttribute), !t.isEmpty { return t }
        if let t = strAttr(labelEl, kAXTitleAttribute), !t.isEmpty { return t }
    }
    if let p = strAttr(e, kAXPlaceholderValueAttribute), !p.isEmpty { return p }
    return ""
}

func normalize(_ s: String) -> String {
    s.replacingOccurrences(of: "...", with: "…")
        .trimmingCharacters(in: .whitespacesAndNewlines)
        .lowercased()
}

func withoutEllipsis(_ s: String) -> String {
    var n = normalize(s)
    while n.hasSuffix("…") { n.removeLast() }
    return n.trimmingCharacters(in: .whitespaces)
}

func roleMatches(_ wanted: String?, _ role: String) -> Bool {
    guard let wanted = wanted, !wanted.isEmpty else { return true }
    let w = wanted.lowercased()
    let r = role.lowercased()
    return r == w || r == "ax" + w
}

// MARK: - Applications and windows

func runningApp(pid: pid_t) -> NSRunningApplication? {
    NSRunningApplication(processIdentifier: pid)
}

func appJSON(_ app: NSRunningApplication) -> [String: Any] {
    var out: [String: Any] = [
        "pid": Int(app.processIdentifier),
        "name": app.localizedName ?? "",
        "bundle_id": app.bundleIdentifier ?? "",
        "active": app.isActive,
        "hidden": app.isHidden,
    ]
    if let date = app.launchDate {
        out["launch_ms"] = Int64(date.timeIntervalSince1970 * 1000)
    }
    return out
}

func resolveApp(_ req: [String: Any]) throws -> NSRunningApplication {
    if let pid = req["pid"] as? Int {
        guard let app = runningApp(pid: pid_t(pid)), !app.isTerminated else {
            throw fail("stale_window", "Process \(pid) is no longer running.")
        }
        if let wanted = req["app"] as? String, !wanted.isEmpty,
           !appMatches(app, wanted) {
            throw fail("stale_window", "Process \(pid) is not \(wanted) any more.")
        }
        return app
    }
    guard let wanted = req["app"] as? String, !wanted.isEmpty else {
        throw fail("invalid", "Name the application with app (bundle id or name).")
    }
    let candidates = NSWorkspace.shared.runningApplications.filter {
        !$0.isTerminated && appMatches($0, wanted)
    }
    if candidates.isEmpty {
        throw fail("not_found", "No running application matches \(wanted).")
    }
    if candidates.count > 1 {
        throw fail("ambiguous", "\(candidates.count) running processes match \(wanted); pass pid.",
                   ["candidates": candidates.map(appJSON)])
    }
    return candidates[0]
}

func appMatches(_ app: NSRunningApplication, _ wanted: String) -> Bool {
    let w = wanted.lowercased()
    return app.bundleIdentifier?.lowercased() == w || app.localizedName?.lowercased() == w
}

func appElement(_ pid: pid_t) -> AXUIElement {
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 3.0)
    return app
}

/// Sheets attached to a window, nested sheets included.
func sheetsOf(_ window: AXUIElement) -> [AXUIElement] {
    var out: [AXUIElement] = []
    var level = elementsAttr(window, kAXChildrenAttribute).filter { roleOf($0).0 == "AXSheet" }
    var depth = 0
    while !level.isEmpty && depth < 4 {
        out.append(contentsOf: level)
        level = level.flatMap { elementsAttr($0, kAXChildrenAttribute).filter { roleOf($0).0 == "AXSheet" } }
        depth += 1
    }
    return out
}

/// The app's focused window and the top-level window it belongs to: a
/// sheet belongs to the window it is attached to.
func focusedWindows(_ appEl: AXUIElement) -> (focused: AXUIElement, top: AXUIElement)? {
    guard let f = elementAttr(appEl, kAXFocusedWindowAttribute) else { return nil }
    var top = f
    var hops = 0
    // Sheets nest (a Go To sheet on a save sheet); climb to the window.
    while roleOf(top).0 == "AXSheet", hops < 8, let parent = elementAttr(top, kAXParentAttribute) {
        top = parent
        hops += 1
    }
    return (f, top)
}

/// Whether keyboard input to the app would go to window `wid` (or a sheet
/// attached to it), with the app frontmost.
func targetHasFocus(pid: pid_t, appEl: AXUIElement, windowID wid: CGWindowID) -> Bool {
    guard frontmostPid() == pid, let fw = focusedWindows(appEl) else { return false }
    return windowID(of: fw.top) == wid
}

func windowJSON(_ window: AXUIElement, app: NSRunningApplication, appEl: AXUIElement) -> [String: Any]? {
    guard let id = windowID(of: window) else { return nil }
    let focused = focusedWindows(appEl).map { CFEqual($0.top, window) } ?? false
    var out: [String: Any] = [
        "window_id": Int(id),
        "pid": Int(app.processIdentifier),
        "app": app.localizedName ?? "",
        "bundle_id": app.bundleIdentifier ?? "",
        "title": strAttr(window, kAXTitleAttribute) ?? "",
        "focused": focused,
        "main": boolAttr(window, kAXMainAttribute) ?? false,
        "minimized": boolAttr(window, kAXMinimizedAttribute) ?? false,
        "role": roleOf(window).0 ?? "",
        "subrole": strAttr(window, kAXSubroleAttribute) ?? "",
        "sheets": sheetsOf(window).map { nameOf($0) },
    ]
    if let f = rectJSON(frameOf(window)) { out["frame"] = f }
    if let date = app.launchDate {
        out["launch_ms"] = Int64(date.timeIntervalSince1970 * 1000)
    }
    return out
}

func axWindows(_ appEl: AXUIElement) -> [AXUIElement] {
    elementsAttr(appEl, kAXWindowsAttribute)
}

/// Finds one window of an app by id or title, or the app's only/focused
/// window. Never guesses between two equally good candidates.
func resolveWindow(_ req: [String: Any]) throws -> (NSRunningApplication, AXUIElement, AXUIElement, CGWindowID) {
    try requireTrusted()
    let app = try resolveApp(req)
    let appEl = appElement(app.processIdentifier)
    let windows = axWindows(appEl)
    if let wanted = req["window_id"] as? Int {
        for w in windows where windowID(of: w) == CGWindowID(wanted) {
            return (app, appEl, w, CGWindowID(wanted))
        }
        throw fail("stale_window",
                   "Window \(wanted) of \(app.localizedName ?? "the app") no longer exists; it was closed or replaced. Observe again.",
                   ["window_id": wanted])
    }
    let identified = windows.compactMap { w in windowID(of: w).map { (w, $0) } }
    if let title = req["title"] as? String, !title.isEmpty {
        let exact = identified.filter { (strAttr($0.0, kAXTitleAttribute) ?? "") == title }
        let pool = exact.isEmpty
            ? identified.filter { normalize(strAttr($0.0, kAXTitleAttribute) ?? "").contains(normalize(title)) }
            : exact
        if pool.count == 1 { return (app, appEl, pool[0].0, pool[0].1) }
        if pool.isEmpty {
            throw fail("not_found", "No window of \(app.localizedName ?? "the app") is titled \(title).",
                       ["windows": identified.compactMap { windowJSON($0.0, app: app, appEl: appEl) }])
        }
        throw fail("ambiguous", "\(pool.count) windows of \(app.localizedName ?? "the app") match the title \(title); pass window.",
                   ["candidates": pool.compactMap { windowJSON($0.0, app: app, appEl: appEl) }])
    }
    if identified.count == 1 { return (app, appEl, identified[0].0, identified[0].1) }
    if identified.isEmpty {
        throw fail("not_found", "\(app.localizedName ?? "The app") has no accessible windows.")
    }
    if let fw = focusedWindows(appEl), let id = windowID(of: fw.top) {
        return (app, appEl, fw.top, id)
    }
    throw fail("ambiguous", "\(app.localizedName ?? "The app") has \(identified.count) windows and none is focused; pass window or title.",
               ["candidates": identified.compactMap { windowJSON($0.0, app: app, appEl: appEl) }])
}

// MARK: - Observation

/// Roles always shown, named or not: things a user can act on.
let CONTROL_ROLES: Set<String> = [
    "AXButton", "AXCheckBox", "AXRadioButton", "AXTextField", "AXTextArea", "AXSecureTextField",
    "AXComboBox", "AXPopUpButton", "AXMenuButton", "AXSlider", "AXIncrementor", "AXLink",
    "AXTabGroup", "AXTable", "AXOutline", "AXList", "AXSheet", "AXWindow", "AXMenuItem",
    "AXDisclosureTriangle", "AXSearchField", "AXColorWell", "AXWebArea", "AXDateField",
    "AXStepper", "AXSegmentedControl", "AXBrowser",
]

/// Roles that are scrolling and layout machinery, skipped with their children.
let SKIP_ROLES: Set<String> = ["AXScrollBar", "AXColumn", "AXSplitter", "AXGrowArea", "AXValueIndicator", "AXRuler"]

/// Roles whose children are rendering detail rather than controls.
let LEAF_ROLES: Set<String> = ["AXTextArea", "AXTextField", "AXStaticText", "AXImage", "AXSecureTextField"]

/// The attributes an observation reads per element, fetched in one
/// accessibility round trip instead of one per attribute.
let OBSERVED_ATTRIBUTES: [String] = [
    kAXRoleAttribute, kAXTitleAttribute, kAXDescriptionAttribute, kAXValueAttribute,
    kAXSubroleAttribute, kAXEnabledAttribute, kAXIdentifierAttribute, kAXPositionAttribute,
    kAXSizeAttribute, kAXChildrenAttribute, kAXPlaceholderValueAttribute, kAXTitleUIElementAttribute,
]

struct NodeAttributes {
    var values: [CFTypeRef?]

    init(_ e: AXUIElement) {
        var raw: CFArray?
        let err = AXUIElementCopyMultipleAttributeValues(
            e, OBSERVED_ATTRIBUTES as CFArray, AXCopyMultipleAttributeOptions(rawValue: 0), &raw)
        var out = [CFTypeRef?](repeating: nil, count: OBSERVED_ATTRIBUTES.count)
        if err == .success, let array = raw as? [AnyObject], array.count == OBSERVED_ATTRIBUTES.count {
            for (i, v) in array.enumerated() {
                let ref = v as CFTypeRef
                // Missing attributes come back as AXValues holding an error.
                if CFGetTypeID(ref) == AXValueGetTypeID(), AXValueGetType(ref as! AXValue) == .axError {
                    continue
                }
                out[i] = ref
            }
        }
        values = out
    }

    func string(_ i: Int) -> String? {
        guard let v = values[i] else { return nil }
        if let s = v as? String { return s }
        if CFGetTypeID(v) == CFAttributedStringGetTypeID() { return (v as! NSAttributedString).string }
        return nil
    }

    var role: String? { string(0) }

    var name: String {
        if let t = string(1), !t.isEmpty { return t }
        if let d = string(2), !d.isEmpty { return d }
        if let labelRef = values[11], CFGetTypeID(labelRef) == AXUIElementGetTypeID() {
            let label = labelRef as! AXUIElement
            if let t = strAttr(label, kAXValueAttribute), !t.isEmpty { return t }
            if let t = strAttr(label, kAXTitleAttribute), !t.isEmpty { return t }
        }
        if let p = string(10), !p.isEmpty { return p }
        return ""
    }

    func value(limit: Int) -> String? {
        guard let v = values[3] else { return nil }
        var s: String?
        if let str = v as? String { s = str }
        else if CFGetTypeID(v) == CFAttributedStringGetTypeID() { s = (v as! NSAttributedString).string }
        else if let n = v as? NSNumber { s = n.stringValue }
        guard var out = s else { return nil }
        if out.count > limit { out = String(out.prefix(limit)) + "…" }
        return out
    }

    var subrole: String? { string(4) }
    var enabled: Bool? { (values[5] as? NSNumber)?.boolValue }
    var identifier: String? { string(6) }

    var frame: CGRect? {
        guard let p = values[7], let sz = values[8],
              CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(sz) == AXValueGetTypeID() else { return nil }
        var point = CGPoint.zero
        var size = CGSize.zero
        AXValueGetValue(p as! AXValue, .cgPoint, &point)
        AXValueGetValue(sz as! AXValue, .cgSize, &size)
        return CGRect(origin: point, size: size)
    }

    var children: [AXUIElement] {
        guard let v = values[9], CFGetTypeID(v) == CFArrayGetTypeID() else { return [] }
        return (v as? [AXUIElement]) ?? []
    }
}

func observe(_ req: [String: Any]) throws -> [String: Any] {
    let (app, appEl, window, wid) = try resolveWindow(req)
    let maxNodes = max(10, min((req["max_nodes"] as? Int) ?? 150, 1500))
    let maxDepth = max(2, min((req["max_depth"] as? Int) ?? 30, 60))
    stateLock.lock()
    let obsID = "o\(nextObservation)"
    nextObservation += 1
    stateLock.unlock()
    let record = ObservationRecord(id: obsID, pid: app.processIdentifier, windowID: wid)
    var nodes: [[String: Any]] = []
    var complete = true
    var counter = 0
    var dialogs: [[String: Any]] = []
    let focusedEl = elementAttr(appEl, kAXFocusedUIElementAttribute)
    var focusedRef: String? = nil

    func visit(_ e: AXUIElement, depth: Int, parentName: String) {
        if nodes.count >= maxNodes { complete = false; return }
        let a = NodeAttributes(e)
        guard let role = a.role else { return }
        if SKIP_ROLES.contains(role) { return }
        let name = a.name
        let value = a.value(limit: 300)
        let subrole = a.subrole
        let hasValue = value.map { !$0.isEmpty } ?? false
        var keep: Bool
        if depth == 0 || CONTROL_ROLES.contains(role) {
            keep = true
        } else if role == "AXStaticText" {
            // A label repeating its container's name says nothing new.
            keep = hasValue && value != parentName
        } else if role == "AXImage" {
            keep = !name.isEmpty
        } else {
            keep = !name.isEmpty || hasValue
        }
        var childDepth = depth
        if keep {
            counter += 1
            let ref = "e\(counter)"
            record.elements[ref] = ElementRecord(element: e, role: role, name: name)
            var node: [String: Any] = ["ref": ref, "depth": depth, "role": role, "name": name]
            if let s = subrole, !s.isEmpty { node["subrole"] = s }
            if let v = value { node["value"] = v }
            if let enabled = a.enabled, !enabled { node["enabled"] = false }
            if let f = focusedEl, CFEqual(f, e) { node["focused"] = true; focusedRef = ref }
            if let id = a.identifier, !id.isEmpty { node["identifier"] = id }
            if let fr = rectJSON(a.frame) { node["frame"] = fr }
            nodes.append(node)
            if role == "AXSheet" || subrole == "AXDialog" || subrole == "AXSystemDialog" {
                dialogs.append(["ref": ref, "role": role, "subrole": subrole ?? "", "name": name])
            }
            childDepth = depth + 1
        }
        if LEAF_ROLES.contains(role) { return }
        if depth >= maxDepth { complete = false; return }
        var children = a.children
        // Sheets first: a dialog's contents matter more than what it covers.
        if role == "AXWindow" || role == "AXSheet" {
            let sheets = children.filter { roleOf($0).0 == "AXSheet" }
            if !sheets.isEmpty {
                children = sheets + children.filter { c in !sheets.contains { CFEqual($0, c) } }
            }
        }
        for child in children {
            if nodes.count >= maxNodes { complete = false; return }
            visit(child, depth: childDepth, parentName: keep ? name : parentName)
        }
    }
    visit(window, depth: 0, parentName: "")

    // Other windows of the app that are dialogs (alerts are separate windows).
    var appDialogs: [[String: Any]] = []
    for w in axWindows(appEl) where windowID(of: w) != wid {
        let sub = strAttr(w, kAXSubroleAttribute) ?? ""
        if sub == "AXDialog" || sub == "AXSystemDialog" || sub == "AXFloatingWindow" {
            if let info = windowJSON(w, app: app, appEl: appEl) { appDialogs.append(info) }
        }
    }

    if nodes.count <= 1 && elementsAttr(window, kAXChildrenAttribute).isEmpty {
        throw fail("unsupported",
                   "This window exposes no accessibility content. Use the Visual channel explicitly (screenshot, then click_at) if that is acceptable.",
                   ["window_id": Int(wid)])
    }

    stateLock.lock()
    observations[obsID] = record
    observationOrder.append(obsID)
    while observationOrder.count > MAX_OBSERVATIONS {
        observations.removeValue(forKey: observationOrder.removeFirst())
    }
    stateLock.unlock()

    var out: [String: Any] = [
        "observation": obsID,
        "window": windowJSON(window, app: app, appEl: appEl) ?? [:],
        "nodes": nodes,
        "complete": complete,
        "dialogs": dialogs,
        "app_dialogs": appDialogs,
    ]
    if let f = focusedRef { out["focused_ref"] = f }
    return out
}

// MARK: - Element resolution and preflight

struct Resolved {
    let app: NSRunningApplication
    let appEl: AXUIElement
    let window: AXUIElement
    let windowID: CGWindowID
    let element: AXUIElement
    let role: String
    let name: String
    let observation: String?
}

func inSavePanel(_ e: AXUIElement) -> Bool {
    // Walk up to the enclosing sheet or dialog and look for the name field
    // every NSSavePanel has.
    var cursor: AXUIElement? = e
    var hops = 0
    while let c = cursor, hops < 40 {
        let role = roleOf(c).0 ?? ""
        let sub = strAttr(c, kAXSubroleAttribute) ?? ""
        if role == "AXSheet" || sub == "AXDialog" || sub == "AXSystemDialog" {
            return findFirst(c, limit: 600) { el in
                strAttr(el, kAXIdentifierAttribute) == "saveAsNameTextField"
            } != nil
        }
        // A document window is not a panel; only a sheet or dialog in it is.
        if role == "AXWindow" { return false }
        cursor = elementAttr(c, kAXParentAttribute)
        hops += 1
    }
    return false
}

func inDialog(_ e: AXUIElement) -> Bool {
    var cursor: AXUIElement? = e
    var hops = 0
    while let c = cursor, hops < 40 {
        let role = roleOf(c).0 ?? ""
        let sub = strAttr(c, kAXSubroleAttribute) ?? ""
        if role == "AXSheet" || sub == "AXDialog" || sub == "AXSystemDialog" { return true }
        if role == "AXWindow" { return false }
        cursor = elementAttr(c, kAXParentAttribute)
        hops += 1
    }
    return false
}

/// Whether `e` is (still) inside `window`, following parents.
func attached(_ e: AXUIElement, to window: AXUIElement) -> Bool {
    var cursor: AXUIElement? = e
    var hops = 0
    while let c = cursor, hops < 80 {
        if CFEqual(c, window) { return true }
        cursor = elementAttr(c, kAXParentAttribute)
        hops += 1
    }
    return false
}

func findFirst(_ root: AXUIElement, limit: Int, _ pred: (AXUIElement) -> Bool) -> AXUIElement? {
    var queue = [root]
    var seen = 0
    while !queue.isEmpty && seen < limit {
        let e = queue.removeFirst()
        seen += 1
        if pred(e) { return e }
        queue.append(contentsOf: elementsAttr(e, kAXChildrenAttribute))
    }
    return nil
}

func findAll(_ root: AXUIElement, limit: Int, _ pred: (AXUIElement) -> Bool) -> [AXUIElement] {
    var queue = [root]
    var seen = 0
    var out: [AXUIElement] = []
    while !queue.isEmpty && seen < limit {
        let e = queue.removeFirst()
        seen += 1
        if pred(e) { out.append(e) }
        let role = roleOf(e).0 ?? ""
        if !LEAF_ROLES.contains(role) || role == "AXTextField" {
            queue.append(contentsOf: elementsAttr(e, kAXChildrenAttribute))
        }
    }
    return out
}

/// Resolves the element a request names, re-checking it against the live
/// state: by observation ref (role and name must be unchanged) or by a live
/// search for a unique name.
func resolveElement(_ req: [String: Any]) throws -> Resolved {
    try requireTrusted()
    if let obsID = req["observation"] as? String, let ref = req["element"] as? String {
        stateLock.lock()
        let record = observations[obsID]
        stateLock.unlock()
        guard let record = record else {
            throw fail("stale_ref", "Observation \(obsID) is unknown or too old. Observe again and use the new refs.")
        }
        guard let el = record.elements[ref] else {
            throw fail("stale_ref", "Observation \(obsID) has no element \(ref).")
        }
        var windowReq = req
        windowReq["pid"] = Int(record.pid)
        windowReq["window_id"] = Int(record.windowID)
        windowReq.removeValue(forKey: "title")
        let (app, appEl, window, wid) = try resolveWindow(windowReq)
        let (role, err) = roleOf(el.element)
        if err == .invalidUIElement || role == nil {
            throw fail("stale_ref", "Element \(obsID):\(ref) (\(el.role) \"\(el.name)\") no longer exists. Observe again.")
        }
        // A closed sheet's controls can outlive it (apps reuse panels), so
        // an element must still hang off the target window.
        if !attached(el.element, to: window) {
            throw fail("stale_ref", "Element \(obsID):\(ref) (\(el.role) \"\(el.name)\") is no longer part of window \(wid); the sheet or view that held it went away. Observe again.")
        }
        let name = nameOf(el.element)
        if role != el.role || name != el.name {
            throw fail("stale_ref",
                       "Element \(obsID):\(ref) changed: it was \(el.role) \"\(el.name)\", now \(role!) \"\(name)\". Observe again.")
        }
        return Resolved(app: app, appEl: appEl, window: window, windowID: wid, element: el.element,
                        role: role!, name: name, observation: obsID)
    }
    let wanted = req["name"] as? String ?? ""
    if wanted.isEmpty && (req["role"] as? String ?? "").isEmpty {
        throw fail("invalid", "Name the element with ref (from observe), or name and/or role.")
    }
    let (app, appEl, window, wid) = try resolveWindow(req)
    let roleWanted = req["role"] as? String
    let target = normalize(wanted)
    let matches = findAll(window, limit: 4000) { e in
        let role = roleOf(e).0 ?? ""
        let sub = strAttr(e, kAXSubroleAttribute) ?? ""
        return (roleMatches(roleWanted, role) || (!sub.isEmpty && roleMatches(roleWanted, sub)))
            && (target.isEmpty || normalize(nameOf(e)) == target)
    }
    if matches.isEmpty {
        throw fail("not_found", wanted.isEmpty ? "No element with role \(roleWanted ?? "") in window \(wid)." : "No element named \(wanted) in window \(wid).")
    }
    if matches.count > 1 {
        let desc = matches.prefix(8).map { e -> [String: Any] in
            var d: [String: Any] = ["role": roleOf(e).0 ?? "", "name": nameOf(e)]
            if let f = rectJSON(frameOf(e)) { d["frame"] = f }
            return d
        }
        throw fail("ambiguous", "\(matches.count) elements are named \(wanted); observe and use a ref, or add role.",
                   ["candidates": desc])
    }
    let e = matches[0]
    return Resolved(app: app, appEl: appEl, window: window, windowID: wid, element: e,
                    role: roleOf(e).0 ?? "", name: nameOf(e), observation: nil)
}

func elementJSON(_ r: Resolved) -> [String: Any] {
    var out: [String: Any] = [
        "role": r.role,
        "name": r.name,
        "enabled": boolAttr(r.element, kAXEnabledAttribute) ?? true,
        "in_dialog": inDialog(r.element),
        "save_panel": inSavePanel(r.element),
        "window": windowJSON(r.window, app: r.app, appEl: r.appEl) ?? [:],
    ]
    if let v = valueString(r.element, limit: 100_000) { out["value"] = v }
    if let f = rectJSON(frameOf(r.element)) { out["frame"] = f }
    if let s = strAttr(r.element, kAXSubroleAttribute) { out["subrole"] = s }
    if let id = strAttr(r.element, kAXIdentifierAttribute) { out["identifier"] = id }
    return out
}

/// The Rust side resolved the element first to decide which capability the
/// action needs; it passes back what it saw, and the action is refused if
/// the element is no longer that.
func checkExpected(_ r: Resolved, _ req: [String: Any]) throws {
    if let role = req["expect_role"] as? String, role != r.role {
        throw fail("stale_ref", "The element is now \(r.role), not \(role). Observe again.")
    }
    if let name = req["expect_name"] as? String, name != r.name {
        throw fail("stale_ref", "The element is now named \"\(r.name)\", not \"\(name)\". Observe again.")
    }
}

func windowState(_ r: Resolved) -> [String: Any] {
    windowJSON(r.window, app: r.app, appEl: r.appEl) ?? [:]
}

// MARK: - Semantic actions

func press(_ req: [String: Any]) throws -> [String: Any] {
    let r = try resolveElement(req)
    try checkExpected(r, req)
    if boolAttr(r.element, kAXEnabledAttribute) == false {
        throw fail("disabled", "\(r.role) \"\(r.name)\" is disabled.")
    }
    var actions: CFArray?
    AXUIElementCopyActionNames(r.element, &actions)
    let names = (actions as? [String]) ?? []
    let action = names.contains(kAXPressAction as String) ? kAXPressAction as String
        : (names.contains("AXConfirm") ? "AXConfirm" : (names.contains("AXPick") ? "AXPick" : ""))
    if action.isEmpty {
        throw fail("unsupported",
                   "\(r.role) \"\(r.name)\" has no press action (actions: \(names.joined(separator: ", "))). Use the Visual channel explicitly (screenshot, then click_at) if that is acceptable.")
    }
    let err = AXUIElementPerformAction(r.element, action as CFString)
    if err != .success {
        throw fail("failed", "Pressing \(r.role) \"\(r.name)\" failed (AX error \(err.rawValue)).",
                   ["dispatched": true])
    }
    Thread.sleep(forTimeInterval: 0.05)
    return ["pressed": r.name, "role": r.role, "window": windowState(r)]
}

func setValue(_ req: [String: Any]) throws -> [String: Any] {
    let r = try resolveElement(req)
    try checkExpected(r, req)
    guard let value = req["value"] as? String else { throw fail("invalid", "set_value needs value.") }
    var settable = DarwinBoolean(false)
    AXUIElementIsAttributeSettable(r.element, kAXValueAttribute as CFString, &settable)
    if !settable.boolValue {
        throw fail("unsupported",
                   "The value of \(r.role) \"\(r.name)\" cannot be set through accessibility. Use type (physical input) explicitly if that is acceptable.")
    }
    let err = AXUIElementSetAttributeValue(r.element, kAXValueAttribute as CFString, value as CFString)
    if err != .success {
        throw fail("failed", "Setting the value failed (AX error \(err.rawValue)).", ["dispatched": true])
    }
    let after = valueString(r.element, limit: 1_000_000) ?? ""
    return ["value": after, "matches": after == value, "window": windowState(r)]
}

func frontmostPid() -> pid_t? {
    let system = AXUIElementCreateSystemWide()
    AXUIElementSetMessagingTimeout(system, 2.0)
    if let app = elementAttr(system, kAXFocusedApplicationAttribute) {
        var pid: pid_t = 0
        if AXUIElementGetPid(app, &pid) == .success { return pid }
    }
    return NSWorkspace.shared.frontmostApplication?.processIdentifier
}

func focus(_ req: [String: Any]) throws -> [String: Any] {
    var r: Resolved
    let hasElement = req["element"] != nil || req["name"] != nil || req["role"] != nil
    if hasElement {
        r = try resolveElement(req)
        try checkExpected(r, req)
    } else {
        let (app, appEl, window, wid) = try resolveWindow(req)
        r = Resolved(app: app, appEl: appEl, window: window, windowID: wid, element: window,
                     role: "AXWindow", name: strAttr(window, kAXTitleAttribute) ?? "", observation: nil)
    }
    AXUIElementSetAttributeValue(r.appEl, kAXFrontmostAttribute as CFString, kCFBooleanTrue)
    AXUIElementPerformAction(r.window, kAXRaiseAction as CFString)
    AXUIElementSetAttributeValue(r.window, kAXMainAttribute as CFString, kCFBooleanTrue)
    if hasElement {
        AXUIElementSetAttributeValue(r.element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
    }
    let deadline = Date().addingTimeInterval(1.5)
    var activated = false
    while Date() < deadline {
        if targetHasFocus(pid: r.app.processIdentifier, appEl: r.appEl, windowID: r.windowID) {
            activated = true
            break
        }
        if frontmostPid() != r.app.processIdentifier { r.app.activate(options: []) }
        Thread.sleep(forTimeInterval: 0.05)
    }
    if !activated {
        throw fail("interference", "Could not bring window \(r.windowID) to the front; another window kept focus.",
                   ["frontmost_pid": Int(frontmostPid() ?? 0), "dispatched": true])
    }
    return ["focused": true, "window": windowState(r)]
}

func menuItem(_ req: [String: Any]) throws -> [String: Any] {
    try requireTrusted()
    let app = try resolveApp(req)
    guard let path = req["path"] as? [String], !path.isEmpty else {
        throw fail("invalid", "menu needs path, such as File > Save….")
    }
    let appEl = appElement(app.processIdentifier)
    guard let bar = elementAttr(appEl, kAXMenuBarAttribute) else {
        throw fail("unsupported", "\(app.localizedName ?? "The app") exposes no menu bar.")
    }
    var items = elementsAttr(bar, kAXChildrenAttribute)
    var current = bar
    var topItem: AXUIElement? = nil
    var topMenu: AXUIElement? = nil
    var trail: [String] = []
    for (i, step) in path.enumerated() {
        var matches = items.filter { normalize(strAttr($0, kAXTitleAttribute) ?? "") == normalize(step) }
        if matches.isEmpty {
            // Apps add or drop the ellipsis as the item's state changes
            // ("Save…" for an untitled document, "Save" otherwise).
            matches = items.filter { withoutEllipsis(strAttr($0, kAXTitleAttribute) ?? "") == withoutEllipsis(step) }
        }
        if matches.isEmpty {
            let available = items.compactMap { strAttr($0, kAXTitleAttribute) }.filter { !$0.isEmpty }
            throw fail("not_found", "No menu item \(step) under \(trail.isEmpty ? "the menu bar" : trail.joined(separator: " > ")).",
                       ["available": available])
        }
        if matches.count > 1 {
            throw fail("ambiguous", "\(matches.count) menu items are titled \(step).")
        }
        current = matches[0]
        trail.append(strAttr(current, kAXTitleAttribute) ?? step)
        if i < path.count - 1 {
            // A menu bar item or a submenu item holds its items in an AXMenu child.
            guard let sub = elementsAttr(current, kAXChildrenAttribute).first(where: { roleOf($0).0 == "AXMenu" }) else {
                throw fail("not_found", "Menu item \(trail.joined(separator: " > ")) has no submenu.")
            }
            if i == 0 { topItem = current; topMenu = sub }
            items = elementsAttr(sub, kAXChildrenAttribute)
        }
    }
    var enabled = boolAttr(current, kAXEnabledAttribute) ?? true
    if !enabled, let top = topItem, let menu = topMenu {
        // Apps enable and retitle menu items when the menu opens; what an
        // unopened menu reports can be stale (still disabled after a sheet
        // closed). Open the menu to have it validated, then close it.
        AXUIElementPerformAction(top, kAXPressAction as CFString)
        let deadline = Date().addingTimeInterval(0.5)
        while Date() < deadline {
            if boolAttr(current, kAXEnabledAttribute) == true { break }
            Thread.sleep(forTimeInterval: 0.03)
        }
        enabled = boolAttr(current, kAXEnabledAttribute) ?? true
        AXUIElementPerformAction(menu, kAXCancelAction as CFString)
        Thread.sleep(forTimeInterval: 0.1)
    }
    let title = strAttr(current, kAXTitleAttribute) ?? ""
    if let expected = req["expect_title"] as? String, expected != title {
        throw fail("stale_ref", "The menu item is now titled \(title), not \(expected).", ["title": title])
    }
    let info: [String: Any] = ["title": title, "path": trail, "enabled": enabled,
                               "pid": Int(app.processIdentifier), "app": app.localizedName ?? "",
                               "bundle_id": app.bundleIdentifier ?? ""]
    if req["dry_run"] as? Bool == true {
        return info
    }
    if !enabled {
        throw fail("disabled", "Menu item \(trail.joined(separator: " > ")) is disabled.")
    }
    let err = AXUIElementPerformAction(current, kAXPressAction as CFString)
    if err != .success {
        throw fail("failed", "Pressing menu item \(trail.joined(separator: " > ")) failed (AX error \(err.rawValue)).",
                   ["dispatched": true])
    }
    Thread.sleep(forTimeInterval: 0.05)
    var out = info
    out["pressed"] = true
    return out
}

// MARK: - Physical input

let modifierFlags: [String: CGEventFlags] = [
    "cmd": .maskCommand, "shift": .maskShift, "opt": .maskAlternate, "ctrl": .maskControl, "fn": .maskSecondaryFn,
]

/// Key codes of the modifier keys, released on exit in case anything was
/// left down.
let modifierKeyCodes: [CGKeyCode] = [55, 54, 56, 60, 58, 61, 59, 62, 63]

func releaseModifiers() {
    let src = CGEventSource(stateID: .privateState)
    for code in modifierKeyCodes {
        if let up = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false) {
            up.flags = []
            up.post(tap: .cghidEventTap)
        }
    }
}

/// Checks that the target is still where physical input would land. Run
/// before every chunk.
/// Events are posted to the target process unless a sheet has focus: open
/// and save panels run in another process and only see events through the
/// system event stream. Posting there is safe only because the target was
/// just verified frontmost and focused, and is re-verified before each chunk.
func postsToHID(_ req: [String: Any], _ t: InputTarget) -> Bool {
    if let via = req["via"] as? String { return via == "hid" }
    return !CFEqual(t.focusedWindow, elementAttr(t.appEl, kAXFocusedWindowAttribute) ?? t.appEl)
        || roleOf(t.focusedWindow).0 == "AXSheet"
}

/// What physical input would go to when an operation starts.
struct InputTarget {
    let pid: pid_t
    let appEl: AXUIElement
    let windowID: CGWindowID
    let focusedWindow: AXUIElement
    let sheets: Int
    let focus: AXUIElement?
}

/// Checks that the target is frontmost and focused, so input would land in
/// it; never moves focus itself.
func captureInputTarget(pid: pid_t, appEl: AXUIElement, windowID wid: CGWindowID) throws -> InputTarget {
    guard targetHasFocus(pid: pid, appEl: appEl, windowID: wid), let fw = focusedWindows(appEl) else {
        throw fail("not_frontmost",
                   "The target window is not frontmost and focused. Call focus first; physical input never steals focus by itself.",
                   ["frontmost_pid": Int(frontmostPid() ?? 0)])
    }
    return InputTarget(pid: pid, appEl: appEl, windowID: wid, focusedWindow: fw.focused,
                       sheets: sheetsOf(fw.top).count,
                       focus: elementAttr(appEl, kAXFocusedUIElementAttribute))
}

/// Checks that input would still land where it did when the operation
/// started. Run before every chunk.
func checkInputTarget(_ t: InputTarget, emitted: String, checkFocus: Bool) throws {
    let details: [String: Any] = ["emitted": emitted, "dispatched": !emitted.isEmpty]
    guard let front = frontmostPid() else {
        throw fail("interference", "Cannot tell which application is frontmost; stopped.", details)
    }
    if front != t.pid {
        let name = runningApp(pid: front)?.localizedName ?? "pid \(front)"
        var d = details
        d["frontmost"] = name
        throw fail("interference", "\(name) became frontmost; input stopped.", d)
    }
    guard let fw = focusedWindows(t.appEl) else {
        throw fail("interference", "The target app has no focused window; input stopped.", details)
    }
    if !CFEqual(fw.focused, t.focusedWindow) {
        let role = roleOf(fw.focused).0 ?? ""
        let sub = strAttr(fw.focused, kAXSubroleAttribute) ?? ""
        var d = details
        d["focused_window"] = windowID(of: fw.top).map { Int($0) } ?? 0
        d["focused_title"] = strAttr(fw.focused, kAXTitleAttribute) ?? nameOf(fw.focused)
        if role == "AXSheet" || sub == "AXDialog" || sub == "AXSystemDialog" {
            throw fail("unexpected_dialog", "A dialog took focus; input stopped.", d)
        }
        throw fail("interference", "Another window took focus; input stopped.", d)
    }
    let sheets = sheetsOf(fw.top)
    if sheets.count > t.sheets {
        var d = details
        d["dialog"] = nameOf(sheets.last!)
        throw fail("unexpected_dialog", "A sheet appeared on the window; input stopped.", d)
    }
    if checkFocus, let start = t.focus {
        let now = elementAttr(t.appEl, kAXFocusedUIElementAttribute)
        if now == nil || !CFEqual(now!, start) {
            throw fail("interference", "Keyboard focus moved to another element; input stopped.", details)
        }
    }
}

func charCount(_ e: AXUIElement?) -> Int? {
    guard let e = e, let v = attr(e, kAXNumberOfCharactersAttribute) as? NSNumber else { return nil }
    return v.intValue
}

func selectedLength(_ e: AXUIElement?) -> Int {
    guard let e = e, let v = attr(e, kAXSelectedTextRangeAttribute), CFGetTypeID(v) == AXValueGetTypeID() else {
        return 0
    }
    var range = CFRange()
    AXValueGetValue(v as! AXValue, .cfRange, &range)
    return range.length
}

func postUnicode(_ text: String, pid: pid_t, hid: Bool) {
    let src = CGEventSource(stateID: .privateState)
    let units = Array(text.utf16)
    for keyDown in [true, false] {
        guard let ev = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: keyDown) else { continue }
        ev.flags = []
        units.withUnsafeBufferPointer { buf in
            ev.keyboardSetUnicodeString(stringLength: buf.count, unicodeString: buf.baseAddress)
        }
        if hid { ev.post(tap: .cghidEventTap) } else { ev.postToPid(pid) }
    }
}

func postKey(_ code: CGKeyCode, flags: CGEventFlags, pid: pid_t, hid: Bool) {
    let src = CGEventSource(stateID: .privateState)
    for keyDown in [true, false] {
        guard let ev = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: keyDown) else { continue }
        ev.flags = flags
        if hid { ev.post(tap: .cghidEventTap) } else { ev.postToPid(pid) }
    }
}

/// Splits text into chunks of at most `size` characters, never splitting a
/// grapheme cluster; newlines and tabs become their own chunks so they can
/// be sent as Return and Tab key presses.
func chunks(_ text: String, size: Int) -> [String] {
    var out: [String] = []
    var current = ""
    var n = 0
    for ch in text {
        if ch == "\n" || ch == "\r\n" || ch == "\t" {
            if !current.isEmpty { out.append(current); current = ""; n = 0 }
            out.append(String(ch))
            continue
        }
        current.append(ch)
        n += 1
        if n >= size || current.utf16.count >= 18 {
            out.append(current); current = ""; n = 0
        }
    }
    if !current.isEmpty { out.append(current) }
    return out
}

func typeText(_ req: [String: Any], id: Int) throws -> [String: Any] {
    guard let text = req["text"] as? String else { throw fail("invalid", "type needs text.") }
    let hasElement = req["element"] != nil || req["name"] != nil || req["role"] != nil
    var appEl: AXUIElement
    var pid: pid_t
    var wid: CGWindowID
    if hasElement {
        let r = try resolveElement(req)
        try checkExpected(r, req)
        appEl = r.appEl; pid = r.app.processIdentifier; wid = r.windowID
        if boolAttr(r.element, kAXFocusedAttribute) != true {
            AXUIElementSetAttributeValue(r.element, kAXFocusedAttribute as CFString, kCFBooleanTrue)
            Thread.sleep(forTimeInterval: 0.05)
        }
    } else {
        let (app, ael, _, w) = try resolveWindow(req)
        appEl = ael; pid = app.processIdentifier; wid = w
    }
    let size = max(1, min((req["chunk"] as? Int) ?? 8, 16))
    let delay = Double(max(0, min((req["delay_ms"] as? Int) ?? 12, 1000))) / 1000.0
    let target = try captureInputTarget(pid: pid, appEl: appEl, windowID: wid)
    let startFocus = target.focus
    let hid = postsToHID(req, target)
    let baseCount = charCount(startFocus)
    let baseSelected = selectedLength(startFocus)
    var emitted = ""
    var chunkCount = 0
    for chunk in chunks(text, size: size) {
        try checkCancelled(id, ["emitted": emitted, "dispatched": !emitted.isEmpty])
        try checkInputTarget(target, emitted: emitted, checkFocus: true)
        if chunk == "\n" || chunk == "\r\n" {
            postKey(36, flags: [], pid: pid, hid: hid)
        } else if chunk == "\t" {
            postKey(48, flags: [], pid: pid, hid: hid)
        } else {
            postUnicode(chunk, pid: pid, hid: hid)
        }
        emitted += chunk
        chunkCount += 1
        // Wait until the app has taken the chunk, so a focus change after
        // this point cannot redirect it.
        if let base = baseCount {
            let expected = base - baseSelected + (emitted as NSString).length
            let deadline = Date().addingTimeInterval(0.4)
            while Date() < deadline, let now = charCount(startFocus), now < expected {
                Thread.sleep(forTimeInterval: 0.005)
            }
        }
        if delay > 0 { Thread.sleep(forTimeInterval: delay) }
    }
    return ["emitted": emitted, "chunks": chunkCount]
}

func pressKey(_ req: [String: Any], id: Int) throws -> [String: Any] {
    guard let code = req["keycode"] as? Int else { throw fail("invalid", "key needs keycode.") }
    let mods = (req["modifiers"] as? [String]) ?? []
    var flags: CGEventFlags = []
    for m in mods {
        guard let f = modifierFlags[m] else { throw fail("invalid", "Unknown modifier \(m).") }
        flags.insert(f)
    }
    let (app, appEl, window, wid) = try resolveWindow(req)
    let target = try captureInputTarget(pid: app.processIdentifier, appEl: appEl, windowID: wid)
    try checkCancelled(id)
    try checkInputTarget(target, emitted: "", checkFocus: false)
    postKey(CGKeyCode(code), flags: flags, pid: app.processIdentifier, hid: postsToHID(req, target))
    Thread.sleep(forTimeInterval: 0.08)
    return ["pressed": req["label"] as? String ?? "", "window": windowJSON(window, app: app, appEl: appEl) ?? [:]]
}

/// The topmost normal window whose bounds contain a point, in CoreGraphics
/// global coordinates.
func windowAt(_ point: CGPoint) -> (CGWindowID, pid_t)? {
    guard let list = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] else { return nil }
    for info in list {
        let layer = info[kCGWindowLayer as String] as? Int ?? 0
        let alpha = info[kCGWindowAlpha as String] as? Double ?? 1
        guard layer == 0, alpha > 0.01,
              let b = info[kCGWindowBounds as String] as? [String: Any],
              let rect = CGRect(dictionaryRepresentation: b as CFDictionary),
              rect.contains(point) else { continue }
        let id = info[kCGWindowNumber as String] as? Int ?? 0
        let pid = info[kCGWindowOwnerPID as String] as? Int ?? 0
        return (CGWindowID(id), pid_t(pid))
    }
    return nil
}

func clickAt(_ req: [String: Any]) throws -> [String: Any] {
    guard let x = req["x"] as? Double, let y = req["y"] as? Double else {
        throw fail("invalid", "click_at needs x and y in screen points.")
    }
    let (app, appEl, window, wid) = try resolveWindow(req)
    guard let frame = frameOf(window) else { throw fail("stale_window", "The window has no frame.") }
    if let expect = req["expect_frame"] as? [Double], expect.count == 4 {
        let e = CGRect(x: expect[0], y: expect[1], width: expect[2], height: expect[3])
        if abs(e.origin.x - frame.origin.x) > 1 || abs(e.origin.y - frame.origin.y) > 1
            || abs(e.width - frame.width) > 1 || abs(e.height - frame.height) > 1 {
            throw fail("stale_capture",
                       "The window moved or was resized since the screenshot (was \(rectJSON(e)!), now \(rectJSON(frame)!)). Take a new screenshot.",
                       ["frame": rectJSON(frame)!])
        }
    }
    let point = CGPoint(x: x, y: y)
    if !frame.contains(point) {
        throw fail("invalid", "Point (\(x), \(y)) is outside window \(wid) \(rectJSON(frame)!).")
    }
    _ = try captureInputTarget(pid: app.processIdentifier, appEl: appEl, windowID: wid)
    if sheetsOf(window).count > 0, req["allow_sheet"] as? Bool != true {
        throw fail("unexpected_dialog", "A sheet covers the window; observe it first.")
    }
    guard let hit = windowAt(point), hit.0 == wid else {
        throw fail("interference", "Another window covers (\(x), \(y)); nothing was clicked.",
                   ["hit_window": Int(windowAt(point)?.0 ?? 0)])
    }
    let src = CGEventSource(stateID: .privateState)
    let restore = CGEvent(source: nil)?.location
    let right = (req["button"] as? String) == "right"
    let count = max(1, min((req["count"] as? Int) ?? 1, 3))
    let downType: CGEventType = right ? .rightMouseDown : .leftMouseDown
    let upType: CGEventType = right ? .rightMouseUp : .leftMouseUp
    let button: CGMouseButton = right ? .right : .left
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: point, mouseButton: button)?
        .post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.02)
    for i in 1...count {
        for type in [downType, upType] {
            if let ev = CGEvent(mouseEventSource: src, mouseType: type, mouseCursorPosition: point, mouseButton: button) {
                ev.setIntegerValueField(.mouseEventClickState, value: Int64(i))
                ev.post(tap: .cghidEventTap)
            }
        }
        Thread.sleep(forTimeInterval: 0.03)
    }
    Thread.sleep(forTimeInterval: 0.05)
    if let r = restore { CGWarpMouseCursorPosition(r) }
    return ["clicked": [x, y], "hit_window": Int(hit.0)]
}

// MARK: - Screenshots

func displayScale(for rect: CGRect) -> Double {
    var ids = [CGDirectDisplayID](repeating: 0, count: 8)
    var count: UInt32 = 0
    CGGetDisplaysWithRect(rect, 8, &ids, &count)
    let id = count > 0 ? ids[0] : CGMainDisplayID()
    guard let mode = CGDisplayCopyDisplayMode(id) else { return 1 }
    let bounds = CGDisplayBounds(id)
    return bounds.width > 0 ? Double(mode.pixelWidth) / Double(bounds.width) : 1
}

func runAsync<T>(_ body: @escaping () async throws -> T) throws -> T {
    let sem = DispatchSemaphore(value: 0)
    var result: Result<T, Error>!
    Task.detached {
        do { result = .success(try await body()) } catch { result = .failure(error) }
        sem.signal()
    }
    if sem.wait(timeout: .now() + 15) == .timedOut {
        throw fail("timeout", "Screen capture did not finish within 15 seconds.")
    }
    return try result.get()
}

func scaled(_ image: CGImage, maxEdge: Int) -> CGImage {
    let w = image.width, h = image.height
    let longest = max(w, h)
    guard maxEdge > 0, longest > maxEdge else { return image }
    let factor = Double(maxEdge) / Double(longest)
    let nw = max(1, Int((Double(w) * factor).rounded())), nh = max(1, Int((Double(h) * factor).rounded()))
    guard let ctx = CGContext(data: nil, width: nw, height: nh, bitsPerComponent: 8, bytesPerRow: 0,
                              space: CGColorSpace(name: CGColorSpace.sRGB)!,
                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return image }
    ctx.interpolationQuality = .high
    ctx.draw(image, in: CGRect(x: 0, y: 0, width: nw, height: nh))
    return ctx.makeImage() ?? image
}

/// A small RGBA grid of the image, for the blank-capture check on the
/// Rust side.
func samples(_ image: CGImage, side: Int) -> String {
    var data = [UInt8](repeating: 0, count: side * side * 4)
    data.withUnsafeMutableBytes { buf in
        if let ctx = CGContext(data: buf.baseAddress, width: side, height: side, bitsPerComponent: 8,
                               bytesPerRow: side * 4, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                               bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) {
            ctx.interpolationQuality = .medium
            ctx.draw(image, in: CGRect(x: 0, y: 0, width: side, height: side))
        }
    }
    return Data(data).base64EncodedString()
}

func png(_ image: CGImage) throws -> Data {
    let data = NSMutableData()
    guard let dest = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil) else {
        throw fail("failed", "Cannot create a PNG encoder.")
    }
    CGImageDestinationAddImage(dest, image, nil)
    if !CGImageDestinationFinalize(dest) { throw fail("failed", "PNG encoding failed.") }
    return data as Data
}

func screenshot(_ req: [String: Any]) throws -> [String: Any] {
    if !CGPreflightScreenCaptureAccess() || req["simulate_no_screen"] as? Bool == true {
        throw fail("permission_denied",
                   "Screen Recording permission is not granted to this process. Grant it in System Settings > Privacy & Security > Screen & System Audio Recording to the terminal or app that runs hame, then restart hame.",
                   ["permission": "screen_recording"])
    }
    let maxEdge = (req["max_edge"] as? Int) ?? 0
    var image: CGImage
    var crop: CGRect
    var scale: Double
    var windowInfo: [String: Any]? = nil
    if let region = req["region"] as? [Double], region.count == 4 {
        let rect = CGRect(x: region[0], y: region[1], width: region[2], height: region[3])
        (image, crop, scale) = try runAsync {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let display = content.displays.first(where: { $0.frame.intersects(rect) }) else {
                throw fail("invalid", "The region is not on any display.")
            }
            let clipped = rect.intersection(display.frame)
            let filter = SCContentFilter(display: display, excludingWindows: [])
            let s = Double(filter.pointPixelScale)
            let config = SCStreamConfiguration()
            config.sourceRect = CGRect(x: clipped.origin.x - display.frame.origin.x,
                                       y: clipped.origin.y - display.frame.origin.y,
                                       width: clipped.width, height: clipped.height)
            config.width = Int((clipped.width * s).rounded())
            config.height = Int((clipped.height * s).rounded())
            config.showsCursor = false
            let img = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
            return (img, clipped, s)
        }
    } else {
        let (app, appEl, window, wid) = try resolveWindow(req)
        windowInfo = windowJSON(window, app: app, appEl: appEl)
        (image, crop, scale) = try runAsync {
            let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            guard let w = content.windows.first(where: { $0.windowID == wid }) else {
                throw fail("unsupported", "Window \(wid) is not on screen (minimized, hidden or on another Space); it cannot be captured.")
            }
            let filter = SCContentFilter(desktopIndependentWindow: w)
            let s = Double(filter.pointPixelScale)
            let rect = filter.contentRect
            let config = SCStreamConfiguration()
            config.width = Int((rect.width * s).rounded())
            config.height = Int((rect.height * s).rounded())
            config.showsCursor = false
            config.ignoreShadowsSingleWindow = true
            config.scalesToFit = false
            let img = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: config)
            return (img, w.frame, s)
        }
    }
    let original = (image.width, image.height)
    let sent = scaled(image, maxEdge: maxEdge)
    let bytes = try png(sent)
    var out: [String: Any] = [
        "png": bytes.base64EncodedString(),
        "width": sent.width,
        "height": sent.height,
        "captured_width": original.0,
        "captured_height": original.1,
        "crop": rectJSON(crop) ?? [0, 0, 0, 0],
        "scale": scale,
        "display_scale": displayScale(for: crop),
        "samples": samples(sent, side: 24),
        "sample_side": 24,
    ]
    if let w = windowInfo { out["window"] = w }
    return out
}

// MARK: - Clipboard

func clipboard(_ req: [String: Any]) throws -> [String: Any] {
    let pb = NSPasteboard.general
    switch req["mode"] as? String ?? "get" {
    case "get":
        return ["text": pb.string(forType: .string) ?? NSNull(), "saved": savedClipboard != nil]
    case "set":
        guard let text = req["text"] as? String else { throw fail("invalid", "clipboard set needs text.") }
        if savedClipboard == nil {
            savedClipboard = (pb.pasteboardItems ?? []).map { item in
                item.types.compactMap { t in item.data(forType: t).map { (t, $0) } }
            }
        }
        pb.clearContents()
        pb.setString(text, forType: .string)
        return ["set": true, "saved": true]
    case "restore":
        let restored = restoreClipboard()
        return ["restored": restored]
    default:
        throw fail("invalid", "clipboard mode is get, set or restore.")
    }
}

@discardableResult
func restoreClipboard() -> Bool {
    guard let saved = savedClipboard else { return false }
    let pb = NSPasteboard.general
    pb.clearContents()
    let items: [NSPasteboardItem] = saved.map { pairs in
        let item = NSPasteboardItem()
        for (t, d) in pairs { item.setData(d, forType: t) }
        return item
    }
    if !items.isEmpty { pb.writeObjects(items) }
    savedClipboard = nil
    return true
}

// MARK: - Waiting

func wait(_ req: [String: Any], id: Int) throws -> [String: Any] {
    let until = req["until"] as? String ?? ""
    let timeout = Double(max(0, min((req["timeout_ms"] as? Int) ?? 5000, 60000))) / 1000.0
    let start = Date()
    var interval = 0.03
    var lastProblem = ""
    while true {
        try checkCancelled(id)
        var satisfied = false
        var detail: [String: Any] = [:]
        do {
            switch until {
            case "window":
                let app = try resolveApp(req)
                let appEl = appElement(app.processIdentifier)
                let title = (req["title"] as? String).map(normalize)
                let found = axWindows(appEl).filter { w in
                    guard windowID(of: w) != nil else { return false }
                    if let excluded = req["exclude"] as? [Int], let id = windowID(of: w), excluded.contains(Int(id)) {
                        return false
                    }
                    return title.map { normalize(strAttr(w, kAXTitleAttribute) ?? "").contains($0) } ?? true
                }
                satisfied = !found.isEmpty
                detail["windows"] = found.compactMap { windowJSON($0, app: app, appEl: appEl) }
            case "window_gone":
                do {
                    _ = try resolveWindow(req)
                    satisfied = false
                } catch let e as HelperError where e.code == "stale_window" {
                    satisfied = true
                }
            case "dialog":
                let (app, appEl, window, _) = try resolveWindow(req)
                let sheets = sheetsOf(window)
                let dialogs = axWindows(appEl).filter {
                    let s = strAttr($0, kAXSubroleAttribute) ?? ""
                    return s == "AXDialog" || s == "AXSystemDialog"
                }
                satisfied = !sheets.isEmpty || !dialogs.isEmpty
                detail["sheets"] = sheets.map { nameOf($0) }
                detail["dialogs"] = dialogs.compactMap { windowJSON($0, app: app, appEl: appEl) }
            case "element", "element_gone":
                do {
                    let r = try resolveElement(req)
                    satisfied = until == "element"
                    detail["element"] = ["role": r.role, "name": r.name]
                } catch let e as HelperError where e.code == "not_found" || e.code == "stale_ref" {
                    satisfied = until == "element_gone"
                    lastProblem = e.message
                }
            case "value":
                let r = try resolveElement(req)
                let v = valueString(r.element, limit: 1_000_000) ?? ""
                if let eq = req["equals"] as? String { satisfied = v == eq }
                else if let c = req["contains"] as? String { satisfied = v.contains(c) }
                else { throw fail("invalid", "wait for value needs equals or contains.") }
                detail["value"] = v.count > 300 ? String(v.prefix(300)) + "…" : v
            case "title":
                let (_, _, window, _) = try resolveWindow(req)
                let t = strAttr(window, kAXTitleAttribute) ?? ""
                satisfied = t.contains(req["contains"] as? String ?? "")
                detail["title"] = t
            default:
                throw fail("invalid", "until is one of window, window_gone, dialog, element, element_gone, value, title.")
            }
        } catch let e as HelperError where e.code == "not_found" && until != "element_gone" {
            lastProblem = e.message
        }
        let waited = Int(Date().timeIntervalSince(start) * 1000)
        if satisfied {
            detail["waited_ms"] = waited
            detail["satisfied"] = true
            return detail
        }
        if Date().timeIntervalSince(start) >= timeout {
            var d = detail
            d["waited_ms"] = waited
            if !lastProblem.isEmpty { d["last"] = lastProblem }
            throw fail("timeout", "Waited \(waited) ms; \(until) did not happen.", d)
        }
        Thread.sleep(forTimeInterval: interval)
        interval = min(interval * 1.5, 0.3)
    }
}

// MARK: - Dispatch

let OPERATIONS = ["apps", "windows", "observe", "resolve", "press", "set_value", "focus", "menu", "type",
                  "key", "screenshot", "click_at", "clipboard", "wait", "window_state", "cancel", "shutdown"]

func handle(_ req: [String: Any]) {
    let id = req["id"] as? Int ?? -1
    let op = req["op"] as? String ?? ""
    do {
        let result: [String: Any]
        switch op {
        case "hello":
            if let first = req["first_observation"] as? Int {
                stateLock.lock(); nextObservation = max(1, first); stateLock.unlock()
            }
            if req["simulate_untrusted"] as? Bool == true { simulateUntrusted = true }
            result = [
                "manifest": [
                    "name": "hame-desktop-helper",
                    "version": HELPER_VERSION,
                    "protocol": req["protocol"] as? Int ?? 0,
                    "channel": "desktop",
                    "operations": OPERATIONS,
                    "platforms": ["macos"],
                    "cancellable": true,
                    "nested_usage": "none",
                ],
                "helper_protocol": HELPER_PROTOCOL,
                "ax_trusted": axTrusted(),
                "screen_access": CGPreflightScreenCaptureAccess(),
                "pid": Int(getpid()),
                "os": ProcessInfo.processInfo.operatingSystemVersionString,
            ]
        case "apps":
            let apps = NSWorkspace.shared.runningApplications.filter {
                $0.activationPolicy == .regular && !$0.isTerminated
            }
            result = ["apps": apps.map(appJSON)]
        case "windows":
            try requireTrusted()
            let app = try resolveApp(req)
            let appEl = appElement(app.processIdentifier)
            result = ["windows": axWindows(appEl).compactMap { windowJSON($0, app: app, appEl: appEl) }]
        case "window_state":
            let (app, appEl, window, _) = try resolveWindow(req)
            result = ["window": windowJSON(window, app: app, appEl: appEl) ?? [:],
                      "frontmost_pid": Int(frontmostPid() ?? 0)]
        case "observe": result = try observe(req)
        case "resolve": result = elementJSON(try resolveElement(req))
        case "press": result = try press(req)
        case "set_value": result = try setValue(req)
        case "focus": result = try focus(req)
        case "menu": result = try menuItem(req)
        case "type": result = try typeText(req, id: id)
        case "key": result = try pressKey(req, id: id)
        case "screenshot": result = try screenshot(req)
        case "click_at": result = try clickAt(req)
        case "clipboard": result = try clipboard(req)
        case "wait": result = try wait(req, id: id)
        default: throw fail("invalid", "Unknown operation \(op).")
        }
        emit(["id": id, "ok": true, "result": result])
    } catch let e as HelperError {
        emit(["id": id, "ok": false, "error": ["code": e.code, "message": e.message, "details": e.details]])
    } catch {
        emit(["id": id, "ok": false, "error": ["code": "failed", "message": "\(error)", "details": [:] as [String: Any]]])
    }
    stateLock.lock()
    cancelled.remove(id)
    stateLock.unlock()
}

func cleanupAndExit(_ code: Int32) -> Never {
    releaseModifiers()
    restoreClipboard()
    exit(code)
}

var signalSources: [DispatchSourceSignal] = []
for sig in [SIGTERM, SIGINT, SIGHUP] {
    signal(sig, SIG_IGN)
    let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
    source.setEventHandler { cleanupAndExit(0) }
    source.resume()
    signalSources.append(source)
}

let work = DispatchQueue(label: "hame.desktop.work")

let reader = Thread {
    while let line = readLine(strippingNewline: true) {
        guard let data = line.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            emit(["id": -1, "ok": false, "error": ["code": "invalid", "message": "unparseable request"]])
            continue
        }
        let op = obj["op"] as? String ?? ""
        if op == "cancel" {
            if let target = obj["target"] as? Int {
                stateLock.lock(); cancelled.insert(target); stateLock.unlock()
            }
            continue
        }
        if op == "shutdown" {
            work.sync {}
            DispatchQueue.main.sync { cleanupAndExit(0) }
        }
        work.async { handle(obj) }
    }
    // stdin closed: the parent is gone. Let queued work finish, then clean up.
    work.sync {}
    DispatchQueue.main.async { cleanupAndExit(0) }
}
reader.start()
RunLoop.main.run()
