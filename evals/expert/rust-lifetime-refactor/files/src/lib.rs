//! kvlog: parse `key=value` log lines and query them.
//!
//! A log is text. Each line holds fields separated by spaces:
//!
//! ```text
//! ts=1700000000 level=warn msg="disk almost full" host=db-1
//! ```
//!
//! See DESIGN.md for the format and for the planned zero-copy API.

pub mod error;
pub mod index;
pub mod lexer;
pub mod parser;
pub mod record;
pub mod stats;

pub use error::{ErrorKind, ParseError};
pub use index::Index;
pub use parser::parse;
pub use record::{Field, Record};
pub use stats::distinct_keys;
