// Compiles a captured vs_main/fs_main MSL pair for the discrete GPU through
// MTLBinaryArchive and serializes the archive, whose GPU slice carries each
// packed function's register counts (`regs.py` reads them).
//
// usage: screen <dir>   (reads <dir>/vs_main.metal and <dir>/fs_main.metal,
//                         writes <dir>/pipeline.metallib)
import Foundation
import Metal

guard CommandLine.arguments.count >= 2 else {
    FileHandle.standardError.write("usage: screen <capture dir>\n".data(using: .utf8)!)
    exit(2)
}
let dir = CommandLine.arguments[1]
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
descriptor.fragmentFunction = try library("fs_main").makeFunction(name: "fs_main")
descriptor.colorAttachments[0].pixelFormat = .rgba16Float
descriptor.depthAttachmentPixelFormat = .depth32Float
// The mover's vertex stream: position, uv, packed normal, packed tangent.
let vertex = MTLVertexDescriptor()
let attributes: [(MTLVertexFormat, Int)] = [(.float3, 0), (.float2, 12), (.ushort2, 20), (.ushort2, 24)]
for (i, (format, offset)) in attributes.enumerated() {
    vertex.attributes[i].format = format
    vertex.attributes[i].offset = offset
    vertex.attributes[i].bufferIndex = 30
}
vertex.layouts[30].stride = 36
descriptor.vertexDescriptor = vertex

let archive = try device.makeBinaryArchive(descriptor: MTLBinaryArchiveDescriptor())
try archive.addRenderPipelineFunctions(descriptor: descriptor)
try archive.serialize(to: URL(fileURLWithPath: "\(dir)/pipeline.metallib"))
print("device: \(device.name)")
