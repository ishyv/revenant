/** Compare interpretive validation with Ajv against actual compiled application contracts. */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import Ajv2020 from "ajv/dist/2020.js";
import { validator } from "../src/schema.ts";

const applications = process.argv.slice(2);
assert.ok(applications.length, "Pass reference application roots containing a published facade");
const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
let checked = 0;

function sample(schema, root, depth = 0) {
  assert.ok(depth < 32, "Recursive sample requires an explicit fixture");
  if (schema === true) return null;
  if (schema === false) throw new Error("No positive sample for false schema");
  if (schema.$ref) {
    assert.ok(schema.$ref.startsWith("#/"), "Only local contract references are supported");
    const target = schema.$ref.slice(2).split("/").reduce((node, key) => node[key.replaceAll("~1", "/").replaceAll("~0", "~")], root);
    return sample(target, root, depth + 1);
  }
  if ("const" in schema) return schema.const;
  if (schema.enum) return schema.enum[0];
  if (schema.oneOf || schema.anyOf) return sample((schema.oneOf ?? schema.anyOf)[0], root, depth + 1);
  const type = Array.isArray(schema.type) ? schema.type[0] : schema.type;
  if (type === "null" || !type) return null;
  if (type === "boolean") return false;
  if (type === "number" || type === "integer") return schema.minimum ?? (schema.exclusiveMinimum ?? -1) + 1;
  if (type === "string") {
    if (schema.pattern?.includes("0|")) return "0";
    return "a".repeat(schema.minLength ?? 0);
  }
  if (type === "array") return Array.from({ length: schema.minItems ?? 0 }, (_, index) => sample(schema.prefixItems?.[index] ?? schema.items ?? true, root, depth + 1));
  if (type === "object") return Object.fromEntries((schema.required ?? []).map((key) => [key, sample(schema.properties?.[key] ?? true, root, depth + 1)]));
  throw new Error(`Unsupported compiled schema type: ${type}`);
}

for (const application of applications) {
  const facade = resolve(application, "web/src/lib/revenant.ts");
  const pointer = readFileSync(facade, "utf8").match(/from ['"](.+\/facade\.ts)['"]/);
  assert.ok(pointer, `Missing published generation in ${facade}`);
  const manifest = JSON.parse(readFileSync(resolve(dirname(facade), dirname(pointer[1]), "revenant.contract.json"), "utf8"));
  for (const operation of manifest.operations) {
    for (const direction of ["input", "output"]) {
      const schema = operation[direction].schema;
      const previous = ajv.compile(structuredClone(schema));
      const current = validator(schema);
      const positive = sample(schema, schema);
      assert.equal(previous(positive), true, `${operation.id} ${direction}: invalid positive fixture ${JSON.stringify(previous.errors)}`);
      const candidates = [positive, null, false, true, 0, -1, 1.5, "", "x", [], {}, { unexpected: true }];
      if (positive && typeof positive === "object" && !Array.isArray(positive)) {
        for (const key of Object.keys(positive)) {
          const missing = { ...positive }; delete missing[key];
          candidates.push(missing, { ...positive, [key]: null }, { ...positive, [key]: [] });
        }
      }
      for (const value of candidates) {
        let accepted = true;
        try { current(value); } catch (error) { assert.equal(error.code, "contract_violation"); accepted = false; }
        assert.equal(accepted, previous(value), `${operation.id} ${direction}: ${JSON.stringify(value)}`);
      }
      checked++;
    }
  }
}
console.log(`Compiled contract parity: ${checked} input/output schemas passed with positive and invalid JSON samples.`);
