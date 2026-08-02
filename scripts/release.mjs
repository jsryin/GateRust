#!/usr/bin/env node

import { execFileSync, spawnSync } from "node:child_process";
import {
  readFileSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const versionFiles = [
  "Cargo.toml",
  "Cargo.lock",
  "client/package.json",
  "web/package.json",
  "scripts/gaterust.sh",
  "README.md",
];
const semverPattern =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

function usage() {
  console.log(`用法：
  ./scripts/release.mjs <version> [--push]

示例：
  ./scripts/release.mjs 0.1.2
  ./scripts/release.mjs v0.1.2-beta.1 --push

默认只创建本地发布提交和 Tag。--push 会将当前分支和 Tag 原子推送到 origin。`);
}

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    cwd: repositoryRoot,
    encoding: "utf8",
    ...options,
  });
}

function commandSucceeds(command, args) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    stdio: "ignore",
  });
  if (result.error) {
    throw result.error;
  }
  return result.status === 0;
}

function replaceOnce(content, pattern, replacement, source) {
  const flags = pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`;
  const matches = content.match(new RegExp(pattern.source, flags));
  if (matches?.length !== 1) {
    throw new Error(`${source} 中应当恰好有一个版本字段，实际找到 ${matches?.length ?? 0} 个`);
  }
  return content.replace(pattern, replacement);
}

function workspaceVersion(content) {
  const section = content.match(
    /^\[workspace\.package\]\s*$([\s\S]*?)(?=^\[|(?![\s\S]))/m,
  )?.[1];
  const version = section?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  if (!version) {
    throw new Error("无法读取 Cargo.toml 中的 workspace.package.version");
  }
  return version;
}

function updateWorkspaceVersion(content, currentVersion, nextVersion) {
  const sectionPattern =
    /(^\[workspace\.package\]\s*$)([\s\S]*?)(?=^\[|(?![\s\S]))/m;
  const sectionMatch = content.match(sectionPattern);
  if (!sectionMatch) {
    throw new Error("Cargo.toml 缺少 [workspace.package]");
  }
  const updatedSection = replaceOnce(
    sectionMatch[2],
    /^version\s*=\s*"[^"]+"\s*$/m,
    `version = "${nextVersion}"`,
    "Cargo.toml 的 [workspace.package]",
  );
  if (!sectionMatch[2].includes(`"${currentVersion}"`)) {
    throw new Error(`Cargo.toml 的当前版本不是 ${currentVersion}`);
  }
  return content.replace(
    sectionPattern,
    (_section, header) => `${header}${updatedSection}`,
  );
}

function updatePackageJson(content, nextVersion, source) {
  const packageMetadata = JSON.parse(content);
  if (typeof packageMetadata.version !== "string") {
    throw new Error(`${source} 缺少字符串类型的 version`);
  }
  packageMetadata.version = nextVersion;
  return `${JSON.stringify(packageMetadata, null, 2)}\n`;
}

function parseArguments() {
  const argumentsList = process.argv.slice(2);
  if (argumentsList.includes("--help") || argumentsList.includes("-h")) {
    usage();
    process.exit(0);
  }
  const unknownOptions = argumentsList.filter(
    (argument) => argument.startsWith("-") && argument !== "--push",
  );
  const versions = argumentsList.filter((argument) => !argument.startsWith("-"));
  if (unknownOptions.length > 0 || versions.length !== 1) {
    usage();
    throw new Error(
      unknownOptions.length > 0
        ? `不支持的参数：${unknownOptions.join(", ")}`
        : "必须提供一个发布版本",
    );
  }
  const version = versions[0].startsWith("v")
    ? versions[0].slice(1)
    : versions[0];
  if (!semverPattern.test(version)) {
    throw new Error(`版本 ${versions[0]} 不是有效的 SemVer`);
  }
  return { push: argumentsList.includes("--push"), version };
}

function verifyVersions(tag) {
  run(process.execPath, [resolve(repositoryRoot, "scripts/verify-release-version.mjs")], {
    env: { ...process.env, TAG: tag },
    stdio: "inherit",
  });
}

function main() {
  const { push, version } = parseArguments();
  const tag = `v${version}`;
  const status = run("git", ["status", "--porcelain=v1", "--untracked-files=all"]).trim();
  if (status) {
    throw new Error("发布前 Git 工作区必须保持干净");
  }
  const branch = run("git", ["symbolic-ref", "--quiet", "--short", "HEAD"]).trim();
  if (!branch) {
    throw new Error("不能从 detached HEAD 创建发布");
  }
  if (commandSucceeds("git", ["show-ref", "--verify", "--quiet", `refs/tags/${tag}`])) {
    throw new Error(`Tag ${tag} 已存在`);
  }

  const cargoToml = readFileSync(resolve(repositoryRoot, "Cargo.toml"), "utf8");
  const currentVersion = workspaceVersion(cargoToml);
  if (currentVersion === version) {
    throw new Error(`当前版本已经是 ${version}`);
  }
  verifyVersions(`v${currentVersion}`);

  const originals = new Map(
    versionFiles.map((relativePath) => [
      relativePath,
      readFileSync(resolve(repositoryRoot, relativePath), "utf8"),
    ]),
  );

  try {
    writeFileSync(
      resolve(repositoryRoot, "Cargo.toml"),
      updateWorkspaceVersion(cargoToml, currentVersion, version),
    );
    for (const relativePath of ["client/package.json", "web/package.json"]) {
      writeFileSync(
        resolve(repositoryRoot, relativePath),
        updatePackageJson(originals.get(relativePath), version, relativePath),
      );
    }
    writeFileSync(
      resolve(repositoryRoot, "scripts/gaterust.sh"),
      replaceOnce(
        originals.get("scripts/gaterust.sh"),
        /^SCRIPT_VERSION="[^"]+"$/m,
        `SCRIPT_VERSION="${tag}"`,
        "scripts/gaterust.sh",
      ),
    );
    writeFileSync(
      resolve(repositoryRoot, "README.md"),
      replaceOnce(
        originals.get("README.md"),
        /^version=[^\s]+$/m,
        `version=${version}`,
        "README.md 发布示例",
      ),
    );

    run("cargo", ["update", "--workspace", "--offline"], {
      stdio: "inherit",
    });
    verifyVersions(tag);
    run("git", ["diff", "--check"], { stdio: "inherit" });
  } catch (error) {
    for (const [relativePath, content] of originals) {
      writeFileSync(resolve(repositoryRoot, relativePath), content);
    }
    throw error;
  }

  run("git", ["add", "--", ...versionFiles], { stdio: "inherit" });
  run("git", ["commit", "-m", `chore: release ${tag}`], { stdio: "inherit" });
  run("git", ["tag", "-a", tag, "-m", `GateRust ${tag}`], { stdio: "inherit" });

  if (push) {
    run(
      "git",
      [
        "push",
        "--atomic",
        "origin",
        `refs/heads/${branch}:refs/heads/${branch}`,
        `refs/tags/${tag}:refs/tags/${tag}`,
      ],
      { stdio: "inherit" },
    );
    console.log(`${tag} 已推送，GitHub Actions 将自动开始构建`);
  } else {
    console.log(`${tag} 已在本地创建；确认后执行 git push --atomic origin ${branch} ${tag}`);
  }
}

try {
  main();
} catch (error) {
  console.error(`发布失败：${error instanceof Error ? error.message : String(error)}`);
  process.exitCode = 1;
}
