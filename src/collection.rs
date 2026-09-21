//! The typed face of one collection in the Platform State.
//!
//! The store keeps a collection as JSON bodies keyed by kind and id and knows
//! nothing about their shape. A `Collection<T>` names the kind and does the
//! serde, so a module asks for `Vec<Record>` and filters it with iterators.
//! That is the whole query language for now; `json_extract` waits for a
//! filter that needs it.

use std::marker::PhantomData;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::store::{StateStore, StoreError};

/// What identifies one item within its collection.
pub trait Keyed {
    fn key(&self) -> String;
}

pub struct Collection<T> {
    kind: &'static str,
    _marker: PhantomData<fn() -> T>,
}

impl<T: Serialize + DeserializeOwned + Keyed> Collection<T> {
    pub const fn new(kind: &'static str) -> Self {
        Collection {
            kind,
            _marker: PhantomData,
        }
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub async fn list<S: StateStore>(&self, store: &S) -> Result<Vec<T>, StoreError> {
        store
            .list_records(self.kind)
            .await?
            .iter()
            .map(|body| self.parse(body))
            .collect()
    }

    pub async fn get<S: StateStore>(&self, store: &S, key: &str) -> Result<Option<T>, StoreError> {
        match store.get_record(self.kind, key).await? {
            Some(body) => Ok(Some(self.parse(&body)?)),
            None => Ok(None),
        }
    }

    pub async fn exists<S: StateStore>(&self, store: &S, key: &str) -> Result<bool, StoreError> {
        Ok(store.get_record(self.kind, key).await?.is_some())
    }

    pub async fn upsert<S: StateStore>(&self, store: &S, item: &T) -> Result<(), StoreError> {
        store
            .put_record(self.kind, &item.key(), &self.render(item)?)
            .await
    }

    pub async fn remove<S: StateStore>(&self, store: &S, key: &str) -> Result<bool, StoreError> {
        store.delete_record(self.kind, key).await
    }

    /// Replaces the collection with `items` in one commit.
    pub async fn replace_all<S: StateStore>(
        &self,
        store: &S,
        items: &[T],
    ) -> Result<(), StoreError> {
        let rows = items
            .iter()
            .map(|item| Ok((item.key(), self.render(item)?)))
            .collect::<Result<Vec<_>, StoreError>>()?;
        store.replace_records(self.kind, &rows).await
    }

    fn parse(&self, body: &str) -> Result<T, StoreError> {
        serde_json::from_str(body)
            .map_err(|e| StoreError::Serialize(format!("could not read a {}: {e}", self.kind)))
    }

    fn render(&self, item: &T) -> Result<String, StoreError> {
        serde_json::to_string(item)
            .map_err(|e| StoreError::Serialize(format!("could not write a {}: {e}", self.kind)))
    }
}

pub static RECORDS: Collection<crate::dns_records::Record> = Collection::new("dns-record");
pub static MACHINES: Collection<crate::environments::EnvironmentRecord> =
    Collection::new("virtual-machine");
pub static IMAGES: Collection<crate::custom_images::Image> = Collection::new("custom-image");
pub static TASKS: Collection<crate::tasks::Task> = Collection::new("task");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::FakeStateStore;
    use serde::Deserialize;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Note {
        id: String,
        text: String,
    }

    impl Keyed for Note {
        fn key(&self) -> String {
            self.id.clone()
        }
    }

    static NOTES: Collection<Note> = Collection::new("note");

    fn note(id: &str, text: &str) -> Note {
        Note {
            id: id.into(),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn items_round_trip_through_the_store() {
        let store = FakeStateStore::new();
        NOTES.upsert(&store, &note("a", "first")).await.unwrap();
        NOTES.upsert(&store, &note("b", "second")).await.unwrap();
        NOTES.upsert(&store, &note("a", "edited")).await.unwrap();

        assert_eq!(
            NOTES.list(&store).await.unwrap(),
            vec![note("a", "edited"), note("b", "second")]
        );
        assert_eq!(
            NOTES.get(&store, "b").await.unwrap(),
            Some(note("b", "second"))
        );
        assert!(NOTES.exists(&store, "a").await.unwrap());
        assert!(NOTES.remove(&store, "a").await.unwrap());
        assert!(!NOTES.remove(&store, "a").await.unwrap());
        assert!(!NOTES.exists(&store, "a").await.unwrap());

        NOTES
            .replace_all(&store, &[note("z", "only")])
            .await
            .unwrap();
        assert_eq!(NOTES.list(&store).await.unwrap(), vec![note("z", "only")]);
    }

    #[tokio::test]
    async fn a_body_that_does_not_parse_is_reported_with_its_kind() {
        let store = FakeStateStore::new();
        store.put_record("note", "x", "{ not json").await.unwrap();
        let error = NOTES.list(&store).await.unwrap_err();
        assert!(error.to_string().contains("note"), "{error}");
    }
}
