// Test-only GPU probe for renderer-owned uniform bytes.
struct Words { values: array<vec4<u32>, 6>, }
@group(0) @binding(0) var<uniform> input: Words;
@group(0) @binding(1) var<storage, read_write> output: Words;
@compute @workgroup_size(1)
fn main() { output = input; }
