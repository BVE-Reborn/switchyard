#[cfg(any(target_os = "windows", target_os = "linux"))]
pub use affinity::set_thread_affinity;

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn set_thread_affinity<B: AsRef<[usize]>>(_: B) -> Result<()> {
    Ok(())
}
