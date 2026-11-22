// Compiles a captured vs_main MSL and one fragment entry's MSL for the
// discrete GPU through MTLBinaryArchive and serializes the archive, whose GPU
// slice carries each packed function's register counts (`regs.py` reads them).
//
// usage: screen <dir> <fragment entry>
//   reads <dir>/vs_main.metal and <dir>/<fragment entry>.metal, and the
//   vertex streams from <dir>/vertex_layout when present (else the mover's),
//   writes <dir>/<fragment entry>.metallib
import Foundation
import Metal

guard CommandLine.arguments.count >= 3 else {
    FileHandle.standardError.write("usage: screen <capture dir> <fragment entry>\n".data(using: .utf8)!)
    exit(2)
}
let dir = CommandLine.arguments[1]
let fragment = CommandLine.arguments[2]
guard let device = MTLCopyAllDevices().first(where: { !$0.isLowPower }) else {
    FileHandle.standardError.write("no discrete Metal device\n".data(using: .utf8)!)
    exit(1)
}

func library(_ name: String) throws -> MTLLibrary {
    let source = try String(contentsOfFile: "\(dir)/\(name).metal", encoding: .utf8)
    let options = MTLCompileOptions()
    options.languageVersion = .version4_0
    options.preserveInvariance = true
    return try device.makeLibrary(source: source, options: options)
}

let descriptor = MTLRenderPipelineDescriptor()
descriptor.vertexFunction = try library("vs_main").makeFunction(name: "vs_main")
descriptor.fragmentFunction = try library(fragment).makeFunction(name: fragment)
descriptor.colorAttachments[0].pixelFormat = .rgba16Float
descriptor.depthAttachmentPixelFormat = .depth32Float
let vertex = MTLVertexDescriptor()
let layoutPath = "\(dir)/vertex_layout"
if let layout = try? String(contentsOfFile: layoutPath, encoding: .utf8) {
    // A capture's own vertex streams, one line each:
    //   layout <buffer> <stride>
    //   attribute <location> <float2|float3|uint> <offset> <buffer>
    let formats: [String: MTLVertexFormat] = ["float2": .float2, "float3": .float3, "uint": .uint]
    for line in layout.split(separator: "\n") {
        let field = line.split(separator: " ").map(String.init)
        if field.first == "layout", field.count == 3 {
            vertex.layouts[Int(field[1])!].stride = Int(field[2])!
        } else if field.first == "attribute", field.count == 5, let format = formats[field[2]] {
            let location = Int(field[1])!
            vertex.attributes[location].format = format
            vertex.attributes[location].offset = Int(field[3])!
            vertex.attributes[location].bufferIndex = Int(field[4])!
        } else {
            FileHandle.standardError.write("bad vertex_layout line: \(line)\n".data(using: .utf8)!)
            exit(1)
        }
    }
} else {
    // The mover's vertex stream: position, uv, packed normal, packed tangent.
    let attributes: [(MTLVertexFormat, Int)] = [(.float3, 0), (.float2, 12), (.ushort2, 20), (.ushort2, 24)]
    for (i, (format, offset)) in attributes.enumerated() {
        vertex.attributes[i].format = format
        vertex.attributes[i].offset = offset
        vertex.attributes[i].bufferIndex = 30
    }
    vertex.layouts[30].stride = 36
}
descriptor.vertexDescriptor = vertex

let archive = try device.makeBinaryArchive(descriptor: MTLBinaryArchiveDescriptor())
try archive.addRenderPipelineFunctions(descriptor: descriptor)
try archive.serialize(to: URL(fileURLWithPath: "\(dir)/\(fragment).metallib"))
print("device: \(device.name)")
