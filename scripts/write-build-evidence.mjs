/** Record and verify bundle hashes against the exact checkout used by CI. */
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { resolve, join, relative } from "node:path";

const verify = process.argv[2] === "--verify";
const argument = process.argv[verify ? 3 : 2];
if (!argument) throw new Error("Usage: node scripts/write-build-evidence.mjs [--verify] <bundle-directory>");
const bundle = resolve(argument);
const provenance = join(bundle, "source-provenance.json");
const checksumFile = join(bundle, "SHA256SUMS");
const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
if (verify) {
  const evidence = JSON.parse(readFileSync(provenance, "utf8"));
  if (process.env.SOURCE_SHA && evidence.sourceCommit !== process.env.SOURCE_SHA) {
    throw new Error("Bundle source does not match the required source commit");
  }
  for (const artifact of evidence.artifacts) {
    const path = resolve(bundle, artifact.path);
    if (!path.startsWith(bundle + "/") && !path.startsWith(bundle + "\\")) {
      throw new Error("Artifact path escapes its bundle");
    }
    if (statSync(path).size !== artifact.bytes || hash(path) !== artifact.sha256) {
      throw new Error(`Artifact checksum mismatch: ${artifact.path}`);
    }
  }
  console.log(`Verified ${evidence.artifacts.length} artifacts from ${evidence.sourceCommit}`);
} else {
  const sourceCommit = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
  const sourceStatus = execFileSync("git", ["status", "--porcelain", "--untracked-files=normal"], { encoding: "utf8" }).trim();
  const expected = process.env.SOURCE_SHA;
  if (expected && sourceCommit !== expected) throw new Error("Checkout differs from requested source commit");
  if (expected && sourceStatus) throw new Error("Build changed tracked source or has untracked source files");
  const artifacts = [];
  function visit(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const path = join(directory, entry.name);
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) visit(path);
      else if (entry.isFile() && path !== provenance && path !== checksumFile) {
        artifacts.push({ path: relative(bundle, path).replaceAll("\\", "/"), bytes: statSync(path).size, sha256: hash(path) });
      }
    }
  }
  visit(bundle);
  artifacts.sort((a, b) => a.path.localeCompare(b.path));
  if (!artifacts.length) throw new Error("Packaging produced no bundle artifacts");
  writeFileSync(provenance, JSON.stringify({
    sourceCommit, expectedSourceCommit: expected ?? sourceCommit,
    sourceStatus,
    workflowRun: process.env.GITHUB_RUN_ID ?? null,
    workflowAttempt: process.env.GITHUB_RUN_ATTEMPT ?? null,
    platform: process.platform, recordedAt: new Date().toISOString(), artifacts,
  }, null, 2) + "\n");
  writeFileSync(checksumFile, artifacts.map((artifact) => `${artifact.sha256}  ${artifact.path}`).join("\n") + "\n");
  console.log(`Recorded ${artifacts.length} bundle artifacts from ${sourceCommit}`);
}
