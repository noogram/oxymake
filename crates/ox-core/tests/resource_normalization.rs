use std::collections::BTreeMap;

use ox_core::OrderedFloat;
use ox_core::model::ResourceValue;
use ox_core::resource::{NormalizedResources, TokenAmount, normalize_resources};

fn normalize(
    items: &[(&str, ResourceValue)],
) -> Result<NormalizedResources, impl std::error::Error> {
    let resources = items
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    normalize_resources(&resources)
}

#[test]
fn canonical_aliases_have_one_checked_vocabulary() {
    for key in ["cpu", "cpus"] {
        let normalized = normalize(&[(key, ResourceValue::Int(2))]).unwrap();
        assert_eq!(normalized.cpu.unwrap().ten_thousandths(), 20_000);
    }
    for key in ["gpu", "gpus"] {
        let normalized = normalize(&[(key, ResourceValue::Float(OrderedFloat(0.5)))]).unwrap();
        assert_eq!(normalized.gpu.unwrap().ten_thousandths(), 5_000);
    }
    for key in ["mem", "memory"] {
        let normalized = normalize(&[(key, ResourceValue::Str("1GiB".into()))]).unwrap();
        assert_eq!(normalized.memory_bytes, Some(1_073_741_824));
    }
    assert_eq!(
        normalize(&[("mem_mb", ResourceValue::Int(1))])
            .unwrap()
            .memory_bytes,
        Some(1_048_576)
    );
    assert_eq!(
        normalize(&[("mem_gb", ResourceValue::Int(1))])
            .unwrap()
            .memory_bytes,
        Some(1_073_741_824)
    );
}

#[test]
fn equivalent_memory_declarations_are_exactly_one_gibibyte() {
    for (key, value) in [
        ("memory", ResourceValue::Str("1GiB".into())),
        ("memory", ResourceValue::Str("1GB".into())),
        ("mem_mb", ResourceValue::Int(1024)),
        ("mem_gb", ResourceValue::Int(1)),
    ] {
        assert_eq!(
            normalize(&[(key, value)]).unwrap().memory_bytes,
            Some(1_073_741_824),
            "{key}"
        );
    }
    assert_eq!(
        normalize(&[("mem_mb", ResourceValue::Float(OrderedFloat(0.5)))])
            .unwrap()
            .memory_bytes,
        Some(524_288)
    );
}

#[test]
fn memory_strings_accept_all_binary_unit_spellings() {
    for (suffix, multiplier) in [
        ("K", 1_u64 << 10),
        ("KB", 1_u64 << 10),
        ("KiB", 1_u64 << 10),
        ("M", 1_u64 << 20),
        ("MB", 1_u64 << 20),
        ("MiB", 1_u64 << 20),
        ("G", 1_u64 << 30),
        ("GB", 1_u64 << 30),
        ("GiB", 1_u64 << 30),
        ("T", 1_u64 << 40),
        ("TB", 1_u64 << 40),
        ("TiB", 1_u64 << 40),
    ] {
        assert_eq!(
            normalize(&[("memory", ResourceValue::Str(format!("1{suffix}")))])
                .unwrap()
                .memory_bytes,
            Some(multiplier),
            "{suffix}"
        );
    }
}

#[test]
fn aliases_and_custom_spellings_cannot_overwrite_each_other() {
    for declarations in [
        vec![
            ("cpu", ResourceValue::Int(1)),
            ("cpus", ResourceValue::Int(1)),
        ],
        vec![
            ("cpu", ResourceValue::Int(1)),
            ("cpus", ResourceValue::Int(2)),
        ],
        vec![
            ("metal", ResourceValue::Int(1)),
            ("custom:metal", ResourceValue::Int(1)),
        ],
    ] {
        assert!(normalize(&declarations).is_err());
    }
}

#[test]
fn explicit_custom_prefix_keeps_reserved_spelling_custom() {
    let normalized = normalize(&[("custom:mem_mb", ResourceValue::Int(3))]).unwrap();
    assert_eq!(
        normalized.custom.get("mem_mb").unwrap().ten_thousandths(),
        30_000
    );
    assert_eq!(normalized.memory_bytes, None);
}

#[test]
fn malformed_fractional_and_non_finite_values_are_errors() {
    for declarations in [
        vec![("memory", ResourceValue::Str("1XB".into()))],
        vec![("mem_mb", ResourceValue::Str("1G".into()))],
        vec![("memory", ResourceValue::Float(OrderedFloat(0.5)))],
        vec![("cpu", ResourceValue::Int(-1))],
        vec![("gpu", ResourceValue::Float(OrderedFloat(f64::NAN)))],
        vec![("gpu", ResourceValue::Float(OrderedFloat(f64::INFINITY)))],
        vec![("memory", ResourceValue::Str("18446744073709551616".into()))],
        vec![("", ResourceValue::Int(1))],
        vec![("custom:", ResourceValue::Int(1))],
        vec![("metal", ResourceValue::Float(OrderedFloat(0.00001)))],
    ] {
        assert!(normalize(&declarations).is_err(), "{declarations:?}");
    }
}

#[test]
fn token_arithmetic_is_exact_to_one_ten_thousandth() {
    let tenth = normalize(&[("cpu", ResourceValue::Float(OrderedFloat(0.1)))])
        .unwrap()
        .cpu
        .unwrap();
    let sum = (0..10).try_fold(TokenAmount::ZERO, |sum, _| sum.checked_add(tenth));
    assert_eq!(sum.unwrap(), TokenAmount::from_ten_thousandths(10_000));
}

#[test]
fn fractional_gpu_is_valid_in_the_shared_normal_form() {
    let normalized = normalize(&[("gpu", ResourceValue::Float(OrderedFloat(0.5)))]).unwrap();
    assert_eq!(normalized.gpu.unwrap().ten_thousandths(), 5_000);
}

#[test]
fn a_broken_exponent_reads_as_malformed_not_as_overflow() {
    // "1e" has no exponent digits: the value is unreadable, not too large.
    // The distinction is what the user is told to fix.
    let declarations = [("memory", ResourceValue::Str("1e".into()))];
    let rendered = normalize(&declarations).unwrap_err().to_string();
    assert!(
        rendered.contains("malformed"),
        "expected a malformed-value diagnostic, got: {rendered}"
    );
    assert!(
        !rendered.contains("overflow"),
        "a broken exponent must not be reported as an overflow: {rendered}"
    );
}
