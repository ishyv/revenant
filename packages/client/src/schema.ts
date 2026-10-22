/** Generated schema validation at the projection boundary; native checks remain authoritative. */
import { compileSchema, isJsonError } from "#schema-library";
import type { SchemaNode } from "json-schema-library";
import { checkSchemaDefinition, contractDraft, validateContract } from "./schema-meta.js";
import { assertJson, cloneJson, failure } from "./contracts.js";
import type {
  ContractManifest,
  JsonSchema,
  OperationDefinition,
} from "./contracts.js";

/** Prepare an interpretive draft 2020-12 validator without dynamic code execution. */
export function validator(schema: JsonSchema): (value: unknown) => void {
  let check: SchemaNode;
  try {
    assertJson(schema);
    checkSchemaDefinition(schema);
    if (typeof schema === "object" && schema !== null && schema.$schema !== undefined
      && schema.$schema !== "https://json-schema.org/draft/2020-12/schema"
      && schema.$schema !== "https://json-schema.org/draft/2020-12/schema#") {
      throw new Error("Contract schemas must use JSON Schema draft 2020-12");
    }
    check = compileSchema(cloneJson(schema), {
      drafts: [contractDraft],
      formatAssertion: false,
      throwOnInvalidSchema: true,
      throwOnInvalidRef: true,
    });
    // Ajv rejected unresolved references at preparation, even in unselected branches.
    for (const node of check.toSchemaNodes()) {
      if (node.schema.$ref !== undefined || node.schema.$dynamicRef !== undefined) {
        const resolved = node.resolveRef();
        if (!resolved || isJsonError(resolved)) throw new Error("Unresolved contract schema reference");
      }
    }
  } catch (error) {
    failure("invalid_schema", "Cannot compile contract schema", String(error));
  }
  return (value) => {
    assertJson(value);
    let result: ReturnType<SchemaNode["validate"]>;
    try {
      result = validateContract(check, value);
    } catch (error) {
      failure("invalid_schema", "Cannot evaluate contract schema", String(error));
    }
    if (!result.valid)
      failure(
        "contract_violation",
        "Value does not satisfy its JSON schema",
        // Library errors retain schema nodes and values; expose bounded diagnostics only.
        result.errors.slice(0, 32).map((error) => ({
          code: String(error.code).slice(0, 128),
          message: error.message.slice(0, 512),
          pointer: error.data.pointer.slice(0, 512),
        })),
      );
  };
}
/** Check desktop protocol and registration uniqueness, preserving generated source locations. */
export function validateManifest(value: unknown): ContractManifest {
  assertJson(value);
  const manifest = value as ContractManifest;
  if (
    manifest?.version !== 6 ||
    manifest.protocol !== 5 ||
    typeof manifest.digest !== "string" ||
    !manifest.digest ||
    !Array.isArray(manifest.operations)
  ) {
    failure(
      "incompatible_contract",
      "Expected a version 6, protocol 5 desktop contract manifest",
    );
  }
  const ids = new Set<string>();
  for (const operation of manifest.operations) {
    if (
      !operation ||
      typeof operation.id !== "string" ||
      !operation.id ||
      ids.has(operation.id) ||
      typeof operation.description !== "string" ||
      !Array.isArray(operation.arguments) ||
      operation.arguments.some(name => typeof name !== "string" || !name) ||
      new Set(operation.arguments).size !== operation.arguments.length ||
      !["unknown", "readOnly", "write"].includes(operation.effect) ||
      (operation.workClass !== undefined && !["cpu", "io", "interactive"].includes(operation.workClass)) ||
      (operation.workClass === "interactive" && operation.effect !== "readOnly") ||
      typeof operation.independent !== "boolean" ||
      typeof operation.agent !== "boolean"
    ) {
      failure(
        "invalid_contract",
        "Operation ids must be unique nonempty strings",
      );
    }
    ids.add(operation.id);
    if (
      operation.source &&
      (typeof operation.source.file !== "string" ||
        typeof operation.source.module !== "string" ||
        !Number.isSafeInteger(operation.source.line) ||
        operation.source.line < 1)
    ) {
      failure(
        "invalid_contract",
        "Operation source must identify a file, module and positive line",
        operation.id,
      );
    }
    for (const definition of [operation.input, operation.output, ...(operation.updates === undefined ? [] : [operation.updates])]) {
      if (
        !definition ||
        typeof definition.name !== "string" ||
        typeof definition.typescript !== "string" ||
        definition.schema === undefined
      ) {
        failure(
          "invalid_contract",
          "Missing exported type definition",
          operation.id,
        );
      }
      validator(definition.schema);
    }
  }
  return cloneJson(manifest);
}
/** Compare execution identity and schemas while allowing descriptive source locations to differ. */
export function sameContract(
  a: OperationDefinition,
  b: OperationDefinition,
): boolean {
  const canonical = (value: unknown): string =>
    JSON.stringify(value, (_key, item: unknown) =>
      item && typeof item === "object" && !Array.isArray(item)
        ? Object.fromEntries(
            Object.entries(item).sort(([left], [right]) =>
              left.localeCompare(right),
            ),
          )
        : item,
    );
  return (
    a.id === b.id &&
    a.effect === b.effect && a.independent === b.independent && a.agent === b.agent &&
    (a.workClass ?? "cpu") === (b.workClass ?? "cpu") &&
    canonical(a.arguments) === canonical(b.arguments) &&
    canonical(a.input.schema) === canonical(b.input.schema) &&
    canonical(a.output.schema) === canonical(b.output.schema) &&
    canonical(a.updates?.schema) === canonical(b.updates?.schema)
  );
}
