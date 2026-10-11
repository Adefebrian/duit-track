// Renders the Duitrack app icon (blue squircle, two wave strokes) to a 1024px PNG.
import AppKit

let size: CGFloat = 1024
let img = NSImage(size: NSSize(width: size, height: size))
img.lockFocus()
let inset: CGFloat = 100
let rect = NSRect(x: inset, y: inset, width: size - inset * 2, height: size - inset * 2)
NSColor(srgbRed: 0x0A / 255, green: 0x64 / 255, blue: 0xF0 / 255, alpha: 1).setFill()
NSBezierPath(roundedRect: rect, xRadius: 185, yRadius: 185).fill()
NSColor.white.setStroke()
for dy: CGFloat in [-95, 95] {
    let p = NSBezierPath()
    p.lineWidth = 62
    p.lineCapStyle = .round
    let y = size / 2 + dy
    p.move(to: NSPoint(x: 270, y: y))
    p.curve(to: NSPoint(x: 512, y: y), controlPoint1: NSPoint(x: 350, y: y + 90), controlPoint2: NSPoint(x: 432, y: y + 90))
    p.curve(to: NSPoint(x: 754, y: y), controlPoint1: NSPoint(x: 592, y: y - 90), controlPoint2: NSPoint(x: 674, y: y - 90))
    p.stroke()
}
img.unlockFocus()
let rep = NSBitmapImageRep(data: img.tiffRepresentation!)!
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
