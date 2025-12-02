use serde::Deserialize;

pub mod omniston_models;

#[derive(Debug, Clone, Deserialize, Default, Eq, PartialEq)]
pub enum QuoteSide {
    Ask,

    #[default]
    Bid,
}
