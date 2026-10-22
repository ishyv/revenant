//! Thread-local counting excludes unrelated executor allocations. Timing disables counting.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

#[derive(Clone, Copy, Default)]
pub(super) struct Counts {
    pub calls: u64,
    pub requested_bytes: u64,
}
thread_local! { static ACTIVE: Cell<Option<Counts>> = const { Cell::new(None) }; }
struct Counter;
#[global_allocator]
static ALLOCATOR: Counter = Counter;
fn record(bytes: usize) {
    let _ = ACTIVE.try_with(|active| {
        if let Some(mut counts) = active.get() {
            counts.calls += 1;
            counts.requested_bytes += bytes as u64;
            active.set(Some(counts));
        }
    });
}
unsafe impl GlobalAlloc for Counter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        record(bytes);
        unsafe { System.realloc(pointer, layout, bytes) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
pub(super) struct Window;
impl Window {
    pub fn start() -> Self {
        ACTIVE.with(|active| {
            assert!(active.get().is_none());
            active.set(Some(Counts::default()));
        });
        Self
    }
    pub fn stop(self) -> Counts {
        ACTIVE.with(|active| active.take().unwrap())
    }
}
impl Drop for Window {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|active| active.set(None));
    }
}
