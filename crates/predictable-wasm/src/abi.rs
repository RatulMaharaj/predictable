//! The three-function C ABI the browser loads.
//!
//! ```text
//! pv_alloc(len: usize) -> *mut u8      // caller writes `len` UTF-8 bytes there
//! pv_call(ptr: *mut u8, len: usize) -> *mut u8
//! pv_free(ptr: *mut u8, len: usize)
//! ```
//!
//! `pv_call` consumes the request buffer and returns a pointer to a response
//! buffer whose first four bytes are its length as a little-endian `u32`,
//! followed by that many UTF-8 bytes. The caller frees it with
//! `pv_free(ptr, 4 + len)`. That is the entire protocol; the JS side is
//! `crates/predictable-viz/frontend/src/wasm/engine.ts`.
//!
//! Why length-prefixed rather than a second `pv_last_len()` call: two calls to
//! read one result is two chances for the page to interleave another `pv_call`
//! and read the wrong length. One buffer cannot be raced.

#[cfg(target_family = "wasm")]
mod exports {
    use std::alloc::{alloc, dealloc, Layout};

    fn layout(len: usize) -> Layout {
        Layout::from_size_align(len.max(1), 1).expect("a byte-aligned layout always exists")
    }

    /// Allocate `len` bytes for the host to write a request into.
    ///
    /// # Safety
    /// The returned pointer is valid for `len` bytes and must be handed back to
    /// [`pv_call`] or [`pv_free`] with the same `len`.
    #[no_mangle]
    pub unsafe extern "C" fn pv_alloc(len: usize) -> *mut u8 {
        alloc(layout(len))
    }

    /// Free a buffer obtained from [`pv_alloc`] or returned by [`pv_call`].
    ///
    /// # Safety
    /// `ptr`/`len` must be a pair this module produced, freed at most once.
    #[no_mangle]
    pub unsafe extern "C" fn pv_free(ptr: *mut u8, len: usize) {
        if !ptr.is_null() {
            dealloc(ptr, layout(len));
        }
    }

    /// Handle a request. Consumes the request buffer, returns a length-prefixed
    /// response buffer.
    ///
    /// # Safety
    /// `ptr`/`len` must describe a buffer from [`pv_alloc`] holding UTF-8.
    #[no_mangle]
    pub unsafe extern "C" fn pv_call(ptr: *mut u8, len: usize) -> *mut u8 {
        let request = String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len)).into_owned();
        pv_free(ptr, len);
        let response = crate::api::call(&request);
        let bytes = response.as_bytes();
        let out = alloc(layout(4 + bytes.len()));
        let header = (bytes.len() as u32).to_le_bytes();
        std::ptr::copy_nonoverlapping(header.as_ptr(), out, 4);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.add(4), bytes.len());
        out
    }
}
