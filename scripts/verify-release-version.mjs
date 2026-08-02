import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tag = process.env.TAG;

if (!tag?.startsWith("v") || tag.length === 1) {
  throw new Error("TAG must be a release tag starting with v");
}

const expectedVersion = tag.slice(1);
const metadata = JSON.parse(
  execFileSync(
    "cargo",
    ["metadata", "--locked", "--no-deps", "--format-version", "1"],
    { cwd: repositoryRoot, encoding: "utf8" },
  ),
);
const workspaceMembers = new Set(metadata.workspace_members);
const workspacePackages = metadata.packages.filter(({ id }) =>
  workspaceMembers.has(id),
);

const readPackageVersion = (relativePath) =>
  JSON.parse(readFileSync(resolve(repositoryRoot, relativePath), "utf8")).version;

const installer = readFileSync(
  resolve(repositoryRoot, "scripts/gaterust.sh"),
  "utf8",
);
const installerVersion = installer.match(/^SCRIPT_VERSION="([^"]+)"$/m)?.[1];
const readme = readFileSync(resolve(repositoryRoot, "README.md"), "utf8");
const documentedVersion = readme.match(/^version=([^\s]+)$/m)?.[1];

const versions = [
  ...workspacePackages.map(({ name, version }) => [
    `Cargo package ${name}`,
    version,
    expectedVersion,
  ]),
  ["client/package.json", readPackageVersion("client/package.json"), expectedVersion],
  ["web/package.json", readPackageVersion("web/package.json"), expectedVersion],
  ["scripts/gaterust.sh", installerVersion, tag],
  ["README.md release example", documentedVersion, expectedVersion],
];
const mismatches = versions.filter(([, actual, expected]) => actual !== expected);

if (mismatches.length > 0) {
  const details = mismatches
    .map(
      ([source, actual, expected]) =>
        `  ${source}: expected ${expected}, found ${actual ?? "no version"}`,
    )
    .join("\n");
  throw new Error(`Release versions do not match ${tag}:\n${details}`);
}

console.log(`Release versions match ${tag}`);
