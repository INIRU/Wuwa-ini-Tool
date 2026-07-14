import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

function fail(message) {
  throw new Error(`repository policy: ${message}`);
}

function readText(relativePath) {
  return readFileSync(join(root, relativePath), "utf8");
}

function readJson(relativePath) {
  try {
    return JSON.parse(readText(relativePath));
  } catch (error) {
    fail(`${relativePath} is not valid JSON: ${error.message}`);
  }
}

function cargoPackageVersion(source) {
  const packageHeader = source.match(/^\[package\]\s*$/m);
  if (!packageHeader || packageHeader.index === undefined) {
    fail("src-tauri/Cargo.toml has no [package] section");
  }
  const sectionStart = packageHeader.index + packageHeader[0].length;
  const remaining = source.slice(sectionStart);
  const nextSection = remaining.search(/^\[/m);
  const packageSection =
    nextSection === -1 ? remaining : remaining.slice(0, nextSection);
  const version = packageSection?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  if (!version) fail("src-tauri/Cargo.toml has no [package] version");
  return version;
}

const requiredFiles = [
  "README.md",
  "README.ko.md",
  "LICENSE",
  "DISCLAIMER.md",
  "CONTRIBUTING.md",
  "CODE_OF_CONDUCT.md",
  "SECURITY.md",
  "SUPPORT.md",
  "CHANGELOG.md",
  ".github/ISSUE_TEMPLATE/bug.yml",
  ".github/ISSUE_TEMPLATE/option-evidence.yml",
  ".github/ISSUE_TEMPLATE/feature.yml",
  ".github/ISSUE_TEMPLATE/config.yml",
  ".github/pull_request_template.md",
  ".github/dependabot.yml",
  ".github/workflows/ci.yml",
  ".github/workflows/release.yml",
];

for (const relativePath of requiredFiles) {
  if (!existsSync(join(root, relativePath))) fail(`missing ${relativePath}`);
}

const packageJson = readJson("package.json");
const packageLock = readJson("package-lock.json");
const tauriConfig = readJson("src-tauri/tauri.conf.json");
const cargoVersion = cargoPackageVersion(readText("src-tauri/Cargo.toml"));
const versions = {
  "package.json": packageJson.version,
  "package-lock.json": packageLock.version,
  "package-lock.json root package": packageLock.packages?.[""]?.version,
  "src-tauri/tauri.conf.json": tauriConfig.version,
  "src-tauri/Cargo.toml": cargoVersion,
};
const uniqueVersions = new Set(Object.values(versions));

if (Object.values(versions).some((version) => typeof version !== "string")) {
  fail(`one or more version fields are missing: ${JSON.stringify(versions)}`);
}
if (uniqueVersions.size !== 1) {
  fail(`version mismatch: ${JSON.stringify(versions)}`);
}

const [version] = uniqueVersions;
const semverPattern =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/;
if (!semverPattern.test(version)) fail(`invalid semantic version: ${version}`);
if (!readText("CHANGELOG.md").includes(`## [${version}]`)) {
  fail(`CHANGELOG.md has no ${version} entry`);
}
if (tauriConfig.productName !== "Wuwa ini Tool")
  fail("unexpected Tauri productName");
if (
  !Array.isArray(tauriConfig.bundle?.targets) ||
  !tauriConfig.bundle.targets.includes("nsis")
) {
  fail("Tauri bundle targets must include nsis");
}

const workflowText = [
  readText(".github/workflows/ci.yml"),
  readText(".github/workflows/release.yml"),
].join("\n");
if (/\bpull_request_target\b/.test(workflowText)) {
  fail("pull_request_target is forbidden for this repository");
}

const args = process.argv.slice(2);
const tagIndex = args.indexOf("--tag");
if (tagIndex !== -1) {
  const tag = args[tagIndex + 1];
  if (!tag) fail("--tag requires a value");
  if (tag !== `v${version}`)
    fail(`tag ${tag} does not match package version v${version}`);
}

if (args.includes("--release")) {
  const updater = tauriConfig.plugins?.updater;
  const updaterArtifacts = tauriConfig.bundle?.createUpdaterArtifacts;
  const expectedEndpoint =
    "https://github.com/INIRU/Wuwa-ini-Tool/releases/latest/download/latest.json";

  if (updaterArtifacts !== true && updaterArtifacts !== "v1Compatible") {
    fail("release requires bundle.createUpdaterArtifacts");
  }
  const publicKey =
    typeof updater?.pubkey === "string" ? updater.pubkey.trim() : "";
  if (
    publicKey.length < 64 ||
    /(placeholder|replace|todo|example|changeme)/i.test(publicKey)
  ) {
    fail("release requires a non-placeholder updater public key");
  }
  if (
    !Array.isArray(updater?.endpoints) ||
    !updater.endpoints.includes(expectedEndpoint)
  ) {
    fail(`release requires updater endpoint ${expectedEndpoint}`);
  }
  if (!existsSync(join(root, "src-tauri/icons/icon.ico"))) {
    fail("release requires src-tauri/icons/icon.ico");
  }
}

console.log(
  `Repository versions and policy files are consistent at ${version}.`,
);
