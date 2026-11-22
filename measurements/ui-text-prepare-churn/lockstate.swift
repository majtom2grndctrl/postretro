import CoreGraphics
if let d = CGSessionCopyCurrentDictionary() as? [String: Any] {
  print("locked=\(d["CGSSessionScreenIsLocked"] ?? "absent") onConsole=\(d["kCGSSessionOnConsoleKey"] ?? "?")")
}
