use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone)]
pub struct ThreadSynchronizer {
    inner: Arc<Inner>,
}

struct Inner {
    counter: Mutex<usize>,
    condvar: Condvar,
    #[cfg(feature = "async")]
    waker: Mutex<Option<std::task::Waker>>,
}

impl ThreadSynchronizer {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                counter: Mutex::new(0),
                condvar: Condvar::new(),
                #[cfg(feature = "async")]
                waker: Mutex::new(None),
            }),
        }
    }

    pub(crate) fn thread_start(&self) {
        let mut counter = self.inner.counter.lock().unwrap();
        *counter += 1;
    }

    pub(crate) fn thread_done(&self) {
        let mut counter = self.inner.counter.lock().unwrap();
        *counter -= 1;

        if *counter == 0 {
            self.inner.condvar.notify_one();

            #[cfg(feature = "async")]
            if let Some(waker) = self.inner.waker.lock().unwrap().take() {
                waker.wake();
            }
        }
    }

    pub(crate) fn wait_for_all_threads(&self) {
        let mut counter = self.inner.counter.lock().unwrap();
        while *counter > 0 {
            counter = self.inner.condvar.wait(counter).unwrap();
        }
    }

    pub(crate) fn is_all_threads_done(&self) -> bool {
        let counter = self.inner.counter.lock().unwrap();
        *counter == 0
    }

    #[cfg(feature = "async")]
    pub(crate) fn set_waker(&self, waker: std::task::Waker) {
        let mut waker_slot = self.inner.waker.lock().unwrap();
        *waker_slot = Some(waker);
    }
}
