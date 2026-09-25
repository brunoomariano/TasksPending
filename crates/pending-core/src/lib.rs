pub mod config;
pub mod model;

pub use config::{AppConfig, SourceConfig};
pub use model::{
    CardSeverity, DashboardSnapshot, Lane, PendingCard, Section, SourceHealth, SourceStatus,
    sample_snapshot,
};
