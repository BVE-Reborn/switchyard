//! Reexport of the `affinity` crate with no-op stubs on other platforms.

/// No-op on unsupported platforms.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use affinity::set_thread_affinity;

/// No-op on unsupported platforms. Binds the current thread to the specified core(s).
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn set_thread_affinity<B: AsRef<[usize]>>(_: B) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
