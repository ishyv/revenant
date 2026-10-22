import assert from "node:assert/strict";
import { test } from "node:test";
import { execFileSync } from "node:child_process";
import Ajv2020 from "ajv/dist/2020.js";
import { validator } from "../src/schema.ts";

const cases = [
  [{ type: "object", properties: { n: { type: "integer", minimum: 0 }, note: { type: ["string", "null"] } }, required: ["n"], additionalProperties: false }, [{ n: 0 }, { n: 1, note: null }, { n: -1 }, { n: 1.5 }, {}, { n: 1, extra: true }]],
  [{ $defs: { value: { type: "string", minLength: 2, pattern: "^[a-z]+$" } }, type: "array", items: { $ref: "#/$defs/value" }, uniqueItems: true, maxItems: 2 }, [["ab"], [], ["ab", "cd"], ["a"], ["AB"], ["ab", "ab"], ["ab", "cd", "ef"]]],
  [{ type: "array", prefixItems: [{ type: "string" }, { type: "integer" }], items: false, minItems: 2 }, [["a", 2], ["a"], [1, 2], ["a", 2, 3]]],
  [{ oneOf: [{ type: "integer" }, { type: "string", enum: ["yes", "no"] }] }, [1, "yes", "no", 1.5, "maybe", true, null]],
  [{ type: "object", properties: { flag: { type: "boolean" }, value: {} }, if: { properties: { flag: { const: true } }, required: ["flag"] }, then: { required: ["value"] }, dependentRequired: { value: ["flag"] } }, [{}, { flag: false }, { flag: true, value: 1 }, { flag: true }, { value: 1 }]],
  [{ type: "object", allOf: [{ properties: { a: { type: "integer" } } }], unevaluatedProperties: false }, [{}, { a: 1 }, { a: "x" }, { b: 1 }]],
  [{ type: "string", format: "date-time", "x-rust-type": "String" }, ["not-a-date", "2026-10-06T00:00:00Z", 2]],
  [true, [null, {}, [], 1, "a"]],
  [false, [null, {}, [], 1, "a"]],
  [{ enum: [{ a: 1, b: { c: [2, 3] } }] }, [{ a: 1, b: { c: [2, 3] } }, { b: { c: [2, 3] }, a: 1 }, { b: { c: [3, 2] }, a: 1 }]],
  [{ type: "array", contains: true, minContains: 0, maxContains: 1 }, [[], [1], [1, 2]]],
  [{ type: "array", contains: false, minContains: 0 }, [[], [1], [1, 2]]],
  [{ type: "array", contains: false }, [[], [1]]],
  [{ type: "array", contains: { type: "integer" }, minContains: 1, maxContains: 2 }, [[], [1], [1, "x"], [1, 2], [1, 2, 3], ["x"]]],
];

test("recursive annotation checks remain bounded for deep valid and invalid inputs", () => {
  // A child-process deadline also bounds a regression that blocks the event
  // loop. Before memoization, depth 16 took seconds and each level doubled it.
  const source = `
    import assert from 'node:assert/strict';
    import { validator } from ${JSON.stringify(new URL("../src/schema.ts", import.meta.url).href)};
    const schema = { $defs: { node: { type: 'object',
      allOf: [{ properties: { next: { $ref: '#/$defs/node' } } }],
      unevaluatedProperties: false } }, $ref: '#/$defs/node' };
    const check = validator(schema);
    let value = {};
    const leaf = value;
    for (let depth = 0; depth < 48; depth++) value = { next: value };
    check(value);
    leaf.unexpected = true;
    assert.throws(() => check(value), error => error.code === 'contract_violation');
    delete leaf.unexpected;
    check(value);
    console.log('recursive annotations passed');
  `;
  assert.match(execFileSync(process.execPath, ["--import", "tsx", "--input-type=module", "--eval", source], {
    cwd: new URL("..", import.meta.url), encoding: "utf8", timeout: 10000,
  }), /recursive annotations passed/);
});

