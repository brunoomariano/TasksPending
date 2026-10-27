//! The weather beside the clock, as the daemon reports it and the app shows
//! it. The texts are shown as Omarchy formats them, e.g. `29°C` and
//! `←15km/h`.

use serde::{Deserialize, Serialize};

/// The body of `GET /api/v1/weather`: `{"available": false}`, or
/// `{"available": true}` with the fields of [`WeatherReport`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeatherAnswer {
    pub available: bool,
    #[serde(flatten)]
    pub report: Option<WeatherReport>,
}

impl WeatherAnswer {
    /// No weather to show.
    pub const UNAVAILABLE: Self = Self {
        available: false,
        report: None,
    };

    /// Weather to show.
    pub fn available(report: WeatherReport) -> Self {
        Self {
            available: true,
            report: Some(report),
        }
    }

    /// The report, when the weather is available and well-formed.
    pub fn report(&self) -> Option<&WeatherReport> {
        self.report.as_ref().filter(|report| {
            self.available
                && !report.place.is_empty()
                && !report.temperature.is_empty()
                && !report.wind.is_empty()
        })
    }
}

/// The current weather at a place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeatherReport {
    pub place: String,
    pub temperature: String,
    pub wind: String,
    pub condition: WeatherCondition,
}

/// The conditions `omarchy-weather-icon` tells apart, as stable keywords
/// for the app to pick an icon from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WeatherCondition {
    ClearDay,
    ClearNight,
    PartlyCloudyDay,
    PartlyCloudyNight,
    Cloudy,
    Fog,
    Rain,
    Sleet,
    Snow,
    Thunder,
    /// The icon command failed, printed a glyph this version doesn't know,
    /// or the daemon is newer and sent a keyword this version doesn't know.
    #[serde(other)]
    Unknown,
}

impl WeatherCondition {
    /// A spoken name for the condition, for tooltips and screen readers.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClearDay => "Clear",
            Self::ClearNight => "Clear night",
            Self::PartlyCloudyDay => "Partly cloudy",
            Self::PartlyCloudyNight => "Partly cloudy night",
            Self::Cloudy => "Cloudy",
            Self::Fog => "Fog",
            Self::Rain => "Rain",
            Self::Sleet => "Sleet",
            Self::Snow => "Snow",
            Self::Thunder => "Thunderstorm",
            Self::Unknown => "Weather",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire format is flat: the flag and the report's fields side by
    /// side, and `{"available": false}` alone when there is nothing.
    #[test]
    fn answers_serialize_flat_and_read_back() {
        let answer = WeatherAnswer::available(WeatherReport {
            place: "Recife".to_owned(),
            temperature: "29°C".to_owned(),
            wind: "←15km/h".to_owned(),
            condition: WeatherCondition::PartlyCloudyDay,
        });
        let json = serde_json::to_value(&answer).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "available": true,
                "place": "Recife",
                "temperature": "29°C",
                "wind": "←15km/h",
                "condition": "partly-cloudy-day"
            })
        );
        let back: WeatherAnswer = serde_json::from_value(json).unwrap();
        assert_eq!(back, answer);
        assert_eq!(back.report().map(|r| r.place.as_str()), Some("Recife"));

        let none = serde_json::to_value(WeatherAnswer::UNAVAILABLE).unwrap();
        assert_eq!(none, serde_json::json!({"available": false}));
        let back: WeatherAnswer = serde_json::from_value(none).unwrap();
        assert_eq!(back.report(), None);
    }

    /// A keyword from a newer daemon is read as unknown, not as an error,
    /// and an answer that claims availability without a place shows nothing.
    #[test]
    fn unknown_conditions_and_empty_reports_are_tolerated() {
        let answer: WeatherAnswer = serde_json::from_str(
            r#"{"available": true, "place": "Recife", "temperature": "29°C", "wind": "0km/h", "condition": "hail"}"#,
        )
        .unwrap();
        assert_eq!(
            answer.report().map(|r| r.condition),
            Some(WeatherCondition::Unknown)
        );

        let answer: WeatherAnswer = serde_json::from_str(
            r#"{"available": true, "place": "", "temperature": "29°C", "wind": "0km/h", "condition": "fog"}"#,
        )
        .unwrap();
        assert_eq!(answer.report(), None);
    }
}
