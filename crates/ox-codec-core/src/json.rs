//! JSON codec — re-exports the JSON format codec from [`crate::codec`].
//!
//! This module exists for backward compatibility. The JSON codec is defined
//! as a const in [`crate::codec::JSON`] alongside all other built-in codecs.

pub use crate::codec::JSON;
