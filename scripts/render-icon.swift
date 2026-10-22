#!/usr/bin/env swift
// Render the small filled-shape SVG subset used by Buckets without external tools.
// Run from the repository root: swift scripts/render-icon.swift
import Foundation
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

struct Shape {
    let path: CGPath
    let color: CGColor
    let evenOdd: Bool
}

func color(_ hex: String) -> CGColor {
    let rgb = UInt32(hex.dropFirst(), radix: 16)!
    return CGColor(red: CGFloat((rgb >> 16) & 255) / 255,
                   green: CGFloat((rgb >> 8) & 255) / 255,
                   blue: CGFloat(rgb & 255) / 255, alpha: 1)
}

func path(_ source: String) -> CGPath {
    let expression = try! NSRegularExpression(pattern: "[A-Za-z]|[-+]?(?:[0-9]*\\.)?[0-9]+")
    let tokens = expression.matches(in: source, range: NSRange(source.startIndex..., in: source))
        .map { String(source[Range($0.range, in: source)!]) }
    let result = CGMutablePath()
    var i = 0
    func number() -> CGFloat { defer { i += 1 }; return CGFloat(Double(tokens[i])!) }
    func point() -> CGPoint { CGPoint(x: number(), y: number()) }
    while i < tokens.count {
        let command = tokens[i]
        i += 1
        switch command {
        case "M": result.move(to: point())
        case "L": result.addLine(to: point())
        case "H": result.addLine(to: CGPoint(x: number(), y: result.currentPoint.y))
        case "V": result.addLine(to: CGPoint(x: result.currentPoint.x, y: number()))
        case "C":
            let c1 = point(), c2 = point(), end = point()
            result.addCurve(to: end, control1: c1, control2: c2)
        case "Q":
            let control = point(), end = point()
            result.addQuadCurve(to: end, control: control)
        case "Z": result.closeSubpath()
        default: fatalError("Unsupported SVG path command: \(command)")
        }
    }
    return result
}

final class SVGReader: NSObject, XMLParserDelegate {
    var shapes = [Shape]()
    func parser(_ parser: XMLParser, didStartElement name: String,
                namespaceURI: String?, qualifiedName: String?, attributes: [String: String]) {
        guard let fill = attributes["fill"] else { return }
        let shape: CGPath
        if name == "rect" {
            func n(_ key: String) -> CGFloat { CGFloat(Double(attributes[key] ?? "0")!) }
            let rect = CGRect(x: n("x"), y: n("y"), width: n("width"), height: n("height"))
            shape = CGPath(roundedRect: rect, cornerWidth: n("rx"), cornerHeight: n("rx"), transform: nil)
        } else if name == "path" {
            shape = path(attributes["d"]!)
        } else { return }
        shapes.append(Shape(path: shape, color: color(fill), evenOdd: attributes["fill-rule"] == "evenodd"))
    }
}

let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let source = root.appendingPathComponent("crates/buckets-app/assets/icon/buckets.svg")
let reader = SVGReader()
let parser = XMLParser(contentsOf: source)!
parser.delegate = reader
precondition(parser.parse(), "Invalid icon SVG")
precondition(reader.shapes.count == 5, "Expected all five icon shapes")
let target = root.appendingPathComponent("target", isDirectory: true)
try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true)
let iconset = target.appendingPathComponent("Buckets.iconset", isDirectory: true)
try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)

func render(_ size: Int) -> Data {
    let space = CGColorSpace(name: CGColorSpace.sRGB)!
    let bitmap = CGContext(data: nil, width: size, height: size, bitsPerComponent: 8,
                          bytesPerRow: size * 4, space: space,
                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    bitmap.translateBy(x: 0, y: CGFloat(size))
    bitmap.scaleBy(x: CGFloat(size) / 1024, y: -CGFloat(size) / 1024)
    for shape in reader.shapes {
        bitmap.addPath(shape.path)
        bitmap.setFillColor(shape.color)
        bitmap.drawPath(using: shape.evenOdd ? .eoFill : .fill)
    }
    let data = NSMutableData()
    let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, bitmap.makeImage()!, nil)
    precondition(CGImageDestinationFinalize(destination))
    return data as Data
}

let entries: [(String, Int, String)] = [
    ("icon_16x16.png", 16, "icp4"), ("icon_16x16@2x.png", 32, "ic11"),
    ("icon_32x32.png", 32, "icp5"), ("icon_32x32@2x.png", 64, "ic12"),
    ("icon_128x128.png", 128, "ic07"), ("icon_128x128@2x.png", 256, "ic13"),
    ("icon_256x256.png", 256, "ic08"), ("icon_256x256@2x.png", 512, "ic14"),
    ("icon_512x512.png", 512, "ic09"), ("icon_512x512@2x.png", 1024, "ic10")
]
var rendered = [Int: Data]()
func bigEndian(_ value: Int) -> Data {
    var value = UInt32(value).bigEndian
    return withUnsafeBytes(of: &value) { Data($0) }
}
var chunks = Data()
for (filename, size, type) in entries {
    let png = rendered[size] ?? render(size)
    rendered[size] = png
    try png.write(to: iconset.appendingPathComponent(filename))
    chunks.append(type.data(using: .ascii)!)
    chunks.append(bigEndian(png.count + 8))
    chunks.append(png)
}
// ICNS PNG chunks are lossless and retain the transparent rounded-square margins.
var icns = "icns".data(using: .ascii)!
icns.append(bigEndian(chunks.count + 8))
icns.append(chunks)
try icns.write(to: root.appendingPathComponent("crates/buckets-app/assets/icon/Buckets.icns"))
for size in [16, 32, 64, 256] {
    try rendered[size]!.write(to: target.appendingPathComponent("icon-preview-\(size).png"))
}
print("Rendered Buckets.icns, Buckets.iconset, and 16/32/64/256px previews")
