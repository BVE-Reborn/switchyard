/// Struct that lies about T being send/sync.
/// All access to the inner value is unsafe because duh.
///
/// # Safety
///
/// - Implementations of PartialEq, Eq, and Hash for `T` _must not_
///   touch anything but the address of T.
pub(crate) struct SenderSyncer<T>(T);

#[allow(dead_code)]
impl<T> SenderSyncer<T> {
    pub unsafe fn new(t: T) -> Self {
        SenderSyncer(t)
    }

    pub unsafe fn inner(self) -> T {
        self.0
    }

    pub unsafe fn inner_ref(&self) -> &T {
        &self.0
    }

    pub unsafe fn inner_ref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

unsafe impl<T> Send for SenderSyncer<T> {}
unsafe impl<T> Sync for SenderSyncer<T> {}

/// A pointer to thread local data that can be sent between threads.
///
/// # Safety
///
/// - Can only be dereferenced on another thread if `TD` is `Send`.
/// - Can only get a `&mut` to inner value if no other references exist.
pub struct ThreadLocalPointer<TD>(pub *mut TD);

unsafe impl<TD> Send for ThreadLocalPointer<TD> {}
unsafe impl<TD> Sync for ThreadLocalPointer<TD> {}
