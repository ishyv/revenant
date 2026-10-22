/** Interpretive schema checks retain the old validator's preparation boundary. */
import { compileSchema, draft2020, extendDraft } from "#schema-library";
import type { Keyword, SchemaNode } from "json-schema-library";
import type { JsonSchema } from "./contracts.js";

function equalJson(left: unknown, right: unknown): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== "object" || typeof right !== "object") return false;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right) && left.length === right.length
      && left.every((item, index) => equalJson(item, right[index]));
  }
  const a = left as Record<string, unknown>;
  const b = right as Record<string, unknown>;
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length
    && keys.every((key) => Object.hasOwn(b, key) && equalJson(a[key], b[key]));
}

const enumKeyword: Keyword = {
  id: "enum",
  keyword: "enum",
  addValidate: ({ schema }) => schema.enum !== undefined,
  validate: ({ node, data, pointer }) => {
    if (node.schema.enum.some((candidate: unknown) => equalJson(candidate, data))) return;
    return node.createError("enum-error", {
      pointer, schema: node.schema, value: data, values: node.schema.enum,
    });
  },
};

const originalContains = draft2020.keywords.find((keyword) => keyword.keyword === "contains")!;
const containsKeyword: Keyword = {
  ...originalContains,
  validate: (options) => {
    const { node, data, pointer } = options;
    if (!Array.isArray(data) || typeof node.schema.contains !== "boolean") {
      return originalContains.validate?.(options);
    }
    // The library's boolean fast path skips 2020-12 occurrence bounds.
    const count = node.schema.contains ? data.length : 0;
    const minimum = node.schema.minContains ?? 1;
    const maximum = node.schema.maxContains ?? Infinity;
    if (count >= minimum && count <= maximum) return;
    return node.createError(count < minimum ? "contains-min-error" : "contains-max-error", {
      pointer, schema: node.schema, value: data,
      delta: count < minimum ? minimum - count : count - maximum,
    });
  },
};

type ValidationOptions = Parameters<NonNullable<Keyword["validate"]>>[0];
type EvaluationKey = string | number;

class AnnotationValidation {
  private schemas = new WeakMap<object, string>();
  private results = new Map<string, Map<unknown, Map<string, boolean>>>();

  private identity(node: SchemaNode): string {
    // $ref resolution recompiles nodes. Include the effective schema rather
    // than using node identity or conflating schemas at one source location.
    const schema = node.schema;
    let definition = typeof schema === "object" ? this.schemas.get(schema) : undefined;
    if (definition === undefined) {
      definition = JSON.stringify(schema);
      if (typeof schema === "object") this.schemas.set(schema, definition);
    }
    return JSON.stringify([node.$id, node.schemaLocation, definition]);
  }

  private scope(node: SchemaNode, path: ValidationOptions["path"]): string {
    if (!Object.keys(node.context.dynamicAnchors).length) return "";
    const resources = new Set<string | undefined>();
    const scope: string[] = [];
    // The library searches resources in path order and stops at the first
    // explicit dynamic anchor. Repeated visits to a resource cannot change
    // that lookup; keeping the entire recursive path would defeat caching.
    for (const entry of path) {
      if (entry.node.schema.$dynamicAnchor) {
        scope.push(this.identity(entry.node));
        break;
      }
      if (!resources.has(entry.node.$id)) {
        resources.add(entry.node.$id);
        scope.push(JSON.stringify([entry.node.$id]));
      }
    }
    return JSON.stringify(scope);
  }

  valid(node: SchemaNode, data: unknown, pointer: string, path: ValidationOptions["path"]): boolean {
    const identity = JSON.stringify([this.identity(node), this.scope(node, path)]);
    let values = this.results.get(identity);
    if (!values) this.results.set(identity, values = new Map());
    let pointers = values.get(data);
    if (!pointers) values.set(data, pointers = new Map());
    const cached = pointers.get(pointer);
    if (cached !== undefined) return cached;
    const valid = node.validate(data, pointer, [...path]).valid;
    pointers.set(pointer, valid);
    return valid;
  }
}

let annotationValidation: AnnotationValidation | undefined;

