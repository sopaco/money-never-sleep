// Builds `assets/icon-dock-alert.png` — the Dock icon used while the butler
// daemon is unhealthy.
//
// Run from the repository root:  swift scripts/make-dock-alert.swift
//
// Why an icon instead of `NSDockTile.badgeLabel`: the text badge is delivered
// through a per-bundle Dock channel that can be stale on machines that have
// ran many copies of the app (DMG mounts, ad-hoc builds). Swapping the whole
// icon goes through `NSApplication.applicationIconImage`, which the Dock
// repaints reliably, and reads at small sizes because the red dot sits on the
// artwork instead of floating above it.

import AppKit
import Foundation

let root = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
let source = root.appendingPathComponent("assets/icon-512.png")
let target = root.appendingPathComponent("assets/icon-dock-alert.png")
let side = 512

guard let image = NSImage(contentsOf: source) else {
    fatalError("cannot read \(source.path)")
}
guard let rep = NSBitmapImageRep(
    bitmapDataPlanes: nil,
    pixelsWide: side,
    pixelsHigh: side,
    bitsPerSample: 8,
    samplesPerPixel: 4,
    hasAlpha: true,
    isPlanar: false,
    colorSpaceName: .deviceRGB,
    bytesPerRow: 0,
    bitsPerPixel: 0
) else { fatalError("cannot allocate the icon canvas") }

NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let size = NSSize(width: side, height: side)
image.draw(in: NSRect(origin: .zero, size: size))

// Red dot with a white ring, tucked into the top-right corner of the artwork.
// Proportions match a macOS Dock badge so it does not read as a second icon.
let diameter = CGFloat(side) * 0.30
let inset = CGFloat(side) * 0.045
let dot = NSRect(
    x: CGFloat(side) - diameter - inset,
    y: CGFloat(side) - diameter - inset,
    width: diameter,
    height: diameter
)

NSColor.white.setFill()
NSBezierPath(ovalIn: dot.insetBy(dx: -CGFloat(side) * 0.022, dy: -CGFloat(side) * 0.022)).fill()

NSColor(srgbRed: 0.92, green: 0.26, blue: 0.21, alpha: 1.0).setFill()
NSBezierPath(ovalIn: dot).fill()
NSGraphicsContext.restoreGraphicsState()

guard let png = rep.representation(using: .png, properties: [:]) else {
    fatalError("cannot encode \(target.path)")
}
try! png.write(to: target)
print("wrote \(target.lastPathComponent) (\(side)x\(side))")
