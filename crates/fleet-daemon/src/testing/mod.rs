//! Shared daemon testing support and deterministic fakes.

pub mod fakes;
mod files;

/// Locks a fake's mutex, recovering the guard after a previous holder panicked.
fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