/** Evaluate once with annotation probes cached only for this synchronous call. */
export function validateContract(node: SchemaNode, data: unknown): ReturnType<SchemaNode["validate"]> {
  const previous = annotationValidation;
  annotationValidation = new AnnotationValidation();
  try {
    return node.validate(data);
  } finally {
    annotationValidation = previous;
  }
}

// The upstream helpers walk failed alternatives, leaking their annotations into
// unevaluated checks. Collect successful applicators once per container instead.
function evaluatedKeys(options: ValidationOptions, items: boolean): Set<EvaluationKey> {
  const { data, pointer, path } = options;
  const keys: EvaluationKey[] = items
    ? (data as unknown[]).map((_, index) => index)
    : Object.keys(data as object);
  const evaluated = new Set<EvaluationKey>();
  const seen = new Set<string>();
  const valid = (node: SchemaNode, value = data, at = pointer): boolean =>
    annotationValidation?.valid(node, value, at, path) ?? node.validate(value, at, [...path]).valid;
  const visit = (node: SchemaNode, adjacent = false): void => {
    // References can create new nodes for the same schema location.
    const identity = `${node.$id}\n${node.schemaLocation}`;
    if (seen.has(identity)) return;
    seen.add(identity);
    const schema = node.schema;
    if (!adjacent && schema[items ? "unevaluatedItems" : "unevaluatedProperties"] !== undefined) {
      keys.forEach((key) => evaluated.add(key));
    }
    if (items) {
      const values = data as unknown[];
      const prefixLength = node.prefixItems?.length ?? 0;
      for (let index = 0; index < Math.min(values.length, prefixLength); index++) evaluated.add(index);
      if (schema.items !== undefined) {
        for (let index = prefixLength; index < values.length; index++) evaluated.add(index);
      }
      if (schema.contains === true) keys.forEach((key) => evaluated.add(key));
      else if (node.contains) values.forEach((value, index) => {
        if (valid(node.contains!, value, `${pointer}/${index}`)) evaluated.add(index);
      });
    } else {
      for (const key of keys as string[]) {
        const declared = Object.hasOwn(node.properties ?? {}, key);
        const pattern = node.patternProperties?.some((entry) => entry.pattern.test(key));
        if (declared || pattern || schema.additionalProperties !== undefined) evaluated.add(key);
      }
      for (const [key, dependency] of Object.entries(node.dependentSchemas ?? {})) {
        if (Object.hasOwn(data as object, key) && typeof dependency !== "boolean" && valid(dependency)) visit(dependency);
      }
    }
    for (const branches of [node.allOf, node.anyOf, node.oneOf]) {
      for (const branch of branches ?? []) if (valid(branch)) visit(branch);
    }
    if (node.if) {
      const matches = valid(node.if);
      if (matches) visit(node.if);
      const selected = matches ? node.then : node.else;
      if (selected && valid(selected)) visit(selected);
    }
    if (schema.$ref !== undefined || schema.$dynamicRef !== undefined) {
      const resolved = node.resolveRef({ pointer, path: [...path] });
      if (resolved !== node && valid(resolved)) visit(resolved);
    }
  };
  visit(options.node, true);
  return evaluated;
}

function unevaluatedKeyword(items: boolean): Keyword {
  const name = items ? "unevaluatedItems" : "unevaluatedProperties";
  const original = draft2020.keywords.find((keyword) => keyword.keyword === name)!;
  return {
    ...original,
    validate: (options) => {
      const { node, data, pointer, path } = options;
      if (items ? !Array.isArray(data) : !data || typeof data !== "object" || Array.isArray(data)) return;
      if (node.schema[name] === true) return;
      const evaluated = evaluatedKeys(options, items);
      const keys: EvaluationKey[] = items ? (data as unknown[]).map((_, index) => index) : Object.keys(data as object);
      const remaining = items ? node.unevaluatedItems : node.unevaluatedProperties;
      return keys.flatMap((key) => {
        if (evaluated.has(key)) return [];
        const at = `${pointer}/${String(key).replaceAll("~", "~0").replaceAll("/", "~1")}`;
        const value = (data as Record<EvaluationKey, unknown>)[key];
        if (remaining) return remaining.validate(value, at, [...path]).errors;
        return [node.createError(items ? "unevaluated-items-error" : "unevaluated-property-error", {
          pointer: at, value: JSON.stringify(value), schema: node.schema,
        })];
      });
    },
  };
}

