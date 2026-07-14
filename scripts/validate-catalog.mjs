import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function fail(path, message) {
  throw new Error(`catalog policy: ${path}: ${message}`);
}

function readJson(relativePath) {
  try {
    return JSON.parse(readFileSync(join(root, relativePath), "utf8"));
  } catch (error) {
    fail(relativePath, `invalid JSON: ${error.message}`);
  }
}

function object(value, path) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(path, "must be an object");
  }
  return value;
}

function exactKeys(value, allowed, required, path) {
  object(value, path);
  const keys = Object.keys(value);
  const extras = keys.filter((key) => !allowed.includes(key));
  const missing = required.filter((key) => !keys.includes(key));
  if (extras.length) fail(path, `unexpected keys: ${extras.join(", ")}`);
  if (missing.length) fail(path, `missing keys: ${missing.join(", ")}`);
}

function nonEmpty(value, path) {
  if (typeof value !== "string" || value.trim() === "")
    fail(path, "must be a non-empty string");
}

function bilingual(value, path) {
  exactKeys(value, ["en", "ko"], ["en", "ko"], path);
  nonEmpty(value.en, `${path}.en`);
  nonEmpty(value.ko, `${path}.ko`);
}

function oneOf(value, allowed, path) {
  if (!allowed.includes(value))
    fail(path, `must be one of ${allowed.join(", ")}`);
}

function unique(values, path) {
  if (new Set(values).size !== values.length)
    fail(path, "must contain unique values");
}

function validateHttps(value, path) {
  nonEmpty(value, path);
  let url;
  try {
    url = new URL(value);
  } catch {
    fail(path, "must be an absolute URL");
  }
  if (url.protocol !== "https:") fail(path, "must use HTTPS");
}

const schemaPaths = [
  "catalog/schema/options.schema.json",
  "catalog/schema/presets.schema.json",
  "catalog/schema/profile.schema.json",
  "catalog/schema/share.schema.json",
];
const schemaDocuments = schemaPaths.map((schemaPath) => [
  schemaPath,
  readJson(schemaPath),
]);
for (const [schemaPath, schema] of schemaDocuments) {
  if (schema.$schema !== "https://json-schema.org/draft/2020-12/schema") {
    fail(schemaPath, "must declare JSON Schema draft 2020-12");
  }
  if (
    typeof schema.$id !== "string" ||
    !schema.$id.startsWith("https://github.com/INIRU/Wuwa-ini-Tool/")
  ) {
    fail(schemaPath, "must use the canonical repository schema ID");
  }
  if (schema.type !== "object" || schema.additionalProperties !== false) {
    fail(schemaPath, "root must be a closed object schema");
  }
}

const catalog = readJson("catalog/options.json");
const presetsDocument = readJson("catalog/presets.json");
const ajv = new Ajv2020({ allErrors: true, strict: true });
addFormats(ajv);
for (const [, schema] of schemaDocuments) ajv.addSchema(schema);
for (const [schemaPath, schema] of schemaDocuments) {
  if (!ajv.getSchema(schema.$id))
    fail(schemaPath, "did not compile successfully");
}
for (const [instancePath, schemaPath, instance] of [
  ["catalog/options.json", "catalog/schema/options.schema.json", catalog],
  [
    "catalog/presets.json",
    "catalog/schema/presets.schema.json",
    presetsDocument,
  ],
]) {
  const schema = schemaDocuments.find(([path]) => path === schemaPath)?.[1];
  const validate = schema && ajv.getSchema(schema.$id);
  if (!validate) fail(schemaPath, "compiled validator is unavailable");
  if (!validate(instance)) {
    const details = ajv.errorsText(validate.errors, {
      dataVar: instancePath,
      separator: "; ",
    });
    fail(instancePath, `JSON Schema validation failed: ${details}`);
  }
}

exactKeys(
  catalog,
  ["schema_version", "options"],
  ["schema_version", "options"],
  "catalog/options.json",
);
if (catalog.schema_version !== 1)
  fail("catalog/options.json.schema_version", "must equal 1");
if (!Array.isArray(catalog.options))
  fail("catalog/options.json.options", "must be an array");

const statuses = [
  "verified",
  "community_reported",
  "experimental",
  "ignored",
  "regressed",
];
const valueTypes = ["boolean", "integer", "float", "text"];
const risks = ["low", "medium", "high"];
const optionByIdentity = new Map();

