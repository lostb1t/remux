use dashmap::DashMap;
use std::hash::Hash;
use std::sync::{Arc, OnceLock};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// Per-key async mutex. Only one task at a time may hold the lock for a given key.
/// The map entry is removed automatically when the guard is dropped.
pub(crate) struct KeyedLock<K: Eq + Hash + Clone + Send + Sync + 'static> {
    map: OnceLock<Arc<DashMap<K, Arc<Mutex<()>>>>>,
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static> KeyedLock<K> {
    pub const fn new() -> Self {
        Self {
            map: OnceLock::new(),
        }
    }

    fn inner(&self) -> Arc<DashMap<K, Arc<Mutex<()>>>> {
        self.map
            .get_or_init(|| Arc::new(DashMap::new()))
            .clone()
    }

    /// Acquire the lock for `key`, inserting an entry if none exists.
    pub async fn lock(&self, key: K) -> KeyedLockGuard<K> {
        let map = self.inner();
        let mutex = map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = mutex
            .lock_owned()
            .await;
        KeyedLockGuard { map, key, _guard }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.map
            .get()
            .map_or(false, |m| m.contains_key(key))
    }

    /// Acquire the lock only if an entry already exists (someone else is working).
    /// Returns `None` immediately if no entry is found.
    pub async fn lock_if_exists(&self, key: &K) -> Option<KeyedLockGuard<K>> {
        let map = self.inner();
        let mutex = map
            .get(key)
            .map(|e| Arc::clone(&e))?;
        let _guard = mutex
            .lock_owned()
            .await;
        Some(KeyedLockGuard {
            map,
            key: key.clone(),
            _guard,
        })
    }
}

pub(crate) struct KeyedLockGuard<K: Eq + Hash + Clone + Send + Sync + 'static> {
    map: Arc<DashMap<K, Arc<Mutex<()>>>>,
    key: K,
    _guard: OwnedMutexGuard<()>,
}

impl<K: Eq + Hash + Clone + Send + Sync + 'static> Drop for KeyedLockGuard<K> {
    fn drop(&mut self) {
        self.map
            .remove(&self.key);
    }
}
