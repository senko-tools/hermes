//! What keeps clients and servers of different versions talking.
//!
//! - Additive changes never bump [`crate::PROTOCOL_VERSION`]: new fields have `#[serde(default)]`,
//!   new ops are skipped by peers that don't know them, and enums the server sends fall back to
//!   a known value (or an `Unknown` variant) for values they don't know.
//! - A server lists what it can do in `features` ([`features`]), so a newer client hides what an
//!   older server lacks instead of failing at it. An older server sends no list at all.
//! - Lists in `ready` and REST answers decode item by item ([`lenient_vec`]): one item a client
//!   can't read is dropped, not the whole answer.
//! - Fallback values are only for enums the server alone sends. Enums a client sends too (a
//!   notification level, who may DM you) stay strict, so a server still refuses a value it
//!   doesn't know; where a client reads one, the field falls back instead ([`or_default`]).

use std::fmt;
use std::marker::PhantomData;

use serde::de::{Deserialize, Deserializer, IgnoredAny, SeqAccess, Visitor};

/// Names in `InstanceResponse::features` and `InstanceInfo::features`. A server of this build
/// has all of [`features::ALL`].
pub mod features {
    /// `GET /search` takes `after` and `has`, and filters without words.
    pub const SEARCH_FILTERS: &str = "search_filters";

    /// Profiles have an "About me" (`UserProfile::bio`, `UpdateMeRequest::bio`).
    pub const BIO: &str = "bio";

    /// `GET /search` finds parts of words and sorts by relevance (`sort`, `offset`).
    pub const SEARCH_RANK: &str = "search_rank";

    /// Everything this build's server can do that older servers can't.
    pub const ALL: &[&str] = &[SEARCH_FILTERS, BIO, SEARCH_RANK];
}

/// Whether a server's `features` list has `name`.
pub fn has_feature(features: &[String], name: &str) -> bool {
    features.iter().any(|f| f == name)
}

/// One list item: the item, or something this build can't read.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Item<T> {
    Known(T),
    Unknown(IgnoredAny),
}

/// `#[serde(deserialize_with = "lenient_vec")]`: a list whose unreadable items are dropped.
pub fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Items<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Items<T> {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a list")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
            while let Some(item) = seq.next_element::<Item<T>>()? {
                if let Item::Known(item) = item {
                    out.push(item);
                }
            }
            Ok(out)
        }
    }

    deserializer.deserialize_seq(Items(PhantomData))
}

/// `#[serde(deserialize_with = "or_default")]`: a field that reads as its default when this
/// build can't read what was sent.
pub fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(match Item::<T>::deserialize(deserializer)? {
        Item::Known(value) => value,
        Item::Unknown(_) => T::default(),
    })
}

/// A whole answer that is a list, read like [`lenient_vec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lenient<T>(pub Vec<T>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Lenient<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        lenient_vec(deserializer).map(Lenient)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Thing {
        n: u32,
    }

    #[derive(Debug, serde::Deserialize)]
    struct List {
        #[serde(deserialize_with = "lenient_vec")]
        items: Vec<Thing>,
    }

    #[test]
    fn unreadable_items_are_dropped() {
        let list: List =
            serde_json::from_str(r#"{"items":[{"n":1},{"n":"x"},{"m":2},7,{"n":3,"new":true}]}"#)
                .unwrap();
        assert_eq!(list.items, vec![Thing { n: 1 }, Thing { n: 3 }]);
        let Lenient(top) =
            serde_json::from_str::<Lenient<Thing>>(r#"[{"n":"x"},{"n":2}]"#).unwrap();
        assert_eq!(top, vec![Thing { n: 2 }]);
    }

    #[test]
    fn features_are_looked_up() {
        let list = vec!["bio".to_owned()];
        assert!(has_feature(&list, "bio"));
        assert!(!has_feature(&list, "polls"));
    }
}
