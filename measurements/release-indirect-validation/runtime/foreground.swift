import AppKit
import CoreGraphics
let args = CommandLine.arguments
let pid = pid_t(args[1])!
if args.count > 2 && args[2] == "activate" {
    if let app = NSRunningApplication(processIdentifier: pid) {
        print("activated=\(app.activate(options: [.activateIgnoringOtherApps]))")
    }
}
let front = NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
print("front_pid=\(front) target_pid=\(pid) foreground=\(front == pid)")
if let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] {
    for w in windows where (w[kCGWindowOwnerPID as String] as? Int) == Int(pid) {
        print("window_id=\(w[kCGWindowNumber as String] ?? "unknown") bounds=\(w[kCGWindowBounds as String] ?? "unknown")")
    }
}
