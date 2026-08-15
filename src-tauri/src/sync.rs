//! Small locking helper shared by the command layer and the exporter.

use std::sync::{Mutex, MutexGuard};

/// Lock without `unwrap`.
///
/// A panic anywhere under one of this app's locks would otherwise poison it,
/// and every later `.lock().unwrap()` on the same mutex panics too — one
/// transient failure permanently takes down detection, the waveform, or the
/// ability to cancel an export for the rest of the session.
///
/// What these mutexes guard is a cache, a set of cancel flags, and a ring of
/// recent stderr lines. None of them holds an invariant that a half-finished
/// update could corrupt in a way worth crashing the feature over, so recovering
/// the data and carrying on is strictly better than propagating the poison.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
