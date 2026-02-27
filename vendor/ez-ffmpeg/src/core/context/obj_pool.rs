use crossbeam::queue::SegQueue;
use std::sync::Arc;
pub(crate) struct ObjPool<T> {
    queue: Arc<SegQueue<T>>,
    create_fn: fn() -> crate::error::Result<T>,
    unref_fn: fn(&mut T),
    is_null_fn: fn(& T) -> bool,
}

impl<T> ObjPool<T> {
    pub(crate) fn new(initial_size: usize, create_fn: fn() -> crate::error::Result<T>, unref_fn: fn(&mut T),is_null_fn: fn(& T) -> bool,) -> crate::error::Result<Self> {
        let queue = SegQueue::new();
        for _ in 0..initial_size {
            queue.push(create_fn()?);
        }
        Ok(Self {
            queue: Arc::new(queue),
            create_fn,
            unref_fn,
            is_null_fn,
        })
    }

    pub(crate) fn get(&self) -> crate::error::Result<T> {
        if let Some(obj) = self.queue.pop() {
            Ok(obj)
        } else {
            (self.create_fn)()
        }
    }

    pub(crate) fn release(&self, mut obj: T) {
        if (self.is_null_fn)(&obj) {
            return;
        }
        (self.unref_fn)(&mut obj);
        self.queue.push(obj);
    }
}

impl<T> Clone for ObjPool<T> {
    fn clone(&self) -> Self {
        ObjPool {
            queue: Arc::clone(&self.queue),
            create_fn: self.create_fn,
            unref_fn: self.unref_fn,
            is_null_fn: self.is_null_fn,
        }
    }
}