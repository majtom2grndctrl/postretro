// Test-only counting global allocator and the zero-allocation assertion test
// for the eval pass.
// See: context/lib/development_guide.md §3.5 (No `unsafe` — the one approved
//      exception is the System delegation below) and §1.4 (performance).

// The IR eval pass must perform ZERO heap allocation per tick (scripting.md
// §12). To prove it, this module installs a global allocator that delegates
// verbatim to `std::alloc::System` and bumps atomic counters on every alloc /
// dealloc. A test arms the probe *after* bind, snapshots the counter, runs
// `eval_value`, and asserts the alloc delta is zero. Bind and the write path
// are excluded from the assertion window.
//
// This file is compiled only under `#[cfg(test)]` (it is declared from the IR
// module behind a `cfg(test)` gate). The `#[global_allocator]` static itself
// lives in the crate test root (`main.rs`, behind `#[cfg(test)]`) because the
// attribute must sit on a crate-root static; it points at [`CountingAllocator`].

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Number of `alloc` calls observed since process start. Monotonic.
pub(crate) static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Number of `dealloc` calls observed since process start. Monotonic.
pub(crate) static DEALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

/// A global allocator that forwards every request verbatim to the system
/// allocator and only adds atomic bookkeeping. Installing it lets a test count
/// allocations across a precise window.
pub(crate) struct CountingAllocator;

// SAFETY: every method forwards the *identical* `ptr`/`layout` to the
// corresponding `std::alloc::System` method — the System allocator already
// upholds the `GlobalAlloc` contract, and we add nothing but a relaxed atomic
// counter increment around the verbatim call. No pointer is fabricated,
// reinterpreted, or freed by us; the layout passed to `dealloc`/`realloc` is the
// one the caller paired with the original allocation. The atomics impose no
// ordering requirement on the allocation itself, so they cannot violate any
// safety invariant the System allocator relies on.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwards the caller's layout unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `ptr`/`layout` are the verbatim pair the caller received from
        // a prior `alloc`/`realloc` on this same allocator (which forwarded to
        // System), so this is a valid System deallocation.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: forwards the caller's layout unchanged to the system allocator.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `ptr`/`layout` are the verbatim pair from a prior allocation
        // and `new_size` is the caller's; all forwarded unchanged to System.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// A snapshot of the allocation counters, used to measure a delta across a
/// precise window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AllocSnapshot {
    allocs: usize,
}

impl AllocSnapshot {
    /// Capture the current allocation count. Call this AFTER bind, immediately
    /// before the work whose allocations must be zero.
    pub(crate) fn arm() -> Self {
        Self {
            allocs: ALLOC_COUNT.load(Ordering::Relaxed),
        }
    }

    /// Number of `alloc`-family calls since [`AllocSnapshot::arm`].
    pub(crate) fn allocs_since(self) -> usize {
        ALLOC_COUNT.load(Ordering::Relaxed) - self.allocs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::ir::eval::eval_value;
    use crate::scripting::ir::scopes::StubScope;
    use crate::scripting::ir::{bind, BakedIr, IrNode, IrValue, CURRENT_IR_VERSION};

    fn num(v: f32) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Number(v),
        })
    }

    fn boolean(v: bool) -> Box<IrNode> {
        Box::new(IrNode::Const {
            value: IrValue::Bool(v),
        })
    }

    fn input(name: &str) -> Box<IrNode> {
        Box::new(IrNode::Input {
            name: name.to_string(),
        })
    }

    /// Build a tree containing at least one of every opcode, nested at least two
    /// levels deep, then bind it and assert `eval_value` allocates nothing.
    #[test]
    fn eval_pass_is_zero_allocation_over_every_opcode() {
        // Numeric subtree exercising add/sub/mul/div/clamp/lerp, ≥2 deep.
        let arithmetic = IrNode::Clamp {
            x: Box::new(IrNode::Lerp {
                a: Box::new(IrNode::Add {
                    a: input("speed"),
                    b: num(1.0),
                }),
                b: Box::new(IrNode::Sub {
                    a: num(10.0),
                    b: Box::new(IrNode::Mul {
                        a: num(2.0),
                        b: Box::new(IrNode::Div {
                            a: num(8.0),
                            b: num(4.0),
                        }),
                    }),
                }),
                t: num(0.5),
            }),
            lo: num(0.0),
            hi: num(100.0),
        };

        // Boolean subtree exercising lt/le/gt/ge/eq/ne, nested under select.
        let condition = IrNode::Select {
            cond: Box::new(IrNode::Ne {
                a: Box::new(IrNode::Eq {
                    a: input("grounded"),
                    b: boolean(true),
                }),
                b: Box::new(IrNode::Lt {
                    a: Box::new(IrNode::Le {
                        a: num(1.0),
                        b: num(2.0),
                    }),
                    b: Box::new(IrNode::Gt {
                        a: Box::new(IrNode::Ge {
                            a: num(3.0),
                            b: num(2.0),
                        }),
                        b: boolean(false),
                    }),
                }),
            }),
            a: boolean(true),
            b: boolean(false),
        };

        // Root selects between the arithmetic value and 0 based on `condition`,
        // so every opcode is reachable in one eval.
        let root = IrNode::Select {
            cond: Box::new(condition),
            a: Box::new(arithmetic),
            b: num(0.0),
        };

        let scope = StubScope::new();
        let program = bind(
            &BakedIr {
                version: CURRENT_IR_VERSION,
                output: None,
                root,
            },
            &scope,
        )
        .expect("the all-opcode tree binds");

        // Warm any one-time lazy state the test framework or scope might touch
        // by evaluating once before arming, so the measured window is pure eval.
        let _ = eval_value(&program, &scope);

        let snapshot = AllocSnapshot::arm();
        let value = eval_value(&program, &scope);
        let allocs = snapshot.allocs_since();

        // The value must be well-formed (a number, since the chosen arm is
        // arithmetic) and the eval window must have allocated nothing.
        assert!(matches!(value, IrValue::Number(_)), "value: {value:?}");
        assert_eq!(allocs, 0, "eval pass must perform zero heap allocations");
    }
}
