// json-schema-library 11.6.4's ESM build imports a CommonJS fast-uri named export
// that Node cannot discover. Its supported require entry works without eval.
// Browser bundlers use the ordinary ESM package through the import condition.
const library = require("json-schema-library");
exports.compileSchema = library.compileSchema;
exports.draft2020 = library.draft2020;
exports.extendDraft = library.extendDraft;
exports.isJsonError = library.isJsonError;
