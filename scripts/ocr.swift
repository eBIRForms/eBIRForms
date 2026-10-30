// Prints the text Vision reads in each image given, one line per text run,
// prefixed by the file name: what scripts/evidence.sh checks a screenshot
// against ("the screen matches its name").
import Foundation
import Vision
import AppKit

for path in CommandLine.arguments.dropFirst() {
    guard let image = NSImage(contentsOfFile: path),
          let cg = image.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
        print("\(path)\t<unreadable>")
        continue
    }
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    try? VNImageRequestHandler(cgImage: cg, options: [:]).perform([request])
    for observation in request.results ?? [] {
        if let line = observation.topCandidates(1).first?.string {
            print("\(path)\t\(line)")
        }
    }
}
