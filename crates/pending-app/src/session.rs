//! What lasts as long as the window and no longer: the alerts set on
//! cards.

use crate::alert::Alerts;

#[derive(Debug, Default)]
pub struct Session {
    pub alerts: Alerts,
}
