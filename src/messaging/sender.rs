use std::ffi::{CString, c_char, c_void};

use serde::Serialize;
use tracing::warn;

/// A messaging sender backed by a C function pointer.
///
/// Used by `run_maker_loop` to publish offers via the delivery module.
/// The C++ side provides a trampoline that routes calls to the delivery module.
pub struct MessagingSender {
    callback: unsafe extern "C" fn(*const c_char, *const c_char, *mut c_void),
    user_data: *mut c_void,
}

// Safety: the C callback and user_data pointer are provided by the C++ caller
// and are expected to be usable from any thread (the C++ trampoline marshals
// to the Qt main thread internally).
unsafe impl Send for MessagingSender {}
unsafe impl Sync for MessagingSender {}

impl MessagingSender {
    /// Create a new sender from a C callback and opaque user data.
    pub fn new(
        callback: unsafe extern "C" fn(*const c_char, *const c_char, *mut c_void),
        user_data: *mut c_void,
    ) -> Self {
        Self {
            callback,
            user_data,
        }
    }

    /// Publish a serializable payload to a content topic.
    pub fn publish<T: Serialize>(&self, topic: &str, payload: &T) {
        let payload_json = match serde_json::to_string(payload) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to serialize messaging payload: {e}");
                return;
            }
        };

        let topic_c = match CString::new(topic) {
            Ok(s) => s,
            Err(e) => {
                warn!("topic contains null byte: {e}");
                return;
            }
        };

        let payload_c = match CString::new(payload_json) {
            Ok(s) => s,
            Err(e) => {
                warn!("payload contains null byte: {e}");
                return;
            }
        };

        unsafe {
            (self.callback)(topic_c.as_ptr(), payload_c.as_ptr(), self.user_data);
        }
    }
}
