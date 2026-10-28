use ox_translate::snakemake::configfile::parse_yaml_config;

#[test]
fn rejects_invalid_duplicate_and_multiple_documents() {
    for yaml in ["samples: [", "a: 1\na: 2\n", "a: 1\n---\nb: 2\n"] {
        assert!(parse_yaml_config(yaml).is_err(), "accepted {yaml:?}");
    }
}

#[test]
fn preserves_scalar_order_and_filters_complex_values() {
    let entries = parse_yaml_config(
        "1: ignored\nnull_value: null\nname: 'true'\nratio: 1.0\nvalues: [true, 2, 0.5, null, {nested: value}, [nested]]\nempty: []\nnested: {key: value}\n",
    )
    .unwrap();
    let actual: Vec<_> = entries
        .iter()
        .map(|entry| (entry.key.as_str(), entry.values.clone()))
        .collect();
    assert_eq!(
        actual,
        vec![
            ("name", vec!["true".to_owned()]),
            ("ratio", vec!["1.0".to_owned()]),
            (
                "values",
                vec!["true".to_owned(), "2".to_owned(), "0.5".to_owned()]
            ),
        ]
    );
}

#[test]
fn resolves_scalar_and_sequence_aliases() {
    let entries = parse_yaml_config(
        "name: &name sample\ncopy: *name\nvalues: &values [1, false]\nmore: *values\n",
    )
    .unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[1].values, vec!["sample"]);
    assert_eq!(entries[3].values, vec!["1", "false"]);
}

#[test]
fn ignores_non_mapping_documents() {
    for yaml in ["", "null", "value", "[a, b]"] {
        assert!(parse_yaml_config(yaml).unwrap().is_empty());
    }
}
