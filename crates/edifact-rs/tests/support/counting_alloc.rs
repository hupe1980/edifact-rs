//! Test-only counting global allocator.
//!
//! Forwards every call to [`System`] and records, **per thread**, the number of
//! allocator calls (`alloc`, `alloc_zeroed` and `realloc` each count as one),
//! the live bytes and the peak live bytes since the last [`measure`] began.
//! Per-thread counters let the default parallel test runner stay on: a test
//! only ever sees the allocations its own thread made.
//!
//! This module is the one place in the repository where `unsafe` appears: a
//! global allocator cannot be written without `unsafe impl GlobalAlloc`. It is
//! compiled only into integration-test binaries that include it with
//! `#[path]` and install [`CountingAlloc`] with `#[global_allocator]`; neither
//! published library contains it, and both keep `#![deny(unsafe_code)]`.
//!
//! The counters are `const`-initialised `Cell`s: no destructor and no lazy
//! initialisation, so touching them inside `alloc` cannot recurse into the
//! allocator. `try_with` ignores the window during thread teardown in which
//! thread-locals are no longer reachable.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

/// Allocation figures of one [`measure`] call, for the calling thread only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    /// Allocator calls that obtained or resized memory (`alloc`,
    /// `alloc_zeroed`, `realloc`).
    pub allocations: usize,
    /// Highest number of bytes live at once, counted from zero at the start
    /// of the measurement (memory allocated earlier is not included).
    pub peak_bytes: usize,
    /// Bytes still live at the end of the measurement (negative when the
    /// closure freed more than it allocated).
    pub net_bytes: isize,
}

/// Global allocator that counts, then delegates to [`System`].
pub struct CountingAlloc;

fn record(delta: isize, counts_as_allocation: bool) {
    let _ = ALLOCATIONS.try_with(|a| {
        if counts_as_allocation {
            a.set(a.get() + 1);
        }
    });
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| {
            if now > peak.get() {
                peak.set(now);
            }
        });
    });
}

// SAFETY: every method forwards to `System` with the caller's arguments
// unchanged, so `System`'s guarantees are the guarantees of this allocator. The
// bookkeeping only touches `const`-initialised thread-local `Cell`s, which
// neither allocate nor unwind.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged; the caller upholds `GlobalAlloc::alloc`'s
        // contract (non-zero size).
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record(layout.size() as isize, true);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record(layout.size() as isize, true);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was returned by this allocator, i.e. by `System`, with
        // this `layout`; the caller upholds `GlobalAlloc::dealloc`'s contract.
        unsafe { System.dealloc(ptr, layout) };
        record(-(layout.size() as isize), false);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: `ptr` was returned by `System` with `layout`; the caller
        // upholds `GlobalAlloc::realloc`'s contract for `new_size`.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            record(new_size as isize - layout.size() as isize, true);
        }
        new_ptr
    }
}

/// Run `f` and return its result with the calling thread's allocation figures.
///
/// Counters are reset on entry, so nested or earlier activity does not leak
/// into the figures. Frees of memory allocated before the call lower
/// `net_bytes` but never the starting point of `peak_bytes`.
pub fn measure<R>(f: impl FnOnce() -> R) -> (R, Stats) {
    ALLOCATIONS.with(|a| a.set(0));
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    let result = f();
    let stats = Stats {
        allocations: ALLOCATIONS.with(Cell::get),
        peak_bytes: PEAK.with(Cell::get).max(0) as usize,
        net_bytes: LIVE.with(Cell::get),
    };
    (result, stats)
}
