use std::borrow::Cow;
use std::fmt;
use std::iter;
use std::ops;

use itertools::Itertools;
// use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};

use super::ParamValue;

/// A comma-separated list of values.
#[derive(Debug, Clone, Default)]
pub struct CommaSeparatedList<T> {
    data: Vec<T>,
}

impl<T> CommaSeparatedList<T> {
    /// Create a new, empty comma-separated list.
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }
}

impl<T> From<Vec<T>> for CommaSeparatedList<T> {
    fn from(data: Vec<T>) -> Self {
        Self { data }
    }
}

impl<T> iter::FromIterator<T> for CommaSeparatedList<T> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: iter.into_iter().collect(),
        }
    }
}

impl<T> ops::Deref for CommaSeparatedList<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> ops::DerefMut for CommaSeparatedList<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> fmt::Display for CommaSeparatedList<T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.data.iter().format(","))
    }
}

impl<'a, T> ParamValue<'a> for CommaSeparatedList<T>
where
    T: ParamValue<'a>,
{
    fn as_value(&self) -> Cow<'a, str> {
        format!("{}", self.data.iter().map(|d| d.as_value()).format(",")).into()
    }
}

impl<'a, 'b, T> ParamValue<'a> for &'b CommaSeparatedList<T>
where
    T: ParamValue<'a>,
{
    fn as_value(&self) -> Cow<'a, str> {
        format!("{}", self.data.iter().map(|d| d.as_value()).format(",")).into()
    }
}
