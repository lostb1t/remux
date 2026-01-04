use moka::{Expiry, sync::Cache};
use std::{any::Any, sync::Arc, time::Duration};

pub trait Cacheable: std::fmt::Debug + Send + Sync + 'static {
    fn as_any(&self) -> &dyn Any;
    fn clone_box(&self) -> Box<dyn Cacheable>;
}

impl<T> Cacheable for T
where
    T: Clone + std::fmt::Debug + Send + Sync + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Cacheable> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Cacheable> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

#[derive(Debug)]
pub struct StoreEntry {
    pub item: Box<dyn Cacheable>,
    pub ttl: Duration,
}

impl Clone for StoreEntry {
    fn clone(&self) -> Self {
        StoreEntry {
            item: self.item.clone_box(),
            ttl: self.ttl,
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

    pub fn save<T: Cacheable>(&self, key: impl Into<String>, item: T, ttl: Duration) {
        let entry = Arc::new(StoreEntry {
            item: Box::new(item),
            ttl,
        });
        self.inner.insert(key.into(), entry);
    }

    pub fn insert<T: Cacheable>(
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
            item: Box::new(item),
            ttl,
        });
        self.inner.insert(key, entry);
        true
    }

    // Voeg `Clone` toe aan de trait bound voor `T`
    pub fn get<T: Cacheable + 'static + Clone>(&self, key: &str) -> Option<T> {
        self.inner
            .get(key)
            .and_then(|entry| entry.item.as_any().downcast_ref::<T>().cloned())
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
