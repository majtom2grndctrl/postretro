// Primitive binding layer: the "one registry" that drives both QuickJS and Luau.
// See: context/lib/scripting.md §4

use std::any::type_name;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use super::error::ScriptError;

/// Where a primitive is legal to call. Registering a `DefinitionOnly`
/// primitive into a behavior context installs the *stub* installer, which
/// unconditionally returns `ScriptError::WrongContext` to script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextScope {
    DefinitionOnly,
    BehaviorOnly,
    Both,
}

/// Name and type spelling for a single primitive parameter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParamInfo {
    pub(crate) name: &'static str,
    pub(crate) ty_name: &'static str,
}

/// Full signature of a registered primitive. `return_ty_name` is the
/// fully-qualified Rust type name; `params` are set by `.param()` calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrimitiveSignature {
    pub(crate) params: Vec<ParamInfo>,
    pub(crate) return_ty_name: &'static str,
}

/// Installer closure type aliases. Both are deliberately NOT `Send + Sync`:
/// they capture `ScriptCtx`, which holds `Rc<RefCell<_>>` (see
/// `scripting::ctx` for the rationale). Scripting is strictly single-threaded.
pub(crate) type QuickJsInstaller =
    Arc<dyn for<'js> Fn(&rquickjs::Ctx<'js>) -> rquickjs::Result<()>>;
pub(crate) type LuauInstaller = Arc<dyn Fn(&mlua::Lua) -> mlua::Result<()>>;

/// A single registered primitive. Clones are cheap — the closures are
/// behind `Arc`, the metadata is plain data.
#[derive(Clone)]
pub(crate) struct ScriptPrimitive {
    pub(crate) name: &'static str,
    pub(crate) doc: &'static str,
    pub(crate) signature: PrimitiveSignature,
    pub(crate) context_scope: ContextScope,
    pub(crate) quickjs_installer: QuickJsInstaller,
    pub(crate) luau_installer: LuauInstaller,
    pub(crate) quickjs_stub_installer: QuickJsInstaller,
    pub(crate) luau_stub_installer: LuauInstaller,
}

impl std::fmt::Debug for ScriptPrimitive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptPrimitive")
            .field("name", &self.name)
            .field("doc", &self.doc)
            .field("signature", &self.signature)
            .field("context_scope", &self.context_scope)
            .finish_non_exhaustive()
    }
}

/// Sealing: the trait lives in a private module and the impl body is only
/// populated by the `impl_registerable!` macro below. Downstream code cannot
/// add new argument shapes because the trait is `pub(crate)` and the macro
/// invocation list is the one source of truth.
pub(crate) trait RegisterablePrimitive<Args> {
    /// Wrap `self` into installer closures and return a fully-populated
    /// `ScriptPrimitive`. The real installers call the user function; the
    /// stub installers throw `ScriptError::WrongContext`.
    fn into_primitive(
        self,
        name: &'static str,
        scope: ContextScope,
        doc: &'static str,
    ) -> ScriptPrimitive;
}

/// The registry. Built at engine startup via `.register(...)` calls; runtime
/// init iterates with [`PrimitiveRegistry::iter`] to install each primitive.
#[derive(Default)]
pub(crate) struct PrimitiveRegistry {
    entries: Vec<ScriptPrimitive>,
}

