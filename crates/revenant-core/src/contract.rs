//! Contracts must agree across serialization, schema, and TypeScript. Large integer values use decimal wrappers; optional fields are omitted rather than null.
use crate::{Error, Result};
use schemars::JsonSchema;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use ts_rs::{TS, TypeVisitor};

/// Marker for representations audited to be JSON-compatible and numerically lossless.
/// Contract macros require it transitively on every field. External implementations
/// are an explicit trust boundary; custom Serde implementations must agree with TS/schema.
pub trait WireSafe: 'static {
    /// Visits every embedded resource handle for admission validation. Implementations
    /// must traverse nested values and stop at the first visitor error. The default
    /// is valid only for representations containing no handles.
    fn visit_handles(&self, _visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        Ok(())
    }
}
macro_rules! safe_primitives { ($($t:ty),*) => { $(impl WireSafe for $t {})* }; }
safe_primitives!((), bool, String, char, i8, i16, i32, u8, u16, u32, f32, f64);
impl<T: WireSafe> WireSafe for Vec<T> {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        for value in self {
            value.visit_handles(visitor)?;
        }
        Ok(())
    }
}
impl<T: WireSafe> WireSafe for Option<T> {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        if let Some(value) = self {
            value.visit_handles(visitor)?;
        }
        Ok(())
    }
}
impl<T: WireSafe> WireSafe for Box<T> {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        (**self).visit_handles(visitor)
    }
}
impl<T: WireSafe, const N: usize> WireSafe for [T; N] {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        for value in self {
            value.visit_handles(visitor)?;
        }
        Ok(())
    }
}
impl<T: WireSafe> WireSafe for BTreeMap<String, T> {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        for value in self.values() {
            value.visit_handles(visitor)?;
        }
        Ok(())
    }
}
impl<T: WireSafe> WireSafe for std::collections::HashMap<String, T> {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        for value in self.values() {
            value.visit_handles(visitor)?;
        }
        Ok(())
    }
}
impl<A: WireSafe, B: WireSafe> WireSafe for (A, B) {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        self.0.visit_handles(visitor)?;
        self.1.visit_handles(visitor)
    }
}
impl<A: WireSafe, B: WireSafe, C: WireSafe> WireSafe for (A, B, C) {
    fn visit_handles(&self, visitor: &mut dyn FnMut(&crate::Handle) -> Result<()>) -> Result<()> {
        self.0.visit_handles(visitor)?;
        self.1.visit_handles(visitor)?;
        self.2.visit_handles(visitor)
    }
}

/// A wire-safe value with matching Serde, JSON Schema, and TypeScript representations.
/// Implementing WireSafe promises correct nested-handle traversal; schema generation
/// additionally rejects asymmetric serialization and lossy integer contracts.
pub trait Contract: WireSafe + Serialize + DeserializeOwned + JsonSchema + TS {
    /// Produces a standalone TypeScript declaration module and normalized JSON Schema.
    /// Fails for colliding type names, asymmetric serialization, or unsafe integer bounds.
    fn definition() -> Result<TypeDefinition> {
        type_definition::<Self>()
    }
}
impl<T: WireSafe + Serialize + DeserializeOwned + JsonSchema + TS> Contract for T {}

/// Generated declarations and validation schema for one root contract.
#[derive(Clone, Debug, PartialEq, Serialize, serde::Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TypeDefinition {
    /// Root TypeScript type expression, including generic arguments where applicable.
    pub name: String,
    /// Standalone module: root declaration and every transitive TS dependency.
    pub typescript: String,
    /// Normalized draft 2020-12 JSON Schema with explicit small-integer bounds.
    /// Optional object fields use omission rather than null as their boundary representation.
    pub schema: Value,
}

struct Definitions {
    seen: HashSet<std::any::TypeId>,
    declarations: BTreeMap<String, String>,
    error: Option<Error>,
    config: ts_rs::Config,
}
impl TypeVisitor for Definitions {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if !self.seen.insert(std::any::TypeId::of::<T>()) {
            return;
        }
        if T::output_path().is_some() {
            let name = T::ident(&self.config);
            let declaration = format!("export {}", T::decl(&self.config));
            if self
                .declarations
                .get(&name)
                .is_some_and(|old| old != &declaration)
            {
                self.error = Some(Error::new(
                    "contract_collision",
                    format!("Conflicting TypeScript type {name}"),
                ));
            }
            self.declarations.insert(name, declaration);
        }
        T::visit_dependencies(self);
        T::visit_generics(self);
    }
}

