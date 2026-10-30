// Prints the on-screen-or-not window ids (CGWindowID, the id AeroSpace
// and `screencapture -l` use) of the process given, one per line.
import CoreGraphics
import Foundation

guard CommandLine.arguments.count > 1, let pid = Int32(CommandLine.arguments[1]) else { exit(2) }
let windows = CGWindowListCopyWindowInfo([.optionAll], kCGNullWindowID) as? [[String: Any]] ?? []
for w in windows where (w[kCGWindowOwnerPID as String] as? Int32) == pid && (w[kCGWindowLayer as String] as? Int) == 0 {
    if let id = w[kCGWindowNumber as String] as? Int { print(id) }
}
