//! The example surfaces upstream publishes beside each basic catalog, each
//! corpus against the schemas and the catalog of its own version.
//!
//! The files are under `tests/conformance/`, at the paths they have in the
//! upstream repository, copied from the commit that directory records.

#![cfg(feature = "schema-validation")]

use ag_ui_a2ui::processor::MessageProcessor;
use ag_ui_a2ui::schema::SchemaBundle;
use ag_ui_a2ui::schema_validation::{SchemaValidator, SurfaceValidator};
use ag_ui_a2ui::{A2uiVersion, Catalog};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

/// One corpus: where it is under `tests/conformance/`, the version it is
/// written in, and how many examples it holds.
const CORPORA: [(&str, A2uiVersion, usize); 3] = [
    (
        "specification/v0_9/catalogs/basic/examples",
        A2uiVersion::V0_9,
        44,
    ),
    (
        "specification/v0_9_1/catalogs/basic/examples",
        A2uiVersion::V0_9_1,
        43,
    ),
    ("catalogs/basic/v1/examples", A2uiVersion::V1_0, 43),
];

/// The examples of one corpus, by file name.
fn examples(directory: &str, count: usize) -> Vec<(String, Value)> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/conformance")
        .join(directory);
    let mut files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{}: {error}", directory.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    assert_eq!(
        files.len(),
        count,
        "{}: update the pinned corpus deliberately",
        directory.display()
    );
    files
        .into_iter()
        .map(|path| {
            let example = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            (path.display().to_string(), example)
        })
        .collect()
}

/// An example that draws components its basic catalog does not define. It
/// says so itself, in `customComponents`, and is an example of an agent
/// writing for a catalog of its own; the basic catalog cannot hold it.
fn draws_from_another_catalog(example: &Value) -> bool {
    example
        .get("customComponents")
        .and_then(Value::as_array)
        .is_some_and(|names| !names.is_empty())
}

#[test]
fn every_official_example_is_accepted_by_a_processor_of_its_version() {
    for (directory, version, count) in CORPORA {
        let bundle = SchemaBundle::for_version(version).unwrap();
        let catalog = Catalog::basic_for(version);
        let mut run = 0;
        for (path, example) in examples(directory, count) {
            if draws_from_another_catalog(&example) {
                continue;
            }
            let mut processor = MessageProcessor::new().accepting_version(version);
            processor.register(catalog.clone()).unwrap();
            processor
                .register_schema(
                    &catalog.catalog_id,
                    SchemaValidator::new(&bundle, &BTreeMap::new()).unwrap(),
                )
                .unwrap();
            for (index, message) in example["messages"].as_array().unwrap().iter().enumerate() {
                processor
                    .process(message)
                    .unwrap_or_else(|rejection| panic!("{path} message {index}: {rejection}"));
            }
            for surface in processor.surfaces().surfaces() {
                // The v0.9.1 examples are still written as `v0.9`, which the
                // family reads as its own.
                assert!(version.accepts(surface.spoken_version()), "{path}");
                // An example that redraws a panel leaves the placeholder it
                // replaced unreachable, which a report notes and does not
                // refuse.
                let report = processor.report(&surface.surface_id).unwrap();
                assert!(report.is_valid(), "{path}: {:?}", report.errors);
            }
            run += 1;
        }
        // One example of the v0.9 corpus is written for a catalog of its own.
        let custom = usize::from(version == A2uiVersion::V0_9);
        assert_eq!(run, count - custom, "{directory}");
    }
}

#[test]
fn every_official_example_passes_schema_and_state_validation() {
    for (directory, version, count) in CORPORA {
        let bundle = SchemaBundle::for_version(version).unwrap();
        for (path, example) in examples(directory, count) {
            if draws_from_another_catalog(&example) {
                continue;
            }
            let mut validator = SurfaceValidator::new();
            validator.register(&bundle, &BTreeMap::new()).unwrap();
            for (index, message) in example["messages"].as_array().unwrap().iter().enumerate() {
                validator
                    .push(message)
                    .unwrap_or_else(|error| panic!("{path} message {index}: {error}"));
            }
            validator
                .finish()
                .unwrap_or_else(|error| panic!("{path} final state: {error}"));
        }
    }
}
