//! The WebAssembly module that `audiobox-device`'s audio worklet runs.
//!
//! It has no imports, and exports plain functions instead of `wasm-bindgen` glue, which
//! the worklet's scope could not run. The script `web/worklet.js` calls them:
//! - `input(len)` returns where to copy a message of `len` bytes, then `apply(len)`
//!   applies it;
//! - `render(frames)` renders interleaved frames and returns where they are, and
//!   `channels()` says how many channels they have;
//! - `reports_ptr()` and `reports_len()` give the reports to send to the page, and
//!   `reports_clear()` forgets them.
//!
//! `scripts/build-worklet.mjs` builds it into `web/audiobox_device_worklet.wasm`, which
//! `audiobox-device` embeds.
use std::cell::RefCell;

use audiobox_device::__worklet::Engine;

thread_local! {
    static ENGINE: RefCell<Engine> = RefCell::new(Engine::new());
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Room for a message of `len` bytes, where the script copies it.
#[unsafe(no_mangle)]
pub extern "C" fn input(len: usize) -> *mut u8 {
    INPUT.with_borrow_mut(|input| {
        input.resize(len, 0);
        input.as_mut_ptr()
    })
}

/// Applies the message of `len` bytes copied where [`input`] said.
#[unsafe(no_mangle)]
pub extern "C" fn apply(len: usize) {
    INPUT.with_borrow(|input| {
        let message = &input[..len.min(input.len())];
        ENGINE.with_borrow_mut(|engine| engine.apply(message));
    });
}

/// The mixer's channels; 0 until the page's first message has made it.
#[unsafe(no_mangle)]
pub extern "C" fn channels() -> usize {
    ENGINE.with_borrow(|engine| engine.channels())
}

/// Renders `frames` interleaved frames and returns where they are.
#[unsafe(no_mangle)]
pub extern "C" fn render(frames: usize) -> *const f32 {
    ENGINE.with_borrow_mut(|engine| engine.render(frames).as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn reports_ptr() -> *const u8 {
    ENGINE.with_borrow(|engine| engine.reports().as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn reports_len() -> usize {
    ENGINE.with_borrow(|engine| engine.reports().len())
}

#[unsafe(no_mangle)]
pub extern "C" fn reports_clear() {
    ENGINE.with_borrow_mut(|engine| engine.clear_reports());
}
