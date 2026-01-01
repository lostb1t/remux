use moka::{Expiry, sync::Cache};
use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub struct StoreEntry<T> {
    pub item: T,
    pub ttl: Duration,
}

#[derive(Clone, Default)]
struct PerEntryExpiry;

impl<T> Expiry<String, Arc<StoreEntry<T>>> for PerEntryExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &Arc<StoreEntry<T>>,
        _now: std::time::Instant,
    ) -> Option<Duration> {
        Some(value.ttl)
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &Arc<StoreEntry<T>>,
        _now: std::time::Instant,
        _current: Option<Duration>,
    ) -> Option<Duration> {
        Some(value.ttl)
    }

    fn expire_after_read(
        &self,
        _key: &String,
        _value: &Arc<StoreEntry<T>>,
        _now: std::time::Instant,
        current: Option<Duration>,
        _extra_param: std::time::Instant,
    ) -> Option<Duration> {
        current
    }
}

#[derive(Clone, Debug)]
pub struct BaseItemStore<T>
where
    T: Clone + std::fmt::Debug + Send + Sync + 'static,
{
    inner: Cache<String, Arc<StoreEntry<T>>>,
}

impl<T> BaseItemStore<T>
where
    T: Clone + std::fmt::Debug + Send + Sync + 'static,
{
    pub fn new(max_capacity: u64) -> Self {
        let inner = Cache::builder()
            .max_capacity(max_capacity)
            .expire_after(PerEntryExpiry)
            .build();

        Self { inner }
    }

    pub fn insert(&self, key: impl Into<String>, item: T) {
        let entry = Arc::new(StoreEntry {
            item,
            ttl: Duration::from_secs(3600),
        });
        self.inner.insert(key.into(), entry);
    }

    pub fn insert_with_ttl(&self, key: impl Into<String>, item: T, ttl: Duration) {
        let entry = Arc::new(StoreEntry { item, ttl });
        self.inner.insert(key.into(), entry);
    }

    pub fn get(&self, key: &str) -> Option<T>
    where
        T: Clone,
    {
        self.inner.get(key).map(|entry| entry.item.clone())
    }

    pub fn remove_key(&self, key: &str) -> Option<Arc<StoreEntry<T>>> {
        self.inner.remove(key)
    }
}
