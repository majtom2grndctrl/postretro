// VM error adapters for descriptor converters. Kept outside `error.rs` so the
// error type itself remains VM-free in `postretro-foundation`.
// See: context/lib/scripting.md §12 (Crate Architecture)

use super::DescriptorError;

pub(crate) fn js_err(e: rquickjs::Error) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: e.to_string(),
    }
}

pub(crate) fn lua_err(e: mlua::Error) -> DescriptorError {
    DescriptorError::InvalidShape {
        reason: e.to_string(),
    }
}
