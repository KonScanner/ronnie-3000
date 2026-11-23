#![allow(unsafe_code)]

/// One borrowed unit of work with a plain function pointer and context pointer.
#[derive(Clone, Copy)]
pub struct Job {
    function: fn(*mut ()) -> bool,
    context: *mut (),
}

impl Job {
    /// Creates a job whose context remains valid until its join completes.
    ///
    /// # Safety
    ///
    /// The caller must keep `context` alive and valid for reads and writes until
    /// `fork_join` returns. Distinct jobs must permit concurrent execution without
    /// aliasing mutable state. The function must return `false` to report failure.
    pub unsafe fn new<T>(function: fn(*mut ()) -> bool, context: &mut T) -> Self {
        Self {
            function,
            context: std::ptr::from_mut(context).cast(),
        }
    }

    pub(crate) const fn noop() -> Self {
        Self {
            function: |_| true,
            context: std::ptr::null_mut(),
        }
    }

    pub(crate) fn run(self) -> bool {
        (self.function)(self.context)
    }
}