test("annotation memoization distinguishes dynamic scopes, shared values and input mutations", () => {
  const schema = { $defs: {
    base: { $id: "https://example.test/base", $dynamicAnchor: "node", type: "object",
      properties: { value: true, next: { $dynamicRef: "#node" } }, unevaluatedProperties: false },
    numbers: { $id: "https://example.test/numbers", $dynamicAnchor: "node",
      $ref: "https://example.test/base", properties: { value: { type: "number" } } },
    strings: { $id: "https://example.test/strings", $dynamicAnchor: "node",
      $ref: "https://example.test/base", properties: { value: { type: "string" } } },
  }, anyOf: [{ $ref: "https://example.test/numbers" }, { $ref: "https://example.test/strings" }] };
  const old = new Ajv2020({ strict: false, allErrors: true, validateFormats: false }).compile(schema);
  const check = validator(schema);
  for (const value of [{ value: 1, next: { value: 2 } }, { value: "a", next: { value: "b" } },
    { value: 1, next: { value: "b" } }, { value: "a", next: { value: 2 } }]) {
    let accepted = true;
    try { check(value); } catch (error) { assert.equal(error.code, "contract_violation"); accepted = false; }
    assert.equal(accepted, old(value), JSON.stringify(value));
  }
  const shared = { value: 1 };
  const tuple = validator({ type: "array", allOf: [{ prefixItems: [
    { properties: { value: { type: "number" } }, unevaluatedProperties: false },
    { properties: { value: { type: "number" } }, unevaluatedProperties: false },
  ] }], unevaluatedItems: false });
  tuple([shared, shared]);
  shared.value = "changed";
  assert.throws(() => tuple([shared, shared]), error => error.code === "contract_violation");
  shared.value = 1;
  tuple([shared, shared]);
});

test("interpretive validation matches Ajv for draft 2020-12 contract shapes", () => {
  const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
  for (const [schema, values] of cases) {
    const original = structuredClone(schema);
    const old = ajv.compile(structuredClone(schema));
    const check = validator(schema);
    assert.deepEqual(schema, original, "schema preparation must not mutate contract identity");
    for (const value of values) {
      let valid = true;
      try { check(value); } catch (error) {
        assert.equal(error.code, "contract_violation");
        assert.doesNotThrow(() => JSON.stringify(error.details));
        valid = false;
      }
      assert.equal(valid, old(value), JSON.stringify({ schema, value }));
    }
  }
});

test("schema preparation matches Ajv metaschema checks throughout nested definitions", () => {
  const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
  const malformed = [
    { minLength: -1 }, { maxLength: 1.5 }, { minItems: -1 }, { maxItems: 0.5 },
    { minContains: -1 }, { maxContains: 1.5 }, { minProperties: -1 }, { maxProperties: 0.5 },
    { type: [] }, { type: ["string", "string"] }, { multipleOf: 0 }, { multipleOf: -1 },
    { required: ["a", "a"] }, { dependentRequired: { a: ["b", "b"] } },
    { minimum: null }, { allOf: [] }, { anyOf: [] }, { oneOf: [] },
    { items: [] }, { prefixItems: [] }, { $vocabulary: { test: "true" } },
    { $anchor: "bad anchor" }, { contentSchema: { minLength: -1 } },
  ];
  for (const definition of malformed) {
    for (const schema of [definition, { type: "object", properties: { x: definition } },
      { $defs: { unused: definition } }, { allOf: [{ anyOf: [true, definition] }] }]) {
      assert.throws(() => ajv.compile(schema), undefined, JSON.stringify(schema));
      assert.throws(() => validator(schema), (error) => error.code === "invalid_schema", JSON.stringify(schema));
    }
  }
  for (const schema of [{}, true, false, { required: [] }, { enum: [1, 1] },
    { $defs: { node: { type: "object", properties: { next: { $ref: "#/$defs/node" } } } }, $ref: "#/$defs/node" },
    { default: { minLength: -1 }, "x-custom": { type: [] } }]) {
    assert.doesNotThrow(() => ajv.compile(schema));
    assert.doesNotThrow(() => validator(schema));
  }
});

