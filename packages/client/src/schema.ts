/** Generated schema validation at the projection boundary; native checks remain authoritative. */
import Ajv2020 from "ajv/dist/2020.js";
import type { ValidateFunction } from "ajv";
import { assertJson, failure } from "./contracts.js";
import type {
  ContractManifest,
  JsonSchema,
  OperationDefinition,
} from "./contracts.js";

/** Compile a generated JSON schema into a classified validator. */
export function validator(schema: JsonSchema): (value: unknown) => void {
  const ajv = new Ajv2020({
    strict: false,
    allErrors: true,
    validateFormats: false,
  });
  let check: ValidateFunction;
  try {
    check = ajv.compile(schema);
  } catch (error) {
    failure("invalid_schema", "Cannot compile contract schema", String(error));
  }
  return (value) => {
    assertJson(value);
    if (!check(value))
      failure(
        "contract_violation",
        "Value does not satisfy its JSON schema",
        check.errors,
      );
  };
}
/** Check desktop protocol and registration uniqueness, preserving generated source locations. */
export function validateManifest(value: unknown): ContractManifest {
  assertJson(value);
  const manifest = value as ContractManifest;
  if (
    manifest?.version !== 3 ||
    manifest.protocol !== 2 ||
    typeof manifest.digest !== "string" ||
    !manifest.digest ||
    !Array.isArray(manifest.operations)
  ) {
    failure(
      "incompatible_contract",
      "Expected a version 3, protocol 2 desktop contract manifest",
    );
  }
  const ids = new Set<string>();
  for (const operation of manifest.operations) {
    if (
      !operation ||
      typeof operation.id !== "string" ||
      !operation.id ||
      ids.has(operation.id) ||
      typeof operation.description !== "string"
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
    for (const definition of [operation.input, operation.output]) {
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
  return structuredClone(manifest);
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
    canonical(a.input.schema) === canonical(b.input.schema) &&
    canonical(a.output.schema) === canonical(b.output.schema)
  );
}
