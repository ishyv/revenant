mod support;
use revenant_sdk::{Contract, WireProjection};
use serde_json::json;

/// Nominal engine identifier; no domain serialization traits are required.
#[derive(Debug, PartialEq, WireProjection)]
#[wire(id)]
struct NodeId(String);

/// Portable summary.
#[derive(Debug, PartialEq, WireProjection)]
struct Summary {
    /// Exact byte total.
    #[wire(decimal, rename = "byteTotal")]
    bytes: u64,
    #[wire(decimal)]
    delta: i64,
}

#[derive(Debug, PartialEq, WireProjection)]
struct Record {
    id: NodeId,
    summary: Summary,
    optional: Option<Summary>,
    children: Vec<Summary>,
}

#[derive(WireProjection)]
struct CapabilityRecord {
    resource: revenant_sdk::Resource<NativeState>,
    #[wire(decimal)]
    samples: Option<Vec<u64>>,
}

// Native state remains entirely outside the wire contract.
struct NativeState;

#[test]
fn generated_containers_preserve_resource_visitors_and_optional_decimal_values() {
    let handle = revenant_sdk::Handle {
        scope: "issued-scope".into(),
        id: "resource-1".into(),
        generation: 1,
        kind: "native".into(),
    };
    let wire = CapabilityRecord {
        resource: revenant_sdk::Resource::from_handle(handle.clone()),
        samples: Some(vec![0, u64::MAX]),
    }
    .into_wire();
    let value = serde_json::to_value(&wire).unwrap();
    assert_eq!(value["samples"], json!(["0", "18446744073709551615"]));
    let mut handles = Vec::new();
    revenant_sdk::WireSafe::visit_handles(&wire, &mut |handle| {
        handles.push(handle.clone());
        Ok(())
    })
    .unwrap();
    assert_eq!(handles, [handle]);
    let restored = CapabilityRecord::from_wire(serde_json::from_value(value).unwrap());
    assert_eq!(restored.samples.unwrap(), [0, u64::MAX]);
}

#[test]
fn generated_projection_removes_conversion_boilerplate_and_round_trips_wide_boundaries() {
    let source = Record {
        id: NodeId("node-17".into()),
        summary: Summary {
            bytes: u64::MAX,
            delta: i64::MIN,
        },
        optional: None,
        children: vec![Summary {
            bytes: 0,
            delta: i64::MAX,
        }],
    };
    let wire = source.into_wire();
    let value = serde_json::to_value(&wire).unwrap();
    assert_eq!(
        value,
        json!({"id":"node-17","summary":{"byteTotal":"18446744073709551615","delta":"-9223372036854775808"},"children":[{"byteTotal":"0","delta":"9223372036854775807"}]})
    );
    let definition = RecordWire::definition().unwrap();
    assert!(definition.typescript.contains("NodeIdWire"));
    assert!(definition.typescript.contains("Exact byte total"));
    revenant_sdk::validate_json(&definition.schema, &value).unwrap();
    let decoded = serde_json::from_value::<RecordWire>(value).unwrap();
    let restored = Record::from_wire(decoded);
    assert_eq!(restored.summary.bytes, u64::MAX);
    assert_eq!(restored.summary.delta, i64::MIN);
    assert_eq!(restored.children[0].delta, i64::MAX);
    assert_eq!(restored.id, NodeId("node-17".into()));
}
