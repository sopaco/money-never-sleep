// Generates every icon artifact the app ships from `assets/app_icon.png`.
//
// Run from the repository root:  swift scripts/make-icons.swift
//
// Outputs:
//   assets/AppIcon.icns        macOS app bundle icon
//   assets/app.ico             Windows executable icon
//   assets/icon-512.png        store/marketing render
//   assets/icon-128.png        in-app brand mark at 44pt (embedded, needs 2x)
//   assets/tray-22.rgba        macOS menu bar bitmap (22pt, raw RGBA8)
//   assets/tray-32.rgba        Windows/Linux tray bitmap (32px, raw RGBA8)

import AppKit
import Foundation

let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let source = root.appendingPathComponent("assets/app_icon.png")
let assets = root.appendingPathComponent("assets")

guard let image = NSImage(contentsOf: source),
      let tiff = image.tiffRepresentation,
      let bitmap = NSBitmapImageRep(data: tiff),
      let cgSource = bitmap.cgImage else {
    fatalError("cannot read \(source.path)")
}

// ── Load into a known 8-bit RGBA buffer ────────────────────────────────
let width = cgSource.width
let height = cgSource.height
var pixels = [UInt8](repeating: 0, count: width * height * 4)
guard let readContext = CGContext(
    data: &pixels,
    width: width,
    height: height,
    bitsPerComponent: 8,
    bytesPerRow: width * 4,
    space: CGColorSpace(name: CGColorSpace.sRGB)!,
    bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else { fatalError("cannot allocate read context") }
readContext.draw(cgSource, in: CGRect(x: 0, y: 0, width: width, height: height))

// ── Trim the white matte around the artwork ────────────────────────────
var minX = width, maxX = 0, minY = height, maxY = 0
for y in 0..<height {
    for x in 0..<width {
        let offset = (y * width + x) * 4
        let r = Int(pixels[offset])
        let g = Int(pixels[offset + 1])
        let b = Int(pixels[offset + 2])
        let a = Int(pixels[offset + 3])
        // Anything meaningfully darker than paper counts as artwork. The
        // source is exported without alpha, so white is the matte.
        let isInk = a > 8 && (r < 246 || g < 246 || b < 246)
        if isInk {
            minX = min(minX, x); maxX = max(maxX, x)
            minY = min(minY, y); maxY = max(maxY, y)
        }
    }
}
guard maxX > minX, maxY > minY else { fatalError("the source icon looks blank") }

// Square crop centred on the artwork, then a hair of breathing room so the
// rounded mask never clips a stroke.
let side = max(maxX - minX, maxY - minY) + 1
let centreX = (minX + maxX) / 2
let centreY = (minY + maxY) / 2
let cropRect = CGRect(
    x: CGFloat(centreX) - CGFloat(side) / 2,
    y: CGFloat(centreY) - CGFloat(side) / 2,
    width: CGFloat(side),
    height: CGFloat(side)
)
guard let cropped = cgSource.cropping(to: cropRect) else { fatalError("cannot crop artwork") }

// ── Render a square icon at a given pixel size ─────────────────────────
/// `masked` rounds the corners the way macOS and Windows 11 draw app tiles.
func render(size: Int, masked: Bool, inset: CGFloat = 0.04) -> CGImage {
    guard let context = CGContext(
        data: nil,
        width: size,
        height: size,
        bitsPerComponent: 8,
        bytesPerRow: 0,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { fatalError("cannot allocate render context") }
    context.interpolationQuality = .high
    context.clear(CGRect(x: 0, y: 0, width: size, height: size))
    context.saveGState()
    let margin = CGFloat(size) * inset
    let box = CGRect(x: margin, y: margin, width: CGFloat(size) - margin * 2, height: CGFloat(size) - margin * 2)
    if masked {
        // 22.37% is the corner radius Apple's app tiles read as "squircle"
        // at small sizes; Windows 11 sits within a point of it.
        let radius = box.width * 0.2237
        let path = CGPath(
            roundedRect: box,
            cornerWidth: radius,
            cornerHeight: radius,
            transform: nil
        )
        context.addPath(path)
        context.clip()
    }
    context.draw(cropped, in: box)
    context.restoreGState()
    guard let result = context.makeImage() else { fatalError("cannot rasterize icon") }
    return result
}

func writePNG(_ image: CGImage, to url: URL) {
    let rep = NSBitmapImageRep(cgImage: image)
    guard let data = rep.representation(using: .png, properties: [:]) else {
        fatalError("cannot encode \(url.lastPathComponent)")
    }
    try! data.write(to: url)
}

func writeRGBA(_ image: CGImage, to url: URL) {
    let w = image.width, h = image.height
    var buffer = [UInt8](repeating: 0, count: w * h * 4)
    guard let context = CGContext(
        data: &buffer,
        width: w,
        height: h,
        bitsPerComponent: 8,
        bytesPerRow: w * 4,
        space: CGColorSpace(name: CGColorSpace.sRGB)!,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
    ) else { fatalError("cannot allocate raw context") }
    context.draw(image, in: CGRect(x: 0, y: 0, width: w, height: h))
    // Premultiplied RGBA is what both tray backends expect; round-trip the
    // alpha so half-transparent edge pixels are not double-darkened.
    var raw = [UInt8](repeating: 0, count: w * h * 4)
    for index in stride(from: 0, to: buffer.count, by: 4) {
        let a = Int(buffer[index + 3])
        if a == 0 { continue }
        raw[index] = UInt8(min(255, Int(buffer[index]) * 255 / a))
        raw[index + 1] = UInt8(min(255, Int(buffer[index + 1]) * 255 / a))
        raw[index + 2] = UInt8(min(255, Int(buffer[index + 2]) * 255 / a))
        raw[index + 3] = UInt8(a)
    }
    try! Data(raw).write(to: url)
}

// ── PNG renditions ─────────────────────────────────────────────────────
writePNG(render(size: 512, masked: true), to: assets.appendingPathComponent("icon-512.png"))
writePNG(render(size: 128, masked: true), to: assets.appendingPathComponent("icon-128.png"))

// ── macOS .icns ────────────────────────────────────────────────────────
let iconset = root.appendingPathComponent("target/AppIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
let icnsSizes: [(Int, String)] = [
    (16, "icon_16x16"), (32, "icon_16x16@2x"),
    (32, "icon_32x32"), (64, "icon_32x32@2x"),
    (128, "icon_128x128"), (256, "icon_128x128@2x"),
    (256, "icon_256x256"), (512, "icon_256x256@2x"),
    (512, "icon_512x512"), (1024, "icon_512x512@2x"),
]
for (size, name) in icnsSizes {
    writePNG(render(size: size, masked: true), to: iconset.appendingPathComponent("\(name).png"))
}
let iconutil = Process()
iconutil.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
iconutil.arguments = ["-c", "icns", iconset.path, "-o", assets.appendingPathComponent("AppIcon.icns").path]
try! iconutil.run()
iconutil.waitUntilExit()

// ── Windows .ico (PNG-compressed payloads, Vista and later) ────────────
let icoSizes = [256, 128, 64, 48, 32, 16]
let payloads = icoSizes.map { size -> Data in
    let rep = NSBitmapImageRep(cgImage: render(size: size, masked: true))
    return rep.representation(using: .png, properties: [:])!
}
var ico = Data()
ico.append(contentsOf: [0, 0])                 // reserved
ico.append(contentsOf: [1, 0])                 // type: icon
ico.append(contentsOf: [UInt8(payloads.count), 0])
var offset = 6 + payloads.count * 16
for (index, size) in icoSizes.enumerated() {
    let dimension = size >= 256 ? UInt8(0) : UInt8(size)
    ico.append(contentsOf: [dimension, dimension, 0, 0])
    ico.append(contentsOf: [1, 0])             // colour planes
    ico.append(contentsOf: [32, 0])            // bits per pixel
    let length = UInt32(payloads[index].count)
    ico.append(contentsOf: withUnsafeBytes(of: length.littleEndian) { Array($0) })
    ico.append(contentsOf: withUnsafeBytes(of: UInt32(offset).littleEndian) { Array($0) })
    offset += payloads[index].count
}
for payload in payloads { ico.append(payload) }
try! ico.write(to: assets.appendingPathComponent("app.ico"))

// ── Tray bitmaps ───────────────────────────────────────────────────────
writeRGBA(render(size: 22, masked: false), to: assets.appendingPathComponent("tray-22.rgba"))
writeRGBA(render(size: 32, masked: false), to: assets.appendingPathComponent("tray-32.rgba"))

print("icons written: icns, ico, 512/128 png, tray 22/32 rgba")