for (const [index, option] of catalog.options.entries()) {
  const path = `catalog/options.json.options[${index}]`;
  exactKeys(
    option,
    [
      "section",
      "key",
      "description",
      "value_type",
      "constraints",
      "risk",
      "status",
      "evidence",
    ],
    [
      "section",
      "key",
      "description",
      "value_type",
      "constraints",
      "risk",
      "status",
      "evidence",
    ],
    path,
  );
  nonEmpty(option.section, `${path}.section`);
  nonEmpty(option.key, `${path}.key`);
  if (/[[\]\u0000-\u001f\u007f]/u.test(option.section))
    fail(`${path}.section`, "contains unsafe INI characters");
  if (/[=\r\n\u0000]/u.test(option.key))
    fail(`${path}.key`, "contains unsafe INI characters");
  bilingual(option.description, `${path}.description`);
  oneOf(option.value_type, valueTypes, `${path}.value_type`);
  oneOf(option.risk, risks, `${path}.risk`);
  oneOf(option.status, statuses, `${path}.status`);

  const identity = `${option.section}\u0000${option.key}`.toLocaleLowerCase(
    "en-US",
  );
  if (optionByIdentity.has(identity))
    fail(path, "duplicates a section/key identity");
  optionByIdentity.set(identity, option);

  exactKeys(
    option.constraints,
    ["minimum", "maximum", "allowed_values"],
    ["minimum", "maximum", "allowed_values"],
    `${path}.constraints`,
  );
  for (const bound of ["minimum", "maximum"]) {
    const value = option.constraints[bound];
    if (
      value !== null &&
      (typeof value !== "number" || !Number.isFinite(value))
    ) {
      fail(`${path}.constraints.${bound}`, "must be a finite number or null");
    }
  }
  if (
    option.constraints.minimum !== null &&
    option.constraints.maximum !== null &&
    option.constraints.minimum > option.constraints.maximum
  ) {
    fail(`${path}.constraints`, "minimum exceeds maximum");
  }
  if (!Array.isArray(option.constraints.allowed_values)) {
    fail(`${path}.constraints.allowed_values`, "must be an array");
  }
  option.constraints.allowed_values.forEach((value, allowedIndex) =>
    nonEmpty(value, `${path}.constraints.allowed_values[${allowedIndex}]`),
  );
  unique(
    option.constraints.allowed_values,
    `${path}.constraints.allowed_values`,
  );

  exactKeys(
    option.evidence,
    [
      "source_url",
      "tested_game_version",
      "tested_date",
      "tested_hardware",
      "runtime_verified",
    ],
    [
      "source_url",
      "tested_game_version",
      "tested_date",
      "tested_hardware",
      "runtime_verified",
    ],
    `${path}.evidence`,
  );
  validateHttps(option.evidence.source_url, `${path}.evidence.source_url`);
  for (const field of ["tested_game_version", "tested_hardware"]) {
    if (option.evidence[field] !== null)
      nonEmpty(option.evidence[field], `${path}.evidence.${field}`);
  }
  if (
    option.evidence.tested_date !== null &&
    !/^\d{4}-\d{2}-\d{2}$/.test(option.evidence.tested_date)
  ) {
    fail(
      `${path}.evidence.tested_date`,
      "must be an ISO calendar date or null",
    );
  }
  if (typeof option.evidence.runtime_verified !== "boolean") {
    fail(`${path}.evidence.runtime_verified`, "must be boolean");
  }
  if (option.status === "verified" && !option.evidence.runtime_verified) {
    fail(path, "verified status requires runtime_verified evidence");
  }
  if (option.evidence.runtime_verified) {
    if (option.status !== "verified")
      fail(path, "runtime_verified requires verified status");
    for (const field of [
      "tested_game_version",
      "tested_date",
      "tested_hardware",
    ]) {
      if (option.evidence[field] === null)
        fail(path, `runtime_verified requires ${field}`);
    }
  }
}

const requiredReviewedKeys = [
  "r.Streaming.PoolSize",
  "r.ParallelFrustumCull",
  "r.ParallelOcclusionCull",
  "r.Streaming.FullyLoadUsedTextures",
  "r.Streaming.HLODStrategy",
  "r.Streaming.UsingKuroStreamingPriority",
];
const catalogKeys = new Set(catalog.options.map((option) => option.key));
for (const key of requiredReviewedKeys) {
  if (!catalogKeys.has(key))
    fail("catalog/options.json", `missing source-reviewed key ${key}`);
}

function validatePresetValue(value, option, path) {
  if (value === null) return;
  if (typeof value !== "string") fail(path, "must be a string or null");
  const allowed = option.constraints.allowed_values;
  if (allowed.length && !allowed.includes(value))
    fail(path, "is not in allowed_values");
  if (option.value_type === "text") return;
  if (
    option.value_type === "boolean" &&
    !["0", "1", "true", "false"].includes(value.toLowerCase())
  ) {
    fail(path, "is not a recognized boolean");
  }
  if (option.value_type === "integer" && !/^-?\d+$/.test(value))
    fail(path, "is not an integer");
  if (
    option.value_type === "float" &&
    (value.trim() === "" || !Number.isFinite(Number(value)))
  ) {
    fail(path, "is not a finite number");
  }
  if (["integer", "float"].includes(option.value_type)) {
    const numeric = Number(value);
    if (
      option.constraints.minimum !== null &&
      numeric < option.constraints.minimum
    )
      fail(path, "is below minimum");
    if (
      option.constraints.maximum !== null &&
      numeric > option.constraints.maximum
    )
      fail(path, "exceeds maximum");
  }
}

