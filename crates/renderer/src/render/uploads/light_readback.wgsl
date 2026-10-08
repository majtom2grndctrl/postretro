// Test-only GPU probe for the first real light record.
struct Words { values: array<vec4<u32>, 4>, }
@group(0) @binding(0) var<storage, read> input: Words;
@group(0) @binding(1) var<storage, read_write> output: Words;
@compute @workgroup_size(1)
fn main() { output = input; }
