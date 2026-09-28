//! Checked normalization of declared resource keys and values.
//!
//! The raw [`ResourceValue`] maps remain part of
//! the workflow and job model. This module computes a canonical side-car form
//! for admission and executor adapters without changing serialization or
//! cache inputs.

use std::collections::BTreeMap;
use std::fmt;

use crate::model::ResourceValue;

/// Fixed-point denominator used for CPU, GPU, and custom token demands.
pub const TOKEN_SCALE: u64 = 10_000;

/// An exact count demand, measured in ten-thousandths of a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TokenAmount(u64);

impl TokenAmount {
    /// Zero tokens.
    pub const ZERO: Self = Self(0);

    /// Construct an amount from its fixed-point representation.
    pub const fn from_ten_thousandths(value: u64) -> Self {
        Self(value)
    }

    /// Return the fixed-point representation.
    pub const fn ten_thousandths(self) -> u64 {
        self.0
    }

    /// Add two exact token amounts, returning `None` on overflow.
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    /// Convert to the representation accepted by executor APIs such as Ray.
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / TOKEN_SCALE as f64
    }
}

impl fmt::Display for TokenAmount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let whole = self.0 / TOKEN_SCALE;
        let fractional = self.0 % TOKEN_SCALE;
        if fractional == 0 {
            write!(f, "{whole}")
        } else {
            write!(f, "{whole}.{fractional:04}")
        }
    }
}

/// Canonical identity of a declared resource key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CanonicalResource {
    /// CPU tokens (`cpu` or `cpus`).
    Cpu,
    /// GPU tokens (`gpu` or `gpus`).
    Gpu,
    /// Memory in bytes (`mem`, `memory`, `mem_mb`, or `mem_gb`).
    Memory,
    /// A case-sensitive custom token.
    Custom(String),
}

impl fmt::Display for CanonicalResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => f.write_str("cpu"),
            Self::Gpu => f.write_str("gpu"),
            Self::Memory => f.write_str("memory"),
            Self::Custom(name) => write!(f, "custom:{name}"),
        }
    }
}

/// Canonical resource values computed alongside the raw declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizedResources {
    /// Exact CPU token demand.
    pub cpu: Option<TokenAmount>,
    /// Exact GPU token demand.
    pub gpu: Option<TokenAmount>,
    /// Exact memory demand in bytes.
    pub memory_bytes: Option<u64>,
    /// Exact case-sensitive custom token demands.
    pub custom: BTreeMap<String, TokenAmount>,
}

/// Why a resource declaration could not be normalized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceValueErrorKind {
    /// The value is negative.
    Negative,
    /// The value is NaN or infinite.
    NonFinite,
    /// The spelling is not a supported number or memory unit.
    Malformed,
    /// A count has precision finer than one ten-thousandth.
    TooPrecise,
    /// A byte calculation produced a fractional byte.
    FractionalByte,
    /// The exact result does not fit in the normalized representation.
    Overflow,
}

impl fmt::Display for ResourceValueErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Negative => f.write_str("negative values are not allowed"),
            Self::NonFinite => f.write_str("NaN and infinite values are not allowed"),
            Self::Malformed => f.write_str("the value or unit is malformed"),
            Self::TooPrecise => f.write_str("count precision is finer than one ten-thousandth"),
            Self::FractionalByte => f.write_str("memory must resolve to whole bytes"),
            Self::Overflow => f.write_str("the normalized value overflows"),
        }
    }
}

/// Checked resource-normalization failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    /// A bare or explicitly prefixed custom resource has no name.
    #[error("resource name must not be empty")]
    EmptyName,
    /// Two raw keys resolve to the same canonical resource.
    #[error("resource {resource} is declared more than once via {first_key:?} and {second_key:?}")]
    Duplicate {
        /// Canonical resource identity.
        resource: CanonicalResource,
        /// First raw spelling encountered.
        first_key: String,
        /// Conflicting raw spelling.
        second_key: String,
    },
    /// A resource value is invalid for its canonical type.
    #[error("invalid value {value:?} for resource {key:?}: {kind}")]
    InvalidValue {
        /// Raw resource key.
        key: String,
        /// Raw value rendered without changing it.
        value: String,
        /// Classified reason for rejection.
        kind: ResourceValueErrorKind,
    },
}