/// Generates transitive TypeScript declarations and a root `ContractValue` alias.
/// Returns `contract_collision` for conflicting names or a reserved alias,
/// `asymmetric_contract` for unequal input/output schemas, and `lossy_integer`
/// for integer ranges beyond JavaScript exact representation.
pub fn type_definition<T: Contract + ?Sized>() -> Result<TypeDefinition> {
    let mut definitions = Definitions {
        seen: HashSet::new(),
        declarations: BTreeMap::new(),
        error: None,
        config: ts_rs::Config::default(),
    };
    definitions.visit::<T>();
    if let Some(error) = definitions.error {
        return Err(error);
    }
    let name = T::name(&definitions.config);
    if definitions.declarations.contains_key("ContractValue") {
        return Err(Error::new(
            "contract_collision",
            "ContractValue is reserved for the root contract alias",
        ));
    }
    let declarations = definitions
        .declarations
        .into_values()
        .collect::<Vec<_>>()
        .join("\n");
    let typescript = format!("export type ContractValue = {name};\n{declarations}");
    let input_schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let output_schema = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>();
    let mut input = serde_json::to_value(&input_schema)?;
    let mut schema = serde_json::to_value(&output_schema)?;
    normalize_optional_schema(&mut input);
    normalize_optional_schema(&mut schema);
    if input != schema {
        return Err(Error::new(
            "asymmetric_contract",
            format!("{name} has different input/output serialization"),
        ));
    }
    validate_contract_schema(&schema)?;
    Ok(TypeDefinition {
        name,
        typescript,
        schema,
    })
}

/// An omitted Option is the canonical boundary representation. Serde additionally
/// accepts null, but boundary validation uses the stricter optional, non-null shape.
fn normalize_optional_schema(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            object.remove("default");
            // Schemars expresses small integer ranges through format alone, which
            // JSON Schema validators need not enforce. Emit explicit bounds.
            let bounds =
                object
                    .get("format")
                    .and_then(Value::as_str)
                    .and_then(|format| match format {
                        "uint8" => Some((0, u8::MAX as i64)),
                        "uint16" => Some((0, u16::MAX as i64)),
                        "uint32" => Some((0, u32::MAX as i64)),
                        "int8" => Some((i8::MIN as i64, i8::MAX as i64)),
                        "int16" => Some((i16::MIN as i64, i16::MAX as i64)),
                        "int32" => Some((i32::MIN as i64, i32::MAX as i64)),
                        _ => None,
                    });
            if let Some((minimum, maximum)) = bounds {
                object.entry("minimum").or_insert(minimum.into());
                object.entry("maximum").or_insert(maximum.into());
            }
            let required = object
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                for (name, property) in properties {
                    if !required.iter().any(|key| key == name) {
                        strip_null(property);
                    }
                }
            }
            for child in object.values_mut() {
                normalize_optional_schema(child);
            }
        }
        Value::Array(values) => {
            for value in values {
                normalize_optional_schema(value);
            }
        }
        _ => {}
    }
}
fn strip_null(property: &mut Value) {
    let Some(object) = property.as_object_mut() else {
        return;
    };
    if let Some(types) = object.get_mut("type").and_then(Value::as_array_mut) {
        types.retain(|ty| ty != "null");
        if types.len() == 1 {
            let value = types[0].clone();
            object.insert("type".into(), value);
        }
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(variants) = object.get_mut(key).and_then(Value::as_array_mut) {
            variants.retain(|variant| variant.get("type") != Some(&Value::String("null".into())));
            if variants.len() == 1 {
                let variant = variants[0].clone();
                object.remove(key);
                if let Some(variant) = variant.as_object() {
                    object.extend(variant.clone());
                }
            }
        }
    }
}

/// Recursively rejects integer schemas lacking bounds within JavaScript exact integers.
/// Returns `lossy_integer` with the offending schema in details. This audits integer
/// representation; it does not validate arbitrary values or prove custom Serde behavior.
pub fn validate_contract_schema(schema: &Value) -> Result<()> {
    fn walk(value: &Value) -> Result<()> {
        match value {
            Value::Object(object) => {
                if object.get("type").is_some_and(|t| {
                    t == "integer"
                        || t.as_array()
                            .is_some_and(|a| a.iter().any(|t| t == "integer"))
                }) {
                    let min = object.get("minimum").and_then(Value::as_f64);
                    let max = object.get("maximum").and_then(Value::as_f64);
                    if min.is_none_or(|n| n < -9_007_199_254_740_991.0)
                        || max.is_none_or(|n| n > 9_007_199_254_740_991.0)
                    {
                        return Err(Error::new(
                            "lossy_integer",
                            "Use DecimalI64/DecimalU64 for large or platform integers",
                        )
                        .details(value.clone()));
                    }
                }
                for value in object.values() {
                    walk(value)?;
                }
            }
            Value::Array(array) => {
                for value in array {
                    walk(value)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(schema)
}

/// Compiles the supplied schema and validates a JSON value against it.
/// Returns `invalid_schema` for a schema compilation failure and
/// `contract_validation` for a value that does not satisfy the schema.
pub fn validate_json(schema: &Value, value: &Value) -> Result<()> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|e| Error::new("invalid_schema", e.to_string()))?;
    validator
        .validate(value)
        .map_err(|e| Error::new("contract_validation", e.to_string()))
}
