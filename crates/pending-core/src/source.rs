//! The port every pending-work source implements.

use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::PendingCard;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A refreshable provider of pending cards.
///
/// Sources decide the section of each card; the name and lane come from the
/// source's configuration. The future is boxed so different sources can live in one
/// `Vec<Box<dyn PendingSource>>`.
pub trait PendingSource: Send + Sync {
    fn refresh(&self) -> BoxFuture<'_, Result<SourceBatch, SourceError>>;
}

/// One card and the section it belongs to inside the source's lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceItem {
    pub section: String,
    pub card: PendingCard,
}

/// What a refresh produced. Non-empty `warnings` mean the data is partial and
/// the source shows as degraded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceBatch {
    pub items: Vec<SourceItem>,
    pub warnings: Vec<String>,
}

/// A refresh that produced nothing usable.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
pub struct SourceError {
    message: String,
    retry_at: Option<DateTime<Utc>>,
}

impl SourceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_at: None,
        }
    }

    /// The provider said not to try again before `at` (e.g. a rate limit).
    pub fn with_retry_at(mut self, at: DateTime<Utc>) -> Self {
        self.retry_at = Some(at);
        self
    }

    pub fn retry_at(&self) -> Option<DateTime<Utc>> {
        self.retry_at
    }
}