/// Resolve a raw resource key to its canonical identity.
pub fn canonicalize_resource_key(key: &str) -> Result<CanonicalResource, ResourceError> {
    if key.trim().is_empty() {
        return Err(ResourceError::EmptyName);
    }
    if let Some(name) = key.strip_prefix("custom:") {
        if name.trim().is_empty() {
            return Err(ResourceError::EmptyName);
        }
        return Ok(CanonicalResource::Custom(name.to_owned()));
    }
    Ok(match key {
        "cpu" | "cpus" => CanonicalResource::Cpu,
        "gpu" | "gpus" => CanonicalResource::Gpu,
        "mem" | "memory" | "mem_mb" | "mem_gb" => CanonicalResource::Memory,
        name => CanonicalResource::Custom(name.to_owned()),
    })
}

/// Normalize a raw declaration map without modifying it.
pub fn normalize_resources(
    resources: &BTreeMap<String, ResourceValue>,
) -> Result<NormalizedResources, ResourceError> {
    let mut normalized = NormalizedResources::default();
    let mut seen = BTreeMap::<CanonicalResource, String>::new();

    for (key, value) in resources {
        let canonical = canonicalize_resource_key(key)?;
        if let Some(first_key) = seen.insert(canonical.clone(), key.clone()) {
            return Err(ResourceError::Duplicate {
                resource: canonical,
                first_key,
                second_key: key.clone(),
            });
        }

        match canonical {
            CanonicalResource::Cpu => normalized.cpu = Some(parse_count(key, value)?),
            CanonicalResource::Gpu => normalized.gpu = Some(parse_count(key, value)?),
            CanonicalResource::Memory => normalized.memory_bytes = Some(parse_memory(key, value)?),
            CanonicalResource::Custom(name) => {
                normalized.custom.insert(name, parse_count(key, value)?);
            }
        }
    }

    Ok(normalized)
}

fn parse_count(key: &str, value: &ResourceValue) -> Result<TokenAmount, ResourceError> {
    let decimal = decimal_from_value(key, value)?;
    scale_decimal(
        decimal,
        TOKEN_SCALE as u128,
        ResourceValueErrorKind::TooPrecise,
    )
    .and_then(|value| u64::try_from(value).map_err(|_| ResourceValueErrorKind::Overflow))
    .map(TokenAmount)
    .map_err(|kind| invalid_value(key, value, kind))
}

fn parse_memory(key: &str, value: &ResourceValue) -> Result<u64, ResourceError> {
    let (decimal, multiplier) = match key {
        "mem_mb" => (decimal_from_value(key, value)?, 1_u128 << 20),
        "mem_gb" => (decimal_from_value(key, value)?, 1_u128 << 30),
        _ => match value {
            ResourceValue::Str(value) => {
                parse_memory_string(value).map_err(|kind| invalid_value(key, value, kind))?
            }
            _ => (decimal_from_value(key, value)?, 1),
        },
    };

    scale_decimal(decimal, multiplier, ResourceValueErrorKind::FractionalByte)
        .and_then(|value| u64::try_from(value).map_err(|_| ResourceValueErrorKind::Overflow))
        .map_err(|kind| invalid_value(key, value, kind))
}

fn parse_memory_string(value: &str) -> Result<(Decimal, u128), ResourceValueErrorKind> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ResourceValueErrorKind::Malformed);
    }
    let upper = trimmed.to_ascii_uppercase();
    let units = [
        ("KIB", 1_u128 << 10),
        ("MIB", 1_u128 << 20),
        ("GIB", 1_u128 << 30),
        ("TIB", 1_u128 << 40),
        ("KB", 1_u128 << 10),
        ("MB", 1_u128 << 20),
        ("GB", 1_u128 << 30),
        ("TB", 1_u128 << 40),
        ("K", 1_u128 << 10),
        ("M", 1_u128 << 20),
        ("G", 1_u128 << 30),
        ("T", 1_u128 << 40),
        ("B", 1),
    ];
    for (suffix, multiplier) in units {
        if upper.ends_with(suffix) {
            let number = &trimmed[..trimmed.len() - suffix.len()];
            return Ok((parse_decimal(number)?, multiplier));
        }
    }
    Ok((parse_decimal(trimmed)?, 1))
}

