//! Reading, watching and validating `config.toml`.

pub mod keys;
pub mod load;
pub mod paths;
pub mod schema;
pub mod watch;
pub mod write;

pub use schema::Config;
