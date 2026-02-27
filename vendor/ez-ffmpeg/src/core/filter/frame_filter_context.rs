//! `FrameFilterContext` provides filters with access to:
//! - A name string (for debugging/logging).
//! - A reference to the pipeline's `attribute_map`, so they can read or modify shared data
//!   without holding a full reference to the entire pipeline.

use std::any::Any;
use std::collections::HashMap;

/// The context passed to each filter method (init, filter_frame, request_frame, uninit).
/// It contains a `name` for the current filter and a mutable reference to the pipeline's attributes.
pub struct FrameFilterContext<'a> {
    name: &'a str,
    attribute_map: &'a mut HashMap<String, Box<dyn Any + std::marker::Send>>,
}

impl<'a> FrameFilterContext<'a> {
    /// Creates a new context for a specific filter name and attribute map.
    pub fn new(name: &'a str, attribute_map: &'a mut HashMap<String, Box<dyn Any + std::marker::Send>>) -> Self {
        Self { name, attribute_map }
    }

    /// Returns the filter's name, useful for logging or debugging.
    pub fn name(&self) -> &str {
        self.name
    }

    /// Retrieves an attribute by `key`, downcasting it to `T`.
    pub fn get_attribute<T: 'static>(&self, key: &str) -> Option<&T> {
        self.attribute_map
            .get(key)
            .and_then(|value| value.downcast_ref::<T>())
    }

    /// Retrieves a mutable attribute by `key`, downcasting it to `T`.
    pub fn get_attribute_mut<T: 'static>(&mut self, key: &str) -> Option<&mut T> {
        self.attribute_map
            .get_mut(key)
            .and_then(|value| value.downcast_mut::<T>())
    }

    /// Inserts or replaces an attribute under `key`.
    pub fn set_attribute<T: 'static + std::marker::Send>(&mut self, key: &str, value: T) {
        self.attribute_map.insert(key.to_string(), Box::new(value));
    }

    /// Removes an attribute by `key` and returns it as `Box<dyn Any>` if found.
    pub fn remove_attribute(&mut self, key: &str) -> Option<Box<dyn Any + std::marker::Send>> {
        self.attribute_map.remove(key)
    }
}