impl PrimitiveRegistry {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Start a builder for a new primitive. The builder's `.finish()` pushes
    /// the resulting `ScriptPrimitive` into this registry.
    ///
    /// # Compile-time return-type enforcement
    ///
    /// `F: RegisterablePrimitive<Args>` is only satisfied when `F` returns
    /// `Result<T, ScriptError>` for some `T` that can cross the FFI boundary.
    /// Primitives returning a bare `T` or a `Result<T, OtherError>` fail to
    /// resolve the trait at the call site:
    ///
    /// ```compile_fail
    /// # use postretro::scripting::primitives_registry::{PrimitiveRegistry, ContextScope};
    /// let mut r = PrimitiveRegistry::new();
    /// // Returns bare `u32` instead of `Result<u32, ScriptError>` — rejected.
    /// r.register("bad", |x: u32| -> u32 { x }).scope(ContextScope::Both).finish();
    /// ```
    ///
    /// ```compile_fail
    /// # use postretro::scripting::primitives_registry::{PrimitiveRegistry, ContextScope};
    /// let mut r = PrimitiveRegistry::new();
    /// // Wrong error type — `Result<_, String>` is rejected; only `ScriptError`.
    /// r.register("bad", |x: u32| -> Result<u32, String> { Ok(x) })
    ///     .scope(ContextScope::Both)
    ///     .finish();
    /// ```
    ///
    /// Non-zero-arity primitives must also chain one `.param(name, ty_name)`
    /// call per argument before `.finish()`. See [`PrimitiveBuilder::param`].
    pub(crate) fn register<F, Args>(
        &mut self,
        name: &'static str,
        f: F,
    ) -> PrimitiveBuilder<'_, F, Args>
    where
        F: RegisterablePrimitive<Args>,
    {
        PrimitiveBuilder {
            registry: self,
            name,
            scope: ContextScope::Both,
            doc: "",
            f: Some(f),
            params: Vec::new(),
            _args: std::marker::PhantomData,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ScriptPrimitive> {
        self.entries.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Builder returned from [`PrimitiveRegistry::register`]. `.finish()` is the
/// only sink — dropping the builder without calling `.finish()` does nothing
/// (the registry entry is not inserted).
pub(crate) struct PrimitiveBuilder<'r, F, Args>
where
    F: RegisterablePrimitive<Args>,
{
    registry: &'r mut PrimitiveRegistry,
    name: &'static str,
    scope: ContextScope,
    doc: &'static str,
    f: Option<F>,
    params: Vec<ParamInfo>,
    _args: std::marker::PhantomData<fn() -> Args>,
}

impl<'r, F, Args> PrimitiveBuilder<'r, F, Args>
where
    F: RegisterablePrimitive<Args>,
{
    pub(crate) fn scope(mut self, scope: ContextScope) -> Self {
        self.scope = scope;
        self
    }

    pub(crate) fn doc(mut self, doc: &'static str) -> Self {
        self.doc = doc;
        self
    }

    /// Supply a real parameter name and short type spelling for the next
    /// parameter slot. Must be called exactly once per closure argument, in
    /// order. Zero-arity primitives must not call this method. The values feed
    /// generated `.d.ts` / `.d.luau` output. See: context/lib/scripting.md §4.
    pub(crate) fn param(mut self, name: &'static str, ty_name: &'static str) -> Self {
        self.params.push(ParamInfo { name, ty_name });
        self
    }

    pub(crate) fn finish(mut self) {
        let f = self
            .f
            .take()
            .expect("PrimitiveBuilder::finish called twice (internal)");
        let mut primitive = f.into_primitive(self.name, self.scope, self.doc);
        let expected = primitive.signature.params.len();
        // Invariant: one `.param()` call per closure argument. Symmetric check
        // covers all three failure shapes — too few, too many, and any call on
        // a zero-arity primitive (`expected == 0` ⇒ `self.params` must be empty).
        debug_assert_eq!(
            self.params.len(),
            expected,
            "primitive `{}` requires {} .param() call(s) but received {}",
            self.name,
            expected,
            self.params.len()
        );
        if !self.params.is_empty() {
            primitive.signature.params = self.params;
        }
        self.registry.entries.push(primitive);
    }
}

// ---------------------------------------------------------------------------
// Stub installers — used when a primitive is prohibited in the target context.
//
// Both runtimes surface `ScriptError::WrongContext` as their native error so
// scripts see a catchable exception / error with a clear message. The stub
// installers bind the *same name* so the primitive is never silently "undefined"
// in the wrong context.

fn make_quickjs_stub(name: &'static str, stub_context: &'static str) -> QuickJsInstaller {
    Arc::new(move |ctx: &rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
        let globals = ctx.globals();
        let f = rquickjs::Function::new(ctx.clone(), move |ctx: rquickjs::Ctx<'_>| {
            let err = ScriptError::WrongContext {
                primitive: name,
                current: stub_context,
            };
            Err::<rquickjs::Value, _>(
                rquickjs::Exception::from_message(ctx, &err.to_string())?.throw(),
            )
        })?;
        globals.set(name, f)?;
        Ok(())
    })
}

fn make_luau_stub(name: &'static str, stub_context: &'static str) -> LuauInstaller {
    Arc::new(move |lua: &mlua::Lua| -> mlua::Result<()> {
        let globals = lua.globals();
        let f = lua.create_function(move |_lua: &mlua::Lua, _args: mlua::MultiValue| {
            let err = ScriptError::WrongContext {
                primitive: name,
                current: stub_context,
            };
            Err::<mlua::Value, _>(mlua::Error::RuntimeError(err.to_string()))
        })?;
        globals.set(name, f)?;
        Ok(())
    })
}

// ---------------------------------------------------------------------------
// Per-arity RegisterablePrimitive impls, 0 through 6.
//
// rquickjs `IntoJsFunc` and mlua `FromLuaMulti`/`IntoLuaMulti` are both
// per-tuple-arity. mlua bounds the *tuple* `FromLuaMulti`, not each argument.
//
// Each expansion wraps the user function in `catch_unwind(AssertUnwindSafe(…))`
// and translates every failure into the runtime's native error type. We throw
// JS exceptions ourselves via `rquickjs::Exception::from_message` rather than
// relying on `IntoJs for Result`.

macro_rules! impl_registerable {
    ( $( ($ty:ident, $arg:ident) ),* ) => {
        // Sealed is parameterized by the arg-tuple so each arity is a distinct
        // impl — Rust does not permit two overlapping `Sealed` impls on `FnT`
        // without disambiguation. We key on the tuple of argument types and
        // the return type.
        impl<FnT, T, $( $ty ),*> RegisterablePrimitive<( $( $ty, )* )> for FnT
        where
            FnT: Fn( $( $ty ),* ) -> Result<T, ScriptError> + Clone + 'static,
            // Per-argument rquickjs FromJs bound: rquickjs decodes arguments
            // one by one, each via its own FromJs impl.
            $( $ty: for<'js> rquickjs::FromJs<'js> + 'static, )*
            // Tuple-level mlua bound: mlua::Lua::create_function wants the
            // *tuple* to implement FromLuaMulti, not each argument.
            ( $( $ty, )* ): mlua::FromLuaMulti,
            T: for<'js> rquickjs::IntoJs<'js> + mlua::IntoLuaMulti + 'static,
        {
            fn into_primitive(
                self,
                name: &'static str,
                scope: ContextScope,
                doc: &'static str,
            ) -> ScriptPrimitive {
                // `finish()` overwrites these with caller-supplied `.param()` values;
                // only the vec's length (arity) is load-bearing here.
                // `return_ty_name` is canonical — no builder override.
                let signature = PrimitiveSignature {
                    params: vec![
                        $( ParamInfo {
                            name: stringify!($arg),
                            ty_name: type_name::<$ty>(),
                        }, )*
                    ],
                    return_ty_name: type_name::<T>(),
                };

                let quickjs_installer: QuickJsInstaller = {
                    let user = self.clone();
                    Arc::new(move |ctx: &rquickjs::Ctx<'_>| -> rquickjs::Result<()> {
                        let globals = ctx.globals();
                        let user = user.clone();
                        let js_fn = rquickjs::Function::new(
                            ctx.clone(),
                            move |ctx: rquickjs::Ctx<'_>, $( $arg: $ty ),*| -> rquickjs::Result<T> {
                                let user = user.clone();
                                let result = catch_unwind(AssertUnwindSafe(|| {
                                    user( $( $arg ),* )
                                }));
                                match result {
                                    Ok(Ok(v)) => Ok(v),
                                    Ok(Err(e)) => Err(rquickjs::Exception::from_message(
                                        ctx.clone(),
                                        &e.to_string(),
                                    )?.throw()),
                                    Err(_) => {
                                        let err = ScriptError::Panicked { name };
                                        Err(rquickjs::Exception::from_message(
                                            ctx.clone(),
                                            &err.to_string(),
                                        )?.throw())
                                    }
                                }
                            },
                        )?;
                        globals.set(name, js_fn)?;
                        Ok(())
                    })
                };

                let luau_installer: LuauInstaller = {
                    let user = self.clone();
                    Arc::new(move |lua: &mlua::Lua| -> mlua::Result<()> {
                        let globals = lua.globals();
                        let user = user.clone();
                        let lua_fn = lua.create_function(
                            move |_lua: &mlua::Lua, ( $( $arg, )* ): ( $( $ty, )* )| -> mlua::Result<T> {
                                let user = user.clone();
                                let result = catch_unwind(AssertUnwindSafe(|| {
                                    user( $( $arg ),* )
                                }));
                                match result {
                                    Ok(Ok(v)) => Ok(v),
                                    Ok(Err(e)) => Err(mlua::Error::RuntimeError(e.to_string())),
                                    Err(_) => {
                                        let err = ScriptError::Panicked { name };
                                        Err(mlua::Error::RuntimeError(err.to_string()))
                                    }
                                }
                            },
                        )?;
                        globals.set(name, lua_fn)?;
                        Ok(())
                    })
                };

                // Stub reports the context it is installed into. `Both` never
                // has a stub invoked, but the type still requires a string.
                let stub_context: &'static str = match scope {
                    ContextScope::DefinitionOnly => "behavior",
                    ContextScope::BehaviorOnly => "definition",
                    ContextScope::Both => "behavior",
                };

                ScriptPrimitive {
                    name,
                    doc,
                    signature,
                    context_scope: scope,
                    quickjs_installer,
                    luau_installer,
                    quickjs_stub_installer: make_quickjs_stub(name, stub_context),
                    luau_stub_installer: make_luau_stub(name, stub_context),
                }
            }
        }
    };
}

