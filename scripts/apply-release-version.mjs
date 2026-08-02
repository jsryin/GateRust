import { execFile } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { versionFromReleaseTag } from "./release-version.mjs";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tag = process.env.TAG;
const version = versionFromReleaseTag(tag);
const execFileAsync = promisify(execFile);

const readRepositoryFile = (relativePath) =>
  readFileSync(resolve(repositoryRoot, relativePath), "utf8");

const writeRepositoryFile = (relativePath, contents) => {
  const path = resolve(repositoryRoot, relativePath);
  if (readFileSync(path, "utf8") !== contents) {
    writeFileSync(path, contents, "utf8");
  }
};

const replaceWorkspaceVersion = (contents) => {
  let currentSection = "";
  let replacements = 0;
  const updated = contents
    .split(/(?<=\n)/u)
    .map((line) => {
      const section = line.match(/^\s*\[([^\]\r\n]+)\]\s*(?:#.*)?(?:\r?\n)?$/u);
      if (section) {
        currentSection = section[1];
        return line;
      }
      if (currentSection !== "workspace.package") {
        return line;
      }
      return line.replace(
        /^(\s*version\s*=\s*")[^"\r\n]*("[^\r\n]*)(\r?\n)?$/u,
        (_match, prefix, suffix, lineEnding = "") => {
          replacements += 1;
          return `${prefix}${version}${suffix}${lineEnding}`;
        },
      );
    })
    .join("");

  if (replacements !== 1) {
    throw new Error(
      `Expected one workspace package version, found ${replacements}`,
    );
  }
  return updated;
};

const updatePackageJson = (relativePath) => {
  const packageJson = JSON.parse(readRepositoryFile(relativePath));
  packageJson.version = version;
  writeRepositoryFile(relativePath, `${JSON.stringify(packageJson, null, 2)}\n`);
};

const updateInstaller = () => {
  const relativePath = "scripts/gaterust.sh";
  let replacements = 0;
  const updated = readRepositoryFile(relativePath).replace(
    /^SCRIPT_VERSION="[^"]*"$/mu,
    () => {
      replacements += 1;
      return `SCRIPT_VERSION="${tag}"`;
    },
  );
  if (replacements !== 1) {
    throw new Error(`Expected one SCRIPT_VERSION, found ${replacements}`);
  }
  writeRepositoryFile(relativePath, updated);
};

writeRepositoryFile(
  "Cargo.toml",
  replaceWorkspaceVersion(readRepositoryFile("Cargo.toml")),
);
updatePackageJson("client/package.json");
updatePackageJson("web/package.json");
updateInstaller();

await execFileAsync(
  "cargo",
  ["update", "--package", "gaterust-client", "--precise", version],
  {
    cwd: repositoryRoot,
    encoding: "utf8",
  },
);

console.log(`Applied release version ${tag}`);
