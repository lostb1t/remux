use moka::{Expiry, sync::Cache};
use std::{any::Any, sync::Arc, time::Duration};

#[derive(Debug)]
pub struct StoreEntry {
    pub item: Arc<dyn Any + Send + Sync>,
    pub ttl: Duration,
    pub weight: u32,
}

impl Clone for StoreEntry {
    fn clone(&self) -> Self {
        StoreEntry {
            item: Arc::clone(&self.item),
            ttl: self.ttl,
            weight: self.weight,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Store {
    inner: Cache<String, Arc<StoreEntry>>,
}

impl Store {
    pub fn new(max_capacity: u64) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .expire_after(PerEntryExpiry)
            .build();
        Self { inner }
    }

    /// Byte-limited cache. `max_bytes` is the total weight cap; items must be
    /// saved via `save_with_weight` to count correctly against this limit.
    pub fn new_weighted(max_bytes: u64) -> Self {
        let inner = Cache::builder()
            .weigher(|_, entry: &Arc<StoreEntry>| entry.weight)
            .max_capacity(max_bytes)
            .expire_after(PerEntryExpiry)
            .build();
        Self { inner }
    }

    pub fn with_cache(cache: Cache<String, Arc<StoreEntry>>) -> Self {
        Self { inner: cache }
    }

    pub fn save<T: Any + Send + Sync + 'static>(
        &self,
        key: impl Into<String>,
        item: T,
        ttl: Duration,
    ) {
        let entry = Arc::new(StoreEntry {
            item: Arc::new(item),
            ttl,
            weight: 1,
        });
        self.inner.insert(key.into(), entry);
    }

    pub fn save_with_weight<T: Any + Send + Sync + 'static>(
        &self,
        key: impl Into<String>,
        item: T,
        weight: u32,
        ttl: Duration,
    ) {
        let entry = Arc::new(StoreEntry {
            item: Arc::new(item),
            ttl,
            weight,
        });
        self.inner.insert(key.into(), entry);
    }

    pub fn insert<T: Any + Send + Sync + 'static>(
        &self,
        key: impl Into<String>,
        item: T,
        ttl: Duration,
    ) -> bool {
        let key = key.into();
        if self.inner.contains_key(&key) {
            return false;
        }
        let entry = Arc::new(StoreEntry {
            item: Arc::new(item),
            ttl,
            weight: 1,
        });
        self.inner.insert(key, entry);
        true
    }

    pub fn get<T: Any + Send + Sync + Clone>(
        &self,
        key: impl Into<String>,
    ) -> Option<T> {
        let key: String = key.into();

        self.inner.get(&key).and_then(|entry| {
            Arc::clone(&entry.item)
                .downcast::<T>()
                .ok()
                .map(|arc| (*arc).clone())
        })
    }

    pub fn delete(&self, key: impl Into<String>) {
        self.inner.invalidate(&key.into());
    }

    /// Scan all keys with a given prefix.
    pub fn scan_keys(&self, prefix: &str) -> Vec<String> {
        self.inner
            .iter()
            .filter_map(|(key, _)| {
                if key.starts_with(prefix) {
                    Some((*key).clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

#[derive(Clone, Default)]
struct PerEntryExpiry;

impl Expiry<String, Arc<StoreEntry>> for PerEntryExpiry {
    fn expire_after_create(
        &self,
        _: &String,
        value: &Arc<StoreEntry>,
        _: std::time::Instant,
    ) -> Option<Duration> {
        Some(value.ttl)
    }

    fn expire_after_update(
        &self,
        _: &String,
        value: &Arc<StoreEntry>,
        _: std::time::Instant,
        _: Option<Duration>,
    ) -> Option<Duration> {
        Some(value.ttl)
    }

    fn expire_after_read(
        &self,
        _: &String,
        _: &Arc<StoreEntry>,
        _: std::time::Instant,
        current: Option<Duration>,
        _: std::time::Instant,
    ) -> Option<Duration> {
        current
    }
}