// Arity expansion 0..=6. rquickjs 0.11's `FromParams` covers tuples up to 7
// elements; one slot is consumed by the `Ctx<'js>` extractor used to throw
// JS exceptions, leaving 6 user arguments. For wider signatures, pack
// arguments into a struct.
impl_registerable!();
impl_registerable!((A, a));
impl_registerable!((A, a), (B, b));
impl_registerable!((A, a), (B, b), (C, c));
impl_registerable!((A, a), (B, b), (C, c), (D, d));
impl_registerable!((A, a), (B, b), (C, c), (D, d), (E, e));
impl_registerable!((A, a), (B, b), (C, c), (D, d), (E, e), (F, f));

#[cfg(test)]
mod tests {
    use super::*;

    // Verifies metadata population and install/invoke flows.
    fn toy_double(x: u32) -> Result<u32, ScriptError> {
        Ok(x.wrapping_mul(2))
    }

    #[test]
    fn register_populates_script_primitive_record() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::Both)
            .doc("Doubles a u32.")
            .param("x", "u32")
            .finish();
        let primitives: Vec<_> = r.iter().collect();
        assert_eq!(primitives.len(), 1);
        let p = primitives[0];
        assert_eq!(p.name, "toy_double");
        assert_eq!(p.doc, "Doubles a u32.");
        assert_eq!(p.context_scope, ContextScope::Both);
        assert_eq!(p.signature.params.len(), 1);
        // Locks in the `.param()` override: name comes from the builder, not
        // from the `stringify!($arg)` placeholder the arity macro emits.
        assert_eq!(p.signature.params[0].name, "x");
        assert_eq!(p.signature.params[0].ty_name, "u32");
        assert_eq!(p.signature.return_ty_name, type_name::<u32>());
    }

    #[test]
    #[should_panic(expected = "primitive `toy_double` requires 1 .param() call(s) but received 0")]
    fn finish_panics_when_param_calls_missing() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::Both)
            .finish();
    }

    #[test]
    #[should_panic(expected = "primitive `toy_double` requires 1 .param() call(s) but received 2")]
    fn finish_panics_when_param_count_mismatches_arity() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::Both)
            .param("x", "u32")
            .param("y", "u32")
            .finish();
    }

    #[test]
    #[should_panic(expected = "primitive `toy_zero` requires 0 .param() call(s) but received 1")]
    fn finish_panics_when_zero_arity_calls_param() {
        fn toy_zero() -> Result<u32, ScriptError> {
            Ok(0)
        }
        let mut r = PrimitiveRegistry::new();
        r.register("toy_zero", toy_zero)
            .scope(ContextScope::Both)
            .param("x", "u32")
            .finish();
    }

    #[test]
    fn quickjs_installer_invokes_user_function_and_returns_value() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::Both)
            .param("x", "u32")
            .finish();

        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&ctx).unwrap();
            }
            let got: u32 = ctx.eval("toy_double(21)").unwrap();
            assert_eq!(got, 42);
        });
    }

    #[test]
    fn luau_installer_invokes_user_function_and_returns_value() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::Both)
            .param("x", "u32")
            .finish();

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let got: u32 = lua.load("return toy_double(21)").eval().unwrap();
        assert_eq!(got, 42);
    }

    // Panics must be caught at the FFI boundary; the test process must not
    // crash and the script must see a normal runtime error.
    fn toy_panic(_x: u32) -> Result<u32, ScriptError> {
        panic!("intentional panic for test");
    }

    #[test]
    fn quickjs_installer_catches_panic_from_user_function() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_panic", toy_panic)
            .scope(ContextScope::Both)
            .param("x", "u32")
            .finish();

        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            for p in r.iter() {
                (p.quickjs_installer)(&ctx).unwrap();
            }
            // Script catches the thrown exception — no Rust panic escapes.
            let msg: String = ctx
                .eval(
                    r#"
                    try { toy_panic(1); "no-throw" }
                    catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(msg.contains("panicked"), "unexpected error message: {msg}");
        });
    }

    #[test]
    fn luau_installer_catches_panic_from_user_function() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_panic", toy_panic)
            .scope(ContextScope::Both)
            .param("x", "u32")
            .finish();

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_installer)(&lua).unwrap();
        }
        let (ok, msg): (bool, String) = lua
            .load("local ok, err = pcall(toy_panic, 1); return ok, tostring(err)")
            .eval()
            .unwrap();
        assert!(!ok, "pcall should report failure");
        assert!(msg.contains("panicked"), "unexpected error message: {msg}");
    }

    #[test]
    fn quickjs_stub_installer_throws_wrong_context() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::DefinitionOnly)
            .param("x", "u32")
            .finish();

        let rt = rquickjs::Runtime::new().unwrap();
        let ctx = rquickjs::Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            for p in r.iter() {
                // Install the stub (as the behavior context would).
                (p.quickjs_stub_installer)(&ctx).unwrap();
            }
            let msg: String = ctx
                .eval(
                    r#"
                    try { toy_double(1); "no-throw" }
                    catch (e) { String(e.message || e) }
                    "#,
                )
                .unwrap();
            assert!(msg.contains("not available"), "unexpected: {msg}");
        });
    }

    #[test]
    fn luau_stub_installer_errors_wrong_context() {
        let mut r = PrimitiveRegistry::new();
        r.register("toy_double", toy_double)
            .scope(ContextScope::DefinitionOnly)
            .param("x", "u32")
            .finish();

        let lua = mlua::Lua::new();
        for p in r.iter() {
            (p.luau_stub_installer)(&lua).unwrap();
        }
        let (ok, msg): (bool, String) = lua
            .load("local ok, err = pcall(toy_double, 1); return ok, tostring(err)")
            .eval()
            .unwrap();
        assert!(!ok);
        assert!(msg.contains("not available"), "unexpected: {msg}");
    }
}
