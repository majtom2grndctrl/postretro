// The binding seam: a pluggable namespace the IR `bind` pass resolves names
// against and the eval pass reads/writes live state through.
// See: context/lib/scripting.md §11 (IR substrate — pluggable scope abstraction)

// Names bind through a scope, not a hardwired global namespace. The mod state
// store is one scope; a movement-local input set is another. The trait is the
// single seam: bind validates names and types against it (`resolve_input` /
// `resolve_output`), and eval moves values across it (`read` / `write`).

use super::{IrType, IrValue};

/// A resolved input: the eval-time handle the scope reads from, plus the
/// projected IR type bind type-checks against.
///
/// A store-backed scope projects `SlotValue::Number ↔ IrValue::Number` and
/// `SlotValue::Boolean ↔ IrValue::Bool`. `String`/`Enum`/`Array` slots have no
/// IR projection — the scope must return `None` from `resolve_input` for them,
/// so bind never sees a non-projectable type here.
pub(crate) struct ResolvedInput<H> {
    pub(crate) handle: H,
    pub(crate) ir_type: IrType,
}

/// A resolved output: the eval-time write handle the scope writes through, plus
/// the projected IR type the root's result type must match.
pub(crate) struct ResolvedOutput<H> {
    pub(crate) handle: H,
    pub(crate) ir_type: IrType,
}

/// The namespace an IR program binds and evaluates against.
///
/// # Contract
///
/// - **Handles are scope-defined.** `InputHandle` / `OutputHandle` are
///   associated types: a store scope carries an owned dotted name, a
///   movement/stub scope an index. Bind stores resolved handles (not names) in
///   the `BoundProgram`; eval dereferences them. Handles must remain valid for
///   the lifetime of the bound program against the scope they were resolved
///   from.
///
/// - **Resolution is type-projecting.** `resolve_input` / `resolve_output`
///   return both the handle and the name's projected IR type, so bind
///   type-checks without a second lookup. Only `number`/`boolean`-projectable
///   names may resolve; a name backed by a non-projectable slot
///   (`String`/`Enum`/`Array`) must return `None`, which bind reports as an
///   unknown name. (The projection failure is the scope's to enforce — bind
///   trusts that any returned `ir_type` is genuinely projectable.)
///
/// - **Writability is a bind-time capability.** A scope grants a write handle
///   *only* for outputs it permits. `resolve_output` returns `None` to deny —
///   either because the name is unknown or because this scope lacks write
///   capability for it (e.g. a script-capability scope facing a readonly
///   engine-owned slot). A read-only scope returns `None` for *every* output.
///   This mirrors the store's engine-bypass vs script-gated write split.
///
/// - **Eval is total.** `read` returns a value for any handle the scope
///   resolved; `write` accepts a value of the resolved output's projected
///   type. Bind guarantees the value type matches the handle's projection, so
///   eval implementations may assume the projection holds.
pub(crate) trait BindingScope {
    type InputHandle;
    type OutputHandle;

    /// Resolve an input name to a read handle and its projected IR type, or
    /// `None` if the name is unknown or backed by a non-projectable slot.
    fn resolve_input(&self, name: &str) -> Option<ResolvedInput<Self::InputHandle>>;

    /// Resolve an output name to a write handle and its projected IR type, or
    /// `None` if the name is unknown, non-projectable, or not writable by this
    /// scope.
    fn resolve_output(&self, name: &str) -> Option<ResolvedOutput<Self::OutputHandle>>;

    /// Read the live value behind a resolved input handle. Total: the handle
    /// came from a successful `resolve_input`, so a value always exists.
    fn read(&self, handle: &Self::InputHandle) -> IrValue;

    /// Write a value behind a resolved output handle. The value's type matches
    /// the handle's projected type (bind enforces this); implementations may
    /// assume it.
    fn write(&mut self, handle: &Self::OutputHandle, value: IrValue);
}