fn decimal_from_value(key: &str, value: &ResourceValue) -> Result<Decimal, ResourceError> {
    match value {
        ResourceValue::Int(value) => {
            if *value < 0 {
                Err(invalid_value(key, value, ResourceValueErrorKind::Negative))
            } else {
                Ok(Decimal {
                    coefficient: *value as u128,
                    scale: 0,
                })
            }
        }
        ResourceValue::Float(value) => {
            let value = value.into_inner();
            if !value.is_finite() {
                return Err(invalid_value(key, value, ResourceValueErrorKind::NonFinite));
            }
            if value < 0.0 {
                return Err(invalid_value(key, value, ResourceValueErrorKind::Negative));
            }
            parse_decimal(&value.to_string()).map_err(|kind| invalid_value(key, value, kind))
        }
        ResourceValue::Str(value) => {
            parse_decimal(value).map_err(|kind| invalid_value(key, value, kind))
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Decimal {
    coefficient: u128,
    /// Number of decimal places; negative values append zeroes.
    scale: i32,
}

fn parse_decimal(input: &str) -> Result<Decimal, ResourceValueErrorKind> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ResourceValueErrorKind::Malformed);
    }
    if input.starts_with('-') {
        return Err(ResourceValueErrorKind::Negative);
    }
    let input = input.strip_prefix('+').unwrap_or(input);
    let mut exponent_parts = input.split(['e', 'E']);
    let mantissa = exponent_parts.next().unwrap_or_default();
    let exponent = exponent_parts
        .next()
        .map(|part| {
            // An exponent that does not parse is a malformed value ("1e"),
            // not a value too large to represent.
            part.parse::<i32>().map_err(|error| match error.kind() {
                std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
                    ResourceValueErrorKind::Overflow
                }
                _ => ResourceValueErrorKind::Malformed,
            })
        })
        .transpose()?
        .unwrap_or(0);
    if exponent_parts.next().is_some() {
        return Err(ResourceValueErrorKind::Malformed);
    }

    let mut coefficient = 0_u128;
    let mut digits = 0_usize;
    let mut fractional_digits = 0_usize;
    let mut saw_dot = false;
    for byte in mantissa.bytes() {
        match byte {
            b'.' if !saw_dot => saw_dot = true,
            b'0'..=b'9' => {
                coefficient = coefficient
                    .checked_mul(10)
                    .and_then(|value| value.checked_add((byte - b'0') as u128))
                    .ok_or(ResourceValueErrorKind::Overflow)?;
                digits += 1;
                if saw_dot {
                    fractional_digits += 1;
                }
            }
            _ => return Err(ResourceValueErrorKind::Malformed),
        }
    }
    if digits == 0 {
        return Err(ResourceValueErrorKind::Malformed);
    }
    let fractional_digits =
        i32::try_from(fractional_digits).map_err(|_| ResourceValueErrorKind::Overflow)?;
    let scale = fractional_digits
        .checked_sub(exponent)
        .ok_or(ResourceValueErrorKind::Overflow)?;
    Ok(Decimal { coefficient, scale })
}

fn scale_decimal(
    decimal: Decimal,
    multiplier: u128,
    fractional_error: ResourceValueErrorKind,
) -> Result<u128, ResourceValueErrorKind> {
    if decimal.coefficient == 0 {
        return Ok(0);
    }
    let mut value = decimal
        .coefficient
        .checked_mul(multiplier)
        .ok_or(ResourceValueErrorKind::Overflow)?;
    if decimal.scale > 0 {
        let divisor = checked_power_of_ten(decimal.scale as u32)?;
        if value % divisor != 0 {
            return Err(fractional_error);
        }
        value /= divisor;
    } else if decimal.scale < 0 {
        let multiplier = checked_power_of_ten(decimal.scale.unsigned_abs())?;
        value = value
            .checked_mul(multiplier)
            .ok_or(ResourceValueErrorKind::Overflow)?;
    }
    Ok(value)
}

fn checked_power_of_ten(exponent: u32) -> Result<u128, ResourceValueErrorKind> {
    10_u128
        .checked_pow(exponent)
        .ok_or(ResourceValueErrorKind::Overflow)
}

fn invalid_value(
    key: &str,
    value: impl fmt::Display,
    kind: ResourceValueErrorKind,
) -> ResourceError {
    ResourceError::InvalidValue {
        key: key.to_owned(),
        value: value.to_string(),
        kind,
    }
}