exactKeys(
  presetsDocument,
  ["schema_version", "presets", "cpu_presets"],
  ["schema_version", "presets", "cpu_presets"],
  "catalog/presets.json",
);
if (presetsDocument.schema_version !== 1)
  fail("catalog/presets.json.schema_version", "must equal 1");
if (!Array.isArray(presetsDocument.presets))
  fail("catalog/presets.json.presets", "must be an array");
if (!Array.isArray(presetsDocument.cpu_presets))
  fail("catalog/presets.json.cpu_presets", "must be an array");

const requiredPresetIds = [
  "vanilla",
  "balanced",
  "performance",
  "visual-quality",
];
const presetIds = presetsDocument.presets.map((preset) => preset.id);
unique(presetIds, "catalog/presets.json.presets ids");
if (
  requiredPresetIds.some((id) => !presetIds.includes(id)) ||
  presetIds.length !== requiredPresetIds.length
) {
  fail(
    "catalog/presets.json.presets",
    `must contain exactly ${requiredPresetIds.join(", ")}`,
  );
}

for (const [index, preset] of presetsDocument.presets.entries()) {
  const path = `catalog/presets.json.presets[${index}]`;
  exactKeys(
    preset,
    ["id", "name", "description", "changes"],
    ["id", "name", "description", "changes"],
    path,
  );
  if (!/^[a-z0-9-]+$/.test(preset.id))
    fail(`${path}.id`, "must be a lowercase slug");
  bilingual(preset.name, `${path}.name`);
  bilingual(preset.description, `${path}.description`);
  if (!Array.isArray(preset.changes))
    fail(`${path}.changes`, "must be an array");
  const seen = new Set();
  for (const [changeIndex, change] of preset.changes.entries()) {
    const changePath = `${path}.changes[${changeIndex}]`;
    exactKeys(
      change,
      ["section", "key", "value"],
      ["section", "key", "value"],
      changePath,
    );
    nonEmpty(change.section, `${changePath}.section`);
    nonEmpty(change.key, `${changePath}.key`);
    const identity = `${change.section}\u0000${change.key}`.toLocaleLowerCase(
      "en-US",
    );
    if (seen.has(identity))
      fail(changePath, "duplicates another change in this preset");
    seen.add(identity);
    const option = optionByIdentity.get(identity);
    if (!option) fail(changePath, "does not reference a catalog option");
    validatePresetValue(change.value, option, `${changePath}.value`);
    if (
      preset.id !== "vanilla" &&
      (option.status !== "verified" || !option.evidence.runtime_verified)
    ) {
      fail(
        changePath,
        "built-in non-vanilla presets require verified runtime evidence",
      );
    }
    if (preset.id === "vanilla" && change.value !== null)
      fail(changePath, "vanilla may only remove managed values");
  }
  if (preset.id === "vanilla" && seen.size !== optionByIdentity.size) {
    fail(path, "vanilla must remove every catalog-managed option exactly once");
  }
}

const requiredCpuIds = ["system-default", "prefer-performance", "custom"];
const cpuIds = presetsDocument.cpu_presets.map((preset) => preset.id);
unique(cpuIds, "catalog/presets.json.cpu_presets ids");
if (cpuIds.length !== 3 || requiredCpuIds.some((id) => !cpuIds.includes(id))) {
  fail(
    "catalog/presets.json.cpu_presets",
    `must contain exactly ${requiredCpuIds.join(", ")}`,
  );
}
for (const [index, preset] of presetsDocument.cpu_presets.entries()) {
  const path = `catalog/presets.json.cpu_presets[${index}]`;
  exactKeys(
    preset,
    [
      "id",
      "name",
      "description",
      "mode",
      "risk",
      "default_priority",
      "auto_select_elevated",
    ],
    [
      "id",
      "name",
      "description",
      "mode",
      "risk",
      "default_priority",
      "auto_select_elevated",
    ],
    path,
  );
  bilingual(preset.name, `${path}.name`);
  bilingual(preset.description, `${path}.description`);
  oneOf(preset.risk, risks, `${path}.risk`);
  oneOf(preset.mode, ["all", "prefer_performance", "custom"], `${path}.mode`);
  if (preset.default_priority !== "normal")
    fail(`${path}.default_priority`, "must remain normal");
  if (preset.auto_select_elevated !== false)
    fail(`${path}.auto_select_elevated`, "must remain false");
}

console.log(
  `Validated ${catalog.options.length} options, ${presetIds.length} INI presets, and ${cpuIds.length} CPU presets.`,
);
