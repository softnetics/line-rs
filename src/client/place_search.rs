//! Place search for location messages
//! (`line.placesearch.ExternalInterlockService` over gRPC).
//!
//! Field numbers and message shapes were reverse-engineered from the LINE
//! Android app (26.x) and have not been checked against a live server.
//! Encoding and decoding are by hand via [`crate::protobuf`]. The calls use
//! the app's default gRPC metadata (`x-line-access`), with no channel token.

use crate::base::BaseClient;
use crate::error::Result;
use crate::protobuf::{Reader, Writer, packed_varints};

const SERVICE: &str = "/line.placesearch.ExternalInterlockService";

/// `PlaceSearchProvider`: the map data source LINE queries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PlaceSearchProvider {
    #[default]
    Unspecified,
    Google,
    Foursquare,
    YahooJapan,
    Kingway,
    Unknown(i32),
}

impl From<i32> for PlaceSearchProvider {
    fn from(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::Google,
            2 => Self::Foursquare,
            3 => Self::YahooJapan,
            4 => Self::Kingway,
            n => Self::Unknown(n),
        }
    }
}

impl From<PlaceSearchProvider> for i32 {
    fn from(p: PlaceSearchProvider) -> i32 {
        match p {
            PlaceSearchProvider::Unspecified => 0,
            PlaceSearchProvider::Google => 1,
            PlaceSearchProvider::Foursquare => 2,
            PlaceSearchProvider::YahooJapan => 3,
            PlaceSearchProvider::Kingway => 4,
            PlaceSearchProvider::Unknown(n) => n,
        }
    }
}

/// A place search: `keyword` near (`latitude`, `longitude`) within
/// `radius` metres, results localised for `client_locale` (e.g. `"th_TH"`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaceSearchQuery {
    pub provider: PlaceSearchProvider,
    pub keyword: String,
    pub client_locale: String,
    pub latitude: f64,
    pub longitude: f64,
    pub radius: i32,
}

impl PlaceSearchQuery {
    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.enumeration(1, self.provider.into());
        w.string(2, &self.keyword);
        w.string(3, &self.client_locale);
        w.double(4, self.latitude);
        w.double(5, self.longitude);
        w.int32(6, self.radius);
        w.into_bytes()
    }
}

/// A search result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Place {
    pub name: String,
    pub address: String,
    pub latitude: f64,
    pub longitude: f64,
    pub category_id: String,
}

impl Place {
    fn decode(buf: &[u8]) -> Result<Self> {
        let mut p = Place::default();
        let mut r = Reader::new(buf);
        while let Some(f) = r.next_field()? {
            match f.number {
                1 => p.name = f.value.as_str()?.to_string(),
                2 => p.address = f.value.as_str()?.to_string(),
                3 => p.latitude = f.value.as_f64()?,
                4 => p.longitude = f.value.as_f64()?,
                5 => p.category_id = f.value.as_str()?.to_string(),
                _ => {}
            }
        }
        Ok(p)
    }
}

fn decode_places(buf: &[u8]) -> Result<Vec<Place>> {
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while let Some(f) = r.next_field()? {
        if f.number == 1 {
            out.push(Place::decode(f.value.as_bytes()?)?);
        }
    }
    Ok(out)
}

/// Repeated enum field 1, packed or not.
fn decode_providers(buf: &[u8]) -> Result<Vec<PlaceSearchProvider>> {
    let mut out = Vec::new();
    let mut r = Reader::new(buf);
    while let Some(f) = r.next_field()? {
        if f.number != 1 {
            continue;
        }
        match f.value {
            crate::protobuf::Value::Len(b) => out.extend(
                packed_varints(b)?
                    .into_iter()
                    .map(|v| PlaceSearchProvider::from(v as i32)),
            ),
            v => out.push(v.as_i32()?.into()),
        }
    }
    Ok(out)
}

impl BaseClient {
    /// `GetPlaceSearchInfo`: places matching a query.
    pub async fn search_places(&self, query: &PlaceSearchQuery) -> Result<Vec<Place>> {
        let res = self
            .grpc_unary(&format!("{SERVICE}/GetPlaceSearchInfo"), &query.encode())
            .await?;
        decode_places(&res)
    }

    /// `GetPlaceSearchProviderList`: the providers available in `region`
    /// (a country code such as `"TH"`), in the app's order of preference.
    pub async fn place_search_providers(&self, region: &str) -> Result<Vec<PlaceSearchProvider>> {
        let mut w = Writer::new();
        w.string(1, region);
        let res = self
            .grpc_unary(
                &format!("{SERVICE}/GetPlaceSearchProviderList"),
                w.as_bytes(),
            )
            .await?;
        decode_providers(&res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding() {
        let q = PlaceSearchQuery {
            provider: PlaceSearchProvider::Google,
            keyword: "cafe".into(),
            client_locale: "th_TH".into(),
            latitude: 13.75,
            longitude: 100.5,
            radius: 500,
        };
        let bytes = q.encode();
        let mut r = Reader::new(&bytes);
        let mut seen = Vec::new();
        while let Some(f) = r.next_field().unwrap() {
            seen.push(f.number);
            if f.number == 4 {
                assert_eq!(f.value.as_f64().unwrap(), 13.75);
            }
        }
        assert_eq!(seen, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn decodes_places_and_providers() {
        let mut place = Writer::new();
        place.string(1, "Cafe");
        place.double(3, 13.75);
        place.double(4, 100.5);
        let mut res = Writer::new();
        res.message(1, place.as_bytes());
        let places = decode_places(res.as_bytes()).unwrap();
        assert_eq!(places[0].name, "Cafe");
        assert_eq!(places[0].longitude, 100.5);

        // Packed [1, 2].
        let mut res = Writer::new();
        res.bytes(1, &[1, 2]);
        assert_eq!(
            decode_providers(res.as_bytes()).unwrap(),
            [PlaceSearchProvider::Google, PlaceSearchProvider::Foursquare]
        );
    }
}
