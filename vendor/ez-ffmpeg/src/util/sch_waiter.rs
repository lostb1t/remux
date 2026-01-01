use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use ffmpeg_sys_next::av_gettime_relative;

/// A synchronization primitive that allows a thread to wait until a condition becomes false.
///
/// This struct is used to avoid busy-waiting when a resource is not available.  Threads can
/// call `wait` which will block until another thread calls `set` with `false`.
pub(crate) struct SchWaiter {
    /// A mutex protecting the `Condvar`.
    lock: Mutex<()>,
    /// A condition variable used for waiting.
    cond: Condvar,
    /// An atomic boolean indicating whether the waiter is "choked" (should wait).
    choked: AtomicBool,
    choked_prev: AtomicBool,
    choked_next: AtomicBool,
}

impl Default for SchWaiter {
    fn default() -> Self {
        Self::new()
    }
}

impl SchWaiter {
    /// Creates a new `SchWaiter`.
    pub(crate) fn new() -> Self {
        Self {
            lock: Mutex::new(()),
            cond: Condvar::new(),
            choked: AtomicBool::new(false),
            choked_prev: AtomicBool::new(false),
            choked_next: AtomicBool::new(false),
        }
    }

    /// Waits until the `choked` condition becomes false.
    ///
    /// If `choked` is initially false, this function returns immediately. Otherwise, it
    /// acquires the lock, and waits on the condition variable until `choked` becomes false.
    #[allow(dead_code)]
    pub(crate) fn wait(&self) {
        // early return
        if !self.choked.load(Ordering::Acquire) {
            return;
        }

        let mut guard = self.lock.lock().unwrap();
        // avoid spurious wakeup
        while self.choked.load(Ordering::Acquire) {
            guard = self.cond.wait(guard).unwrap();
        }
    }

    pub(crate) fn get_choked(&self) -> bool {
        self.choked.load(Ordering::Acquire)
    }

    pub(crate) fn wait_with_scheduler_status(&self, scheduler_status: &Arc<AtomicUsize>, cal_wait_time: bool) -> i64 {
        // early return
        if !self.choked.load(Ordering::Acquire) {
            return 0;
        }
        if scheduler_status.load(Ordering::Acquire)
            == crate::core::scheduler::ffmpeg_scheduler::STATUS_END
        {
            return 0;
        }

        let start = if cal_wait_time {
            unsafe { av_gettime_relative() }
        } else {
            0
        };
        let mut guard = self.lock.lock().unwrap();
        // avoid spurious wakeup
        while self.choked.load(Ordering::Acquire)
            && scheduler_status.load(Ordering::Acquire)
                != crate::core::scheduler::ffmpeg_scheduler::STATUS_END
        {
            guard = self.cond.wait(guard).unwrap();
        }

        if cal_wait_time {
            unsafe { av_gettime_relative() - start }
        } else {
            0
        }
    }

    /// Sets the `choked` condition to the specified value and notifies a waiting thread.
    ///
    /// This function acquires the lock, updates the `choked` value, and notifies a single
    /// waiting thread (if any).
    pub(crate) fn set(&self, choked: bool) {
        let _guard = self.lock.lock().unwrap();
        self.choked.store(choked, Ordering::Release);
        self.cond.notify_one();
    }

    pub(crate) fn set_choked_prev(&self, value: bool) {
        self.choked_prev.store(value, Ordering::Release);
    }

    pub(crate) fn get_choked_prev(&self) -> bool {
        self.choked_prev.load(Ordering::Acquire)
    }

    pub(crate) fn set_choked_next(&self, value: bool) {
        self.choked_next.store(value, Ordering::Release);
    }

    pub(crate) fn get_choked_next(&self) -> bool {
        self.choked_next.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_wait_when_not_choked() {
        let waiter = Arc::new(SchWaiter::new());
        let waiter_clone = Arc::clone(&waiter);

        let handle = thread::spawn(move || {
            waiter_clone.wait();
        });

        handle.join().unwrap();
    }

    #[test]
    fn test_wait_when_choked() {
        let waiter = Arc::new(SchWaiter::new());
        let waiter_clone = Arc::clone(&waiter);

        let handle = thread::spawn(move || {
            waiter_clone.wait();
        });

        thread::sleep(Duration::from_millis(100));

        waiter.set(false);

        handle.join().unwrap();
    }

    #[test]
    fn test_set_choked() {
        let waiter = Arc::new(SchWaiter::new());
        let waiter_clone = Arc::clone(&waiter);

        waiter.set(true);

        let handle = thread::spawn(move || {
            waiter_clone.wait();
        });

        thread::sleep(Duration::from_millis(100));

        waiter.set(false);

        handle.join().unwrap();
    }

    #[test]
    fn test_no_deadlock() {
        let waiter = Arc::new(SchWaiter::new());
        let waiter_clone = Arc::clone(&waiter);

        let handle = thread::spawn(move || {
            waiter_clone.set(true);
            waiter_clone.wait();
        });

        thread::sleep(Duration::from_millis(100));

        waiter.set(false);

        handle.join().unwrap();
    }
}
