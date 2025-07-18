use std::borrow::Cow;

use chrono::{DateTime, NaiveDate, Utc};
use url::Url;

use serde::Serialize;
// use serde_urlencoded;
// use crate::api::BodyError;

/// A trait representing a parameter value.
pub trait ParamValue<'a> {
    #[allow(clippy::wrong_self_convention)]
    /// The parameter value as a string.
    fn as_value(&self) -> Cow<'a, str>;
}

impl ParamValue<'static> for bool {
    fn as_value(&self) -> Cow<'static, str> {
        if *self {
            "true".into()
        } else {
            "false".into()
        }
    }
}

impl<'a> ParamValue<'a> for &'a str {
    fn as_value(&self) -> Cow<'a, str> {
        (*self).into()
    }
}

impl ParamValue<'static> for String {
    fn as_value(&self) -> Cow<'static, str> {
        self.clone().into()
    }
}

impl<'a> ParamValue<'a> for &'a String {
    fn as_value(&self) -> Cow<'a, str> {
        (*self).into()
    }
}

impl<'a> ParamValue<'a> for Cow<'a, str> {
    fn as_value(&self) -> Cow<'a, str> {
        self.clone()
    }
}

impl<'a, 'b: 'a> ParamValue<'a> for &'b Cow<'a, str> {
    fn as_value(&self) -> Cow<'a, str> {
        (*self).clone()
    }
}

impl ParamValue<'static> for u64 {
    fn as_value(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl ParamValue<'static> for f64 {
    fn as_value(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl ParamValue<'static> for i32 {
    fn as_value(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl ParamValue<'static> for u32 {
    fn as_value(&self) -> Cow<'static, str> {
        self.to_string().into()
    }
}

impl ParamValue<'static> for DateTime<Utc> {
    fn as_value(&self) -> Cow<'static, str> {
        self.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
            .into()
    }
}

impl ParamValue<'static> for NaiveDate {
    fn as_value(&self) -> Cow<'static, str> {
        format!("{}", self.format("%Y-%m-%d")).into()
    }
}

/// A structure for form parameters.
#[derive(Debug, Default, Clone)]
pub struct FormParams<'a> {
    params: Vec<(Cow<'a, str>, Cow<'a, str>)>,
}

impl<'a> FormParams<'a> {
    /// Push a single parameter.
    pub fn push<'b, K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: Into<Cow<'a, str>>,
        V: ParamValue<'b>,
        'b: 'a,
    {
        self.params.push((key.into(), value.as_value()));
        self
    }

    /// Push a single parameter.
    pub fn push_opt<'b, K, V>(&mut self, key: K, value: Option<V>) -> &mut Self
    where
        K: Into<Cow<'a, str>>,
        V: ParamValue<'b>,
        'b: 'a,
    {
        if let Some(value) = value {
            self.params.push((key.into(), value.as_value()));
        }
        self
    }

    /// Push a set of parameters.
    pub fn extend<'b, I, K, V>(&mut self, iter: I) -> &mut Self
    where
        I: Iterator<Item = (K, V)>,
        K: Into<Cow<'a, str>>,
        V: ParamValue<'b>,
        'b: 'a,
    {
        self.params
            .extend(iter.map(|(key, value)| (key.into(), value.as_value())));
        self
    }

    // pub fn into_body(self) -> Result<Option<(&'static str, Vec<u8>)>, anyhow::Error> {
    //     let body = serde_urlencoded::to_string(self.params)?;
    //     Ok(Some((
    //         "application/x-www-form-urlencoded",
    //         body.into_bytes(),
    //     )))
    // }
}

/// A structure for query parameters.
#[derive(Debug, Default, Clone)]
pub struct QueryParams<'a> {
    pub params: Vec<(Cow<'a, str>, Cow<'a, str>)>,
}

impl<'a> QueryParams<'a> {
    /// Push a single parameter.
    pub fn push<'b, K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: Into<Cow<'a, str>>,
        V: ParamValue<'b>,
        'b: 'a,
    {
        self.params.push((key.into(), value.as_value()));
        self
    }

    /// Push a single parameter.
    pub fn push_opt<'b, K, V>(&mut self, key: K, value: Option<V>) -> &mut Self
    where
        K: Into<Cow<'a, str>>,
        V: ParamValue<'b>,
        'b: 'a,
    {
        if let Some(value) = value {
            self.params.push((key.into(), value.as_value()));
        }
        self
    }

    /// Push a set of parameters.
    pub fn extend<'b, I, K, V>(&mut self, iter: I) -> &mut Self
    where
        I: Iterator<Item = (K, V)>,
        K: Into<Cow<'a, str>>,
        V: ParamValue<'b>,
        'b: 'a,
    {
        self.params
            .extend(iter.map(|(key, value)| (key.into(), value.as_value())));
        self
    }

    /// Add the parameters to a URL.
    pub fn add_to_url(&self, url: &mut Url) {
        let mut pairs = url.query_pairs_mut();
        pairs.extend_pairs(self.params.iter());
    }

    pub fn to_query_string(&self) -> String {
        let mut url = Url::parse("http://dummy").unwrap();
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in &self.params {
                pairs.append_pair(k, v);
            }
        }
        url.query().unwrap_or("").to_string()
    }
}

use serde_qs;

impl<'a, T> From<&T> for QueryParams<'a>
where
    T: Serialize,
{
    fn from(value: &T) -> Self {
        let query = serde_qs::to_string(value)
            .unwrap_or_else(|err| panic!("Failed to serialize query: {err:?}"));

        let params = query
            .split('&')
            .filter_map(|pair| {
                let mut split = pair.splitn(2, '=');
                let key = split.next()?;
                let val = split.next().unwrap_or_default();

                if val.is_empty() {
                    None
                } else {
                    Some((Cow::Owned(key.to_string()), Cow::Owned(val.to_string())))
                }
            })
            .collect();

        QueryParams { params }
    }
}