test("unevaluated checks collect annotations only from successful applicable branches", () => {
  const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
  const fixtures = [];
  for (const keyword of ["anyOf", "oneOf"]) {
    fixtures.push([
      { type: "array", [keyword]: [
        { contains: { type: "number" }, minContains: 2 },
        { contains: { type: "string" }, minContains: 2 },
      ], unevaluatedItems: false },
      [[1, "a", "b"], [], [1]],
    ], [
      { type: "object", [keyword]: [
        { properties: { a: true }, required: ["missing"] },
        { properties: { b: true }, required: ["b"] },
      ], unevaluatedProperties: false },
      [{ a: 1, b: 2 }, { b: 2 }, { a: 1 }, {}],
    ]);
  }
  fixtures.push([
    { type: "array", $defs: { tuple: { prefixItems: [{ type: "number" }] } },
      $ref: "#/$defs/tuple", unevaluatedItems: { type: "string" } },
    [[1, "a"], [1, 2], [1], ["a"]],
  ], [
    { type: "object", $defs: { shape: { properties: { a: { type: "number" } } } },
      $ref: "#/$defs/shape", unevaluatedProperties: { type: "string" } },
    [{ a: 1, b: "x" }, { a: 1, b: 2 }, { a: "x" }, {}],
  ], [
    { type: "object", properties: { flag: true },
      dependentSchemas: { flag: { properties: { value: true } } }, unevaluatedProperties: false },
    [{ flag: true, value: 1 }, { value: 1 }, {}],
  ], [
    { type: "array", anyOf: [{ prefixItems: [true] }, { prefixItems: [true, true] }], unevaluatedItems: false },
    [[1], [1, 2], [1, 2, 3]],
  ], [
    { type: "object", anyOf: [{ properties: { a: true } }, { properties: { b: true } }], unevaluatedProperties: false },
    [{ a: 1, b: 2 }, { a: 1 }, { b: 2 }, { c: 3 }],
  ], [
    { type: "array", allOf: [{ prefixItems: [true], unevaluatedItems: { type: "string" } }], unevaluatedItems: false },
    [[1, "a"], [1, 2], [1]],
  ]);
  for (const [schema, values] of fixtures) {
    const previous = ajv.compile(structuredClone(schema));
    const check = validator(schema);
    for (const value of values) {
      let accepted = true;
      try { check(value); } catch (error) {
        assert.equal(error.code, "contract_violation");
        accepted = false;
      }
      assert.equal(accepted, previous(value), JSON.stringify({ schema, value }));
    }
  }
});

test("contains and conditional annotations follow draft 2020-12 even where Ajv differs", () => {
  // Core 7.7.1.2 drops annotations from failed schema objects; 10.3.1.3
  // annotates every matching contains index. Ajv's optimized bookkeeping
  // disagrees on these combinations, so the specification is the oracle.
  for (const keyword of ["anyOf", "oneOf"]) {
    const check = validator({ type: "array", [keyword]: [
      { contains: { type: "number" }, minContains: 2 },
      { contains: { type: "string" }, minContains: 2 },
    ], unevaluatedItems: false });
    check([1, 2]);
    check(["a", "b"]);
    assert.throws(() => check([1, "a", "b"]), (error) => error.code === "contract_violation");
  }
  const array = validator({ type: "array", if: { prefixItems: [true, true, true], minItems: 4 },
    else: { prefixItems: [true, true] }, unevaluatedItems: false });
  array([1, 2]);
  assert.throws(() => array([1, 2, 3]), (error) => error.code === "contract_violation");
  const object = validator({ type: "object", if: { properties: { a: true }, required: ["missing"] },
    else: { properties: { b: true } }, unevaluatedProperties: false });
  object({ b: 2 });
  assert.throws(() => object({ a: 1, b: 2 }), (error) => error.code === "contract_violation");
  validator({ type: "array", contains: true, minContains: 0, unevaluatedItems: false })([1, 2]);
});

test("schema validation rejects invalid schemas and non-JSON inputs with classified errors", () => {
  assert.throws(() => validator({ type: "imaginary" }), (error) => error.code === "invalid_schema");
  assert.throws(() => validator({ $schema: "http://json-schema.org/draft-07/schema", type: "object" }), (error) => error.code === "invalid_schema");
  assert.throws(() => validator({ type: "object", properties: { unused: { $ref: "#missing" } } }), (error) => error.code === "invalid_schema");
  const check = validator({ type: "object" });
  const cyclic = {}; cyclic.self = cyclic;
  for (const value of [cyclic, { n: NaN }, { n: undefined }, { n: 1n }]) {
    assert.throws(() => check(value), (error) => error.code === "invalid_json");
  }
});

test("runtime validator prepares and checks contracts when dynamic code execution is blocked", () => {
  const original = globalThis.Function;
  globalThis.Function = function () { throw new Error("CSP blocked dynamic execution"); };
  try {
    const check = validator(cases[0][0]);
    check({ n: 1 });
    assert.throws(() => check({ n: -1 }), (error) => error.code === "contract_violation");
    const annotations = validator({ type: "object", anyOf: [
      { properties: { a: true }, required: ["missing"] },
      { properties: { b: true }, required: ["b"] },
    ], unevaluatedProperties: false });
    annotations({ b: 1 });
    assert.throws(() => annotations({ a: 1, b: 2 }), (error) => error.code === "contract_violation");
  } finally { globalThis.Function = original; }
});
