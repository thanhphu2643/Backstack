// Draws Resources/AppIcon.icns: a stack of three archive boxes on a slate tile.
// Run: swift scripts/make-icon.swift
import AppKit

func color(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> NSColor {
    NSColor(red: r, green: g, blue: b, alpha: a)
}

func render(_ size: Int) -> Data {
    let s = CGFloat(size) / 1024
    let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: size, pixelsHigh: size, bitsPerSample: 8,
                               samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                               bytesPerRow: 0, bitsPerPixel: 0)!
    NSGraphicsContext.saveGraphicsState()
    NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
    let ctx = NSGraphicsContext.current!.cgContext
    ctx.scaleBy(x: s, y: s)

    // Tile on Apple's icon grid: 824pt body, 100pt margin, soft drop shadow.
    let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
    let shape = NSBezierPath(roundedRect: tile, xRadius: 185, yRadius: 185)
    ctx.saveGState()
    ctx.setShadow(offset: CGSize(width: 0, height: -12), blur: 28, color: NSColor.black.withAlphaComponent(0.3).cgColor)
    NSColor.black.setFill()
    shape.fill()
    ctx.restoreGState()
    ctx.saveGState()
    shape.addClip()
    NSGradient(colors: [color(0.20, 0.29, 0.40), color(0.11, 0.16, 0.24)])!.draw(in: tile, angle: -90)

    // Three archive boxes, oldest at the bottom, each a little offset like a real stack.
    let boxes: [(y: CGFloat, dx: CGFloat, top: NSColor, front: NSColor)] = [
        (230, -14, color(0.62, 0.78, 0.86), color(0.42, 0.60, 0.70)),
        (410, 16, color(0.98, 0.78, 0.45), color(0.86, 0.58, 0.25)),
        (590, -6, color(0.99, 0.62, 0.50), color(0.88, 0.40, 0.30)),
    ]
    for b in boxes {
        let x = 262 + b.dx, w: CGFloat = 500, h: CGFloat = 150
        ctx.saveGState()
        ctx.setShadow(offset: CGSize(width: 0, height: -10), blur: 20, color: NSColor.black.withAlphaComponent(0.35).cgColor)
        let body = NSBezierPath(roundedRect: CGRect(x: x, y: b.y, width: w, height: h), xRadius: 26, yRadius: 26)
        b.front.setFill()
        body.fill()
        ctx.restoreGState()
        // Lid along the top of the box.
        let lid = NSBezierPath(roundedRect: CGRect(x: x - 14, y: b.y + h - 46, width: w + 28, height: 58), xRadius: 22, yRadius: 22)
        b.top.setFill()
        lid.fill()
        // Label plate with a handle slot.
        let plate = NSBezierPath(roundedRect: CGRect(x: x + w / 2 - 90, y: b.y + 26, width: 180, height: 56), xRadius: 14, yRadius: 14)
        color(1, 0.97, 0.90).setFill()
        plate.fill()
        let slot = NSBezierPath(roundedRect: CGRect(x: x + w / 2 - 50, y: b.y + 46, width: 100, height: 16), xRadius: 8, yRadius: 8)
        b.front.setFill()
        slot.fill()
    }
    ctx.restoreGState()

    NSGraphicsContext.restoreGraphicsState()
    return rep.representation(using: .png, properties: [:])!
}

let root = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent().deletingLastPathComponent()
let iconset = FileManager.default.temporaryDirectory.appendingPathComponent("BackstackIcon.iconset")
try? FileManager.default.removeItem(at: iconset)
try! FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
for base in [16, 32, 128, 256, 512] {
    try! render(base).write(to: iconset.appendingPathComponent("icon_\(base)x\(base).png"))
    try! render(base * 2).write(to: iconset.appendingPathComponent("icon_\(base)x\(base)@2x.png"))
}
let out = root.appendingPathComponent("Resources/AppIcon.icns")
let task = Process()
task.executableURL = URL(fileURLWithPath: "/usr/bin/iconutil")
task.arguments = ["-c", "icns", iconset.path, "-o", out.path]
try! task.run()
task.waitUntilExit()
try! render(1024).write(to: root.appendingPathComponent("Resources/AppIcon.png"))
print("Wrote \(out.path)")
