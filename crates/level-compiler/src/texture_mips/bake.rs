//! Per-slot mip-chain builders kept separate from cache orchestration.

pub(super) fn build_diffuse_chain(
    rgba: &[u8],
    width: u32,
    height: u32,
    lut: &[f32; 256],
) -> Vec<u8> {
    super::build_diffuse_chain_impl(rgba, width, height, lut)
}

pub(super) fn build_specular_chain(r8: &[u8], width: u32, height: u32) -> Vec<u8> {
    super::build_specular_chain_impl(r8, width, height)
}

pub(super) fn build_normal_bc5_chain(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    super::build_normal_bc5_chain_impl(rgba, width, height)
}