/** Apply interpretive corrections while retaining draft 2020-12 annotation boundaries. */
export const contractDraft = extendDraft(draft2020, {
  keywords: [enumKeyword, containsKeyword, unevaluatedKeyword(true), unevaluatedKeyword(false)],
});

// Local-reference composition of the standard 2020-12 vocabularies:
// https://json-schema.org/draft/2020-12/schema and its seven meta/* resources.
// Each standard #meta dynamic reference refers to this complete schema; using
// a local root reference here keeps validation recursive and entirely offline.
// Formats remain annotations, matching Ajv's validateFormats:false setting.
const schema = { $ref: "#" };
const nonNegative = { $ref: "#/$defs/nonNegativeInteger" };
const schemaMap = { type: "object", additionalProperties: schema };
const schemaArray = { type: "array", minItems: 1, items: schema };
const stringArray = { type: "array", items: { type: "string" }, uniqueItems: true };
const anchor = { type: "string", pattern: "^[A-Za-z_][-A-Za-z0-9._]*$" };
const meta = compileSchema({
  type: ["object", "boolean"],
  $defs: {
    nonNegativeInteger: { type: "integer", minimum: 0 },
    simpleTypes: { enum: ["array", "boolean", "integer", "null", "number", "object", "string"] },
  },
  properties: {
    $id: { type: "string", pattern: "^[^#]*#?$" },
    $schema: { type: "string" },
    $ref: { type: "string" },
    $dynamicRef: { type: "string" },
    $anchor: anchor,
    $dynamicAnchor: anchor,
    $recursiveAnchor: anchor,
    $recursiveRef: { type: "string" },
    $vocabulary: { type: "object", additionalProperties: { type: "boolean" } },
    $comment: { type: "string" },
    $defs: schemaMap,
    definitions: schemaMap,
    dependencies: { type: "object", additionalProperties: { anyOf: [schema, stringArray] } },
    type: { anyOf: [
      { $ref: "#/$defs/simpleTypes" },
      { type: "array", items: { $ref: "#/$defs/simpleTypes" }, minItems: 1, uniqueItems: true },
    ] },
    const: true,
    enum: { type: "array" },
    multipleOf: { type: "number", exclusiveMinimum: 0 },
    maximum: { type: "number" },
    exclusiveMaximum: { type: "number" },
    minimum: { type: "number" },
    exclusiveMinimum: { type: "number" },
    minLength: nonNegative,
    maxLength: nonNegative,
    minItems: nonNegative,
    maxItems: nonNegative,
    minContains: nonNegative,
    maxContains: nonNegative,
    minProperties: nonNegative,
    maxProperties: nonNegative,
    pattern: { type: "string" },
    uniqueItems: { type: "boolean" },
    required: stringArray,
    dependentRequired: { type: "object", additionalProperties: stringArray },
    prefixItems: schemaArray,
    items: schema,
    contains: schema,
    additionalProperties: schema,
    properties: schemaMap,
    patternProperties: schemaMap,
    dependentSchemas: schemaMap,
    propertyNames: schema,
    if: schema,
    then: schema,
    else: schema,
    allOf: schemaArray,
    anyOf: schemaArray,
    oneOf: schemaArray,
    not: schema,
    unevaluatedItems: schema,
    unevaluatedProperties: schema,
    title: { type: "string" },
    description: { type: "string" },
    default: true,
    deprecated: { type: "boolean" },
    readOnly: { type: "boolean" },
    writeOnly: { type: "boolean" },
    examples: { type: "array" },
    format: { type: "string" },
    contentEncoding: { type: "string" },
    contentMediaType: { type: "string" },
    contentSchema: schema,
  },
}, { drafts: [contractDraft], formatAssertion: false, throwOnInvalidSchema: true, throwOnInvalidRef: true });

/** Validate every nested schema before compiling its value assertions. */
export function checkSchemaDefinition(schema: JsonSchema): void {
  if (!meta.validate(schema).valid) throw new Error("Invalid draft 2020-12 schema definition");
}
